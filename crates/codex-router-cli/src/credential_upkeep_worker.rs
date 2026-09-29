//! Long-lived enabled-account OAuth upkeep, independent of quota observation.

use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use codex_router_auth::resolver::AsyncRouterCredentialResolver;
use codex_router_auth::resolver::CredentialRefreshClient;
#[cfg(test)]
use codex_router_auth::resolver::NoopCredentialRefreshClient;
#[cfg(not(test))]
use codex_router_auth::resolver::OpenAiOAuthRefreshClient;
use codex_router_auth::resolver::current_unix_seconds;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::account::AccountStatus;
use codex_router_state::credential_maintenance::CredentialMaintenanceState;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const UPKEEP_CYCLE_SECONDS: u64 = 180;
const LOCAL_FAILURE_RETRY_SECONDS: u64 = 60;
const MAX_CONCURRENT_ACCOUNTS: usize = 4;
const SHUTDOWN_DRAIN_SECONDS: u64 = 30;

/// A running OAuth upkeep loop with a bounded shutdown drain.
pub(crate) struct CredentialUpkeepWorker {
    control_sender: mpsc::Sender<WorkerControl>,
    stopped_receiver: mpsc::Receiver<()>,
    stop_requested: CancellationToken,
    shutdown_deadline: Arc<OnceLock<Instant>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for CredentialUpkeepWorker {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(SHUTDOWN_DRAIN_SECONDS);
        let _ = self.shutdown_deadline.set(deadline);
        self.stop_requested.cancel();
        let _ = self.control_sender.send(WorkerControl::Stop);
        if self
            .stopped_receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
    }
}

enum WorkerControl {
    Stop,
    #[cfg(test)]
    Wake,
}

#[cfg(test)]
impl CredentialUpkeepWorker {
    pub(crate) fn wake_for_test(&self) {
        self.control_sender
            .send(WorkerControl::Wake)
            .expect("worker control should send");
    }

    pub(crate) fn wake_handle_for_test(&self) -> CredentialUpkeepWakeHandle {
        CredentialUpkeepWakeHandle {
            control_sender: self.control_sender.clone(),
        }
    }
}

#[cfg(test)]
pub(crate) struct CredentialUpkeepWakeHandle {
    control_sender: mpsc::Sender<WorkerControl>,
}

#[cfg(test)]
impl CredentialUpkeepWakeHandle {
    pub(crate) fn wake(&self) {
        self.control_sender
            .send(WorkerControl::Wake)
            .expect("worker control should send");
    }
}

#[derive(Debug, Error)]
pub enum CredentialUpkeepStartError {
    #[error("credential upkeep runtime unavailable")]
    Runtime(#[source] std::io::Error),
    #[error("credential upkeep state unavailable")]
    State(#[source] StateStoreError),
    #[error("credential upkeep secret store unavailable")]
    Secret(#[source] SecretStoreError),
    #[error("credential upkeep thread unavailable")]
    Thread(#[source] std::io::Error),
}

pub(crate) fn start_background_credential_upkeep_worker(
    state_db_path: &Path,
    secret_store: EncryptedCredentialStore,
) -> Result<CredentialUpkeepWorker, CredentialUpkeepStartError> {
    #[cfg(test)]
    return start_background_credential_upkeep_worker_with_client_and_clock(
        state_db_path,
        secret_store,
        NoopCredentialRefreshClient,
        || current_unix_seconds().unwrap_or(0),
    );
    #[cfg(not(test))]
    start_background_credential_upkeep_worker_with_client_and_clock(
        state_db_path,
        secret_store,
        OpenAiOAuthRefreshClient::new(),
        || current_unix_seconds().unwrap_or(0),
    )
}

pub(crate) fn start_background_credential_upkeep_worker_with_client_and_clock<C, F>(
    state_db_path: &Path,
    secrets: EncryptedCredentialStore,
    refresh_client: C,
    observed_clock: F,
) -> Result<CredentialUpkeepWorker, CredentialUpkeepStartError>
where
    C: CredentialRefreshClient + Clone + Send + Sync + 'static,
    F: Fn() -> u64 + Send + Sync + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(MAX_CONCURRENT_ACCOUNTS)
        .enable_all()
        .build()
        .map_err(CredentialUpkeepStartError::Runtime)?;
    let state = runtime
        .block_on(AsyncSqliteStateStore::open(state_db_path))
        .map_err(CredentialUpkeepStartError::State)?;
    let (control_sender, control_receiver) = mpsc::channel();
    let (stopped_sender, stopped_receiver) = mpsc::channel();
    let stop_requested = CancellationToken::new();
    let worker_stop = stop_requested.clone();
    let shutdown_deadline = Arc::new(OnceLock::new());
    let worker_deadline = Arc::clone(&shutdown_deadline);
    let thread = thread::Builder::new()
        .name("router-credential-upkeep".to_owned())
        .spawn(move || {
            loop {
                if worker_stop.is_cancelled() {
                    break;
                }
                let cycle_started = Instant::now();
                let observed_now = observed_clock();
                let cycle_result = runtime.block_on(run_upkeep_cycle_until_stop(
                    &state,
                    &secrets,
                    refresh_client.clone(),
                    observed_now,
                    &worker_stop,
                    &worker_deadline,
                ));
                if worker_stop.is_cancelled() {
                    break;
                }
                let remaining =
                    bounded_upkeep_wait(cycle_result, observed_now, cycle_started.elapsed());
                match control_receiver.recv_timeout(remaining) {
                    Ok(WorkerControl::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    #[cfg(test)]
                    Ok(WorkerControl::Wake) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            let close_budget = remaining_shutdown_time(&worker_deadline);
            if !close_budget.is_zero() {
                match runtime
                    .block_on(async { tokio::time::timeout(close_budget, state.close()).await })
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => eprintln!("credential upkeep state close failed: {error}"),
                    Err(_) => eprintln!("credential upkeep state close exceeded drain bound"),
                }
            }
            runtime.shutdown_timeout(remaining_shutdown_time(&worker_deadline));
            let _ = stopped_sender.send(());
        })
        .map_err(CredentialUpkeepStartError::Thread)?;
    Ok(CredentialUpkeepWorker {
        control_sender,
        stopped_receiver,
        stop_requested,
        shutdown_deadline,
        thread: Some(thread),
    })
}

fn remaining_shutdown_time(deadline: &OnceLock<Instant>) -> Duration {
    deadline
        .get()
        .map_or(Duration::from_secs(SHUTDOWN_DRAIN_SECONDS), |deadline| {
            deadline.saturating_duration_since(Instant::now())
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
        refresh_client,
        observed_now,
        &CancellationToken::new(),
        &OnceLock::new(),
    )
    .await
}

async fn run_upkeep_cycle_until_stop<C>(
    state: &AsyncSqliteStateStore,
    secrets: &EncryptedCredentialStore,
    refresh_client: C,
    observed_now: u64,
    stop_requested: &CancellationToken,
    shutdown_deadline: &OnceLock<Instant>,
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
            refresh_client.clone(),
            Some(observed_now),
        );
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
            let remaining = remaining_shutdown_time(shutdown_deadline);
            if remaining.is_zero() {
                tasks.abort_all();
                break;
            }
            match tokio::time::timeout(remaining, tasks.join_next()).await {
                Ok(result) => result,
                Err(_) => {
                    tasks.abort_all();
                    break;
                }
            }
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
