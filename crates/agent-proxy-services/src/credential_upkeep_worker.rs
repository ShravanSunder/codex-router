//! Long-lived enabled-account OAuth upkeep, independent of quota observation.

use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use codex_router_auth::resolver::AsyncRouterCredentialResolver;
use codex_router_auth::resolver::CredentialRefreshClient;
use codex_router_auth::resolver::CredentialRefreshTaskSupervisor;
use codex_router_auth::resolver::ProviderCredentialRefreshClients;
use codex_router_auth::resolver::current_unix_seconds;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_state::account::AccountStatus;
use codex_router_state::credential_maintenance::CredentialMaintenanceState;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[path = "credential_upkeep_worker/telemetry.rs"]
mod telemetry;
use telemetry::TelemetryCredentialUpkeepRefreshClient;

const UPKEEP_CYCLE_SECONDS: u64 = 180;
const LOCAL_FAILURE_RETRY_SECONDS: u64 = 60;
const MAX_CONCURRENT_ACCOUNTS: usize = 4;

/// A running OAuth upkeep loop retaining every acquired renewal wrapper.
pub struct CredentialUpkeepWorker {
    control_sender: UnboundedSender<WorkerControl>,
    stop_requested: CancellationToken,
    task: Option<JoinHandle<()>>,
    #[cfg(test)]
    refresh_client_type: std::any::TypeId,
}

impl CredentialUpkeepWorker {
    pub(crate) fn request_stop(&self) {
        self.stop_requested.cancel();
        let _ = self.control_sender.send(WorkerControl::Stop);
    }

    pub async fn shutdown(&mut self) {
        self.request_stop();
        let _result = self.join_stopped().await;
    }

    pub(crate) async fn join_stopped(&mut self) -> Result<(), tokio::task::JoinError> {
        let result = match self.task.as_mut() {
            Some(task) => task.await,
            None => Ok(()),
        };
        self.task = None;
        result
    }
}

impl Drop for CredentialUpkeepWorker {
    fn drop(&mut self) {
        self.request_stop();
    }
}

enum WorkerControl {
    Stop,
    #[cfg(any(test, feature = "test-support"))]
    Wake,
}

#[cfg(any(test, feature = "test-support"))]
impl CredentialUpkeepWorker {
    pub fn wake_for_test(&self) -> Result<(), std::io::Error> {
        self.control_sender.send(WorkerControl::Wake).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "fixture worker control closed",
            )
        })
    }

    pub fn wake_handle_for_test(&self) -> CredentialUpkeepWakeHandle {
        CredentialUpkeepWakeHandle {
            control_sender: self.control_sender.clone(),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct CredentialUpkeepWakeHandle {
    control_sender: UnboundedSender<WorkerControl>,
}

#[cfg(any(test, feature = "test-support"))]
impl CredentialUpkeepWakeHandle {
    pub fn wake(&self) -> Result<(), std::io::Error> {
        self.control_sender.send(WorkerControl::Wake).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "fixture worker control closed",
            )
        })
    }
}

#[derive(Debug, Error)]
pub enum CredentialUpkeepStartError {
    #[error("credential upkeep state unavailable")]
    State(#[from] StateStoreError),
}

pub async fn start_background_credential_upkeep_worker(
    state_db_path: PathBuf,
    secret_store: EncryptedCredentialStore,
    refresh_tasks: CredentialRefreshTaskSupervisor,
) -> Result<CredentialUpkeepWorker, CredentialUpkeepStartError> {
    start_background_credential_upkeep_worker_with_client_and_clock(
        state_db_path,
        secret_store,
        refresh_tasks,
        ProviderCredentialRefreshClients::new(),
        || current_unix_seconds().unwrap_or(0),
    )
    .await
}

pub async fn start_background_credential_upkeep_worker_with_client_and_clock<C, F>(
    state_db_path: PathBuf,
    secrets: EncryptedCredentialStore,
    refresh_tasks: CredentialRefreshTaskSupervisor,
    refresh_client: C,
    observed_clock: F,
) -> Result<CredentialUpkeepWorker, CredentialUpkeepStartError>
where
    C: CredentialRefreshClient + Clone + Send + Sync + 'static,
    F: Fn() -> u64 + Send + Sync + 'static,
{
    let state = AsyncSqliteStateStore::open(&state_db_path).await?;
    let (control_sender, mut control_receiver) = tokio::sync::mpsc::unbounded_channel();
    let stop_requested = CancellationToken::new();
    let worker_stop = stop_requested.clone();
    let worker_refresh_tasks = refresh_tasks.clone();
    let task = tokio::spawn(async move {
        loop {
            if worker_stop.is_cancelled() {
                break;
            }
            let cycle_started = Instant::now();
            let observed_now = observed_clock();
            let cycle_result = run_upkeep_cycle_until_stop(
                &state,
                &secrets,
                worker_refresh_tasks.clone(),
                refresh_client.clone(),
                observed_now,
                &worker_stop,
            )
            .await;
            if worker_stop.is_cancelled() {
                break;
            }
            let remaining =
                bounded_upkeep_wait(cycle_result, observed_now, cycle_started.elapsed());
            tokio::select! {
                biased;
                _ = worker_stop.cancelled() => break,
                control = control_receiver.recv() => match control {
                    Some(WorkerControl::Stop) | None => break,
                    #[cfg(any(test, feature = "test-support"))]
                    Some(WorkerControl::Wake) => {}
                },
                _ = tokio::time::sleep(remaining) => {}
            }
        }
        if let Err(error) = state.close().await {
            eprintln!("credential upkeep state close failed: {error}");
        }
    });
    Ok(CredentialUpkeepWorker {
        control_sender,
        stop_requested,
        task: Some(task),
        #[cfg(test)]
        refresh_client_type: std::any::TypeId::of::<C>(),
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct UpkeepCycleResult {
    earliest_due: Option<u64>,
    had_local_error: bool,
}

fn bounded_upkeep_wait(
    result: UpkeepCycleResult,
    observed_now: u64,
    cycle_elapsed: Duration,
) -> Duration {
    let due_delay = result
        .earliest_due
        .map(|deadline| deadline.saturating_sub(observed_now).max(1))
        .unwrap_or(UPKEEP_CYCLE_SECONDS)
        .min(UPKEEP_CYCLE_SECONDS);
    let due_remaining = Duration::from_secs(due_delay)
        .saturating_sub(cycle_elapsed)
        .max(Duration::from_secs(1));
    if !result.had_local_error {
        return due_remaining;
    }
    let local_retry = Duration::from_secs(LOCAL_FAILURE_RETRY_SECONDS);
    let Some(deadline) = result
        .earliest_due
        .filter(|deadline| *deadline > observed_now)
    else {
        return local_retry;
    };
    let until_due = Duration::from_secs(deadline - observed_now).saturating_sub(cycle_elapsed);
    if until_due.is_zero() {
        local_retry
    } else {
        until_due.min(local_retry)
    }
}

#[cfg(test)]
async fn run_upkeep_cycle<C>(
    state: &AsyncSqliteStateStore,
    secrets: &EncryptedCredentialStore,
    refresh_client: C,
    observed_now: u64,
) -> UpkeepCycleResult
where
    C: CredentialRefreshClient + Clone + Send + Sync + 'static,
{
    run_upkeep_cycle_until_stop(
        state,
        secrets,
        CredentialRefreshTaskSupervisor::new(),
        refresh_client,
        observed_now,
        &CancellationToken::new(),
    )
    .await
}

async fn run_upkeep_cycle_until_stop<C>(
    state: &AsyncSqliteStateStore,
    secrets: &EncryptedCredentialStore,
    refresh_tasks: CredentialRefreshTaskSupervisor,
    refresh_client: C,
    observed_now: u64,
    stop_requested: &CancellationToken,
) -> UpkeepCycleResult
where
    C: CredentialRefreshClient + Clone + Send + Sync + 'static,
{
    if !matches!(
        secrets.status(),
        codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus::Ready
    ) {
        return UpkeepCycleResult::default();
    }
    let accounts = match state.list_accounts().await {
        Ok(accounts) => accounts,
        Err(error) => {
            eprintln!("credential upkeep account discovery failed: {error}");
            return UpkeepCycleResult {
                earliest_due: None,
                had_local_error: true,
            };
        }
    };
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_ACCOUNTS));
    let mut tasks = JoinSet::new();
    for account in accounts {
        if stop_requested.is_cancelled() {
            break;
        }
        if account.status() != AccountStatus::Enabled {
            continue;
        }
        let Some(account_generation) = account.active_credential_generation() else {
            continue;
        };
        let resolver = AsyncRouterCredentialResolver::new(
            state.clone(),
            secrets.clone(),
            TelemetryCredentialUpkeepRefreshClient::new(refresh_client.clone()),
            Some(observed_now),
        )
        .with_refresh_task_supervisor(refresh_tasks.clone());
        let account_id = account.account_id().clone();
        let health_state = state.clone();
        let account_semaphore = std::sync::Arc::clone(&semaphore);
        let account_stop = stop_requested.clone();
        tasks.spawn(async move {
            let permit = tokio::select! {
                biased;
                _ = account_stop.cancelled() => return (None, false),
                permit = account_semaphore.acquire_owned() => permit,
            };
            let Ok(_permit) = permit else {
                return (None, true);
            };
            if account_stop.is_cancelled() {
                return (None, false);
            }
            let maintenance_failed = resolver
                .maintain_account_credentials(&account_id)
                .await
                .is_err();
            let recorded_state = if maintenance_failed {
                health_state
                    .load_credential_maintenance(&account_id)
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            let current_record = recorded_state
                .as_ref()
                .filter(|record| record.credential_generation == account_generation);
            let current_state = current_record.map(|record| record.state);
            let recorded_outcome = current_record.is_some_and(|record| match record.state {
                CredentialMaintenanceState::Retrying => record
                    .next_attempt_unix_seconds
                    .is_some_and(|deadline| deadline > observed_now),
                CredentialMaintenanceState::ReauthRequired
                | CredentialMaintenanceState::Unrefreshable
                | CredentialMaintenanceState::InProgress => true,
                CredentialMaintenanceState::Healthy => false,
            });
            let local_unrecorded_failure = maintenance_failed && !recorded_outcome;
            match resolver
                .next_maintenance_due_unix_seconds(&account_id)
                .await
            {
                Ok(_) if current_state == Some(CredentialMaintenanceState::InProgress) => {
                    (None, false)
                }
                Ok(deadline) => {
                    if local_unrecorded_failure {
                        eprintln!("credential upkeep local failure was not recorded");
                    }
                    (deadline, local_unrecorded_failure)
                }
                Err(error) => {
                    eprintln!("credential upkeep status unavailable: {error}");
                    (None, true)
                }
            }
        });
    }
    let mut cycle_result = UpkeepCycleResult::default();
    loop {
        let next_result = if stop_requested.is_cancelled() {
            tasks.join_next().await
        } else {
            tokio::select! {
                biased;
                _ = stop_requested.cancelled() => continue,
                result = tasks.join_next() => result,
            }
        };
        let Some(result) = next_result else { break };
        match result {
            Ok((deadline, failed)) => {
                cycle_result.had_local_error |= failed;
                if let Some(deadline) = deadline {
                    cycle_result.earliest_due = Some(
                        cycle_result
                            .earliest_due
                            .map_or(deadline, |earlier| earlier.min(deadline)),
                    );
                }
            }
            Err(_) => cycle_result.had_local_error = true,
        }
    }
    cycle_result
}

#[cfg(test)]
mod tests;
