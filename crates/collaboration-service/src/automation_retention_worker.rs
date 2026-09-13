//! Bounded event maintenance runs independently of native availability and workflow admission.
use automation_storage::{AutomationStore, StorageError};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct AutomationRetentionWorker {
    store: Arc<Mutex<AutomationStore>>,
}
impl AutomationRetentionWorker {
    pub(crate) fn new(store: Arc<Mutex<AutomationStore>>) -> Self {
        Self { store }
    }

    /// One bounded transaction; return the removed count so the runner can drain an old backlog.
    pub async fn prune_batch(&self, now_ms: i64) -> Result<u64, StorageError> {
        self.store
            .lock()
            .await
            .prune_automation_events(now_ms, 1000)
            .await
    }

    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { biased; _ = shutdown.cancelled() => return, _ = interval.tick() => {} }
            let now = chrono::Utc::now().timestamp_millis();
            loop {
                let result = tokio::select! { biased; _ = shutdown.cancelled() => return, result = self.prune_batch(now) => result };
                match result {
                    Ok(0) => break,
                    Ok(_) => tokio::task::yield_now().await,
                    Err(_) => {
                        tracing::warn!(
                            "automation event maintenance unavailable; current records remain intact"
                        );
                        break;
                    }
                }
            }
        }
    }
}
