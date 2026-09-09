//! Host-composed schedule timing and bounded Run steps; no overlap policy is delegated to native queues.
use crate::scheduled_run_worker::ScheduledRunWorker;
use agent_automation::{RunId, ScheduleId};
use automation_storage::{AutomationStore, StorageError};
use communication_protocol::{EndpointRef, SessionRef};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
pub struct ScheduleTimingWorker {
    store: Arc<Mutex<AutomationStore>>,
    runner: ScheduledRunWorker,
}
impl ScheduleTimingWorker {
    pub(crate) fn new(
        store: Arc<Mutex<AutomationStore>>,
        backend: Option<crate::NativeControlBackend>,
        configuration: crate::AutomationConfigurationHandle,
    ) -> Self {
        Self {
            store: Arc::clone(&store),
            runner: ScheduledRunWorker {
                store,
                backend,
                configuration,
            },
        }
    }
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut active = HashMap::<tokio::task::Id, RunId>::new();
        // Scan progress is only a fairness hint; durable Run state still owns exclusion.
        let mut run_cursor = None;
        let mut waiting_cursor = None;
        let mut tasks = tokio::task::JoinSet::<(RunId, Result<(), StorageError>)>::new();
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                    return;
                }
                completed = tasks.join_next_with_id(), if !tasks.is_empty() => {
                    match completed {
                        Some(Ok((task_id, (_, result)))) => {
                            active.remove(&task_id);
                            if result.is_err() {
                                tracing::warn!("scheduled Run step unavailable; retained state remains inspectable");
                            }
                        }
                        Some(Err(error)) => {
                            active.remove(&error.id());
                            tracing::warn!("scheduled Run task interrupted; retained effect state requires inspection");
                        }
                        None => {}
                    }
                    continue;
                }
                _ = interval.tick() => {}
            }
            if self.tick(&mut waiting_cursor).await.is_err() {
                tracing::warn!(
                    "schedule timing storage unavailable; no new Run admission in this pass"
                );
                continue;
            }
            let ids = self
                .store
                .lock()
                .await
                .observable_run_ids(run_cursor.as_ref())
                .await;
            let Ok(ids) = ids else {
                continue;
            };
            if ids.is_empty() {
                run_cursor = None;
            }
            for id in ids {
                if active.len() >= 16 {
                    break;
                }
                run_cursor = Some(id.clone());
                if !active.values().any(|active_id| active_id == &id) {
                    let runner = self.runner.clone();
                    let run_id = id.clone();
                    let task = tasks.spawn(async move {
                        let result =
                            tokio::time::timeout(Duration::from_secs(35), runner.step(id.clone()))
                                .await
                                .unwrap_or(Err(StorageError::InvalidRecord));
                        (id, result)
                    });
                    active.insert(task.id(), run_id);
                }
            }
        }
    }
    async fn tick(&self, waiting_cursor: &mut Option<ScheduleId>) -> Result<(), StorageError> {
        let now = chrono::Utc::now().timestamp_millis();
        let due = self.store.lock().await.due_schedule_ids(now).await?;
        for id in due {
            self.store
                .lock()
                .await
                .enqueue_due_run::<SessionRef, EndpointRef>(&id, now)
                .await?;
        }
        let waiting = self
            .store
            .lock()
            .await
            .waiting_schedule_ids(waiting_cursor.as_ref())
            .await?;
        if waiting.is_empty() {
            *waiting_cursor = None;
        }
        for id in waiting {
            *waiting_cursor = Some(id.clone());
            let configuration_lease = self.runner.configuration.admission_lease().await;
            if configuration_lease.configuration().is_none() {
                break;
            }
            self.store
                .lock()
                .await
                .admit_waiting_run::<SessionRef, EndpointRef>(&id, now)
                .await?;
        }
        Ok(())
    }
}
