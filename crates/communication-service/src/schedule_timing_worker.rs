//! Host-composed schedule timing and bounded Run steps; no overlap policy is delegated to native queues.
use crate::scheduled_run_worker::ScheduledRunWorker;
use agent_automation::RunId;
use automation_storage::{AutomationStore, StorageError};
use communication_protocol::{EndpointRef, SessionRef};
use std::{collections::HashSet, sync::Arc, time::Duration};
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
    ) -> Self {
        Self {
            store: Arc::clone(&store),
            runner: ScheduledRunWorker {
                store,
                backend,
                timeout_seconds: 3600,
                summary_timeout_seconds: 900,
            },
        }
    }
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut active = HashSet::<RunId>::new();
        let mut tasks = tokio::task::JoinSet::<(RunId, Result<(), StorageError>)>::new();
        loop {
            tokio::select! {biased;_=shutdown.cancelled()=>{tasks.abort_all();while tasks.join_next().await.is_some(){}return;},completed=tasks.join_next(),if !tasks.is_empty()=>{
                match completed{Some(Ok((id,result)))=>{active.remove(&id);if result.is_err(){tracing::warn!("scheduled Run step unavailable; retained state remains inspectable");}},_=>{tracing::warn!("scheduled Run task interrupted; retained effect state requires inspection");}}
                continue;
            },_=interval.tick()=>{}}
            if self.tick().await.is_err() {
                tracing::warn!(
                    "schedule timing storage unavailable; no new Run admission in this pass"
                );
                continue;
            }
            let ids = self.store.lock().await.observable_run_ids().await;
            let Ok(ids) = ids else {
                continue;
            };
            for id in ids {
                if active.len() >= 16 {
                    break;
                }
                if active.insert(id.clone()) {
                    let runner = self.runner.clone();
                    tasks.spawn(async move {
                        let result =
                            tokio::time::timeout(Duration::from_secs(35), runner.step(id.clone()))
                                .await
                                .unwrap_or(Err(StorageError::InvalidRecord));
                        (id, result)
                    });
                }
            }
        }
    }
    async fn tick(&self) -> Result<(), StorageError> {
        let now = chrono::Utc::now().timestamp_millis();
        let due = self.store.lock().await.due_schedule_ids(now).await?;
        for id in due {
            self.store
                .lock()
                .await
                .enqueue_due_run::<SessionRef, EndpointRef>(&id, now)
                .await?;
        }
        let waiting = self.store.lock().await.waiting_schedule_ids().await?;
        for id in waiting {
            self.store
                .lock()
                .await
                .admit_waiting_run::<SessionRef, EndpointRef>(&id, now)
                .await?;
        }
        Ok(())
    }
}
