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

#[cfg(test)]
mod tests {
    use agent_automation::{EventId, InstructionText, OperationId};
    use sqlx::Connection;
    use tokio::sync::Mutex;

    use super::AutomationRetentionWorker;
    use automation_storage::AutomationStore;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn minute_worker_prunes_expired_events_without_touching_current_or_instruction_rows() {
        let path = std::env::temp_dir().join(format!(
            "automation-retention-worker-{}.sqlite",
            OperationId::generate().as_str()
        ));
        let mut store = AutomationStore::open(&path)
            .await
            .unwrap_or_else(|error| panic!("automation store should open: {error}"));
        let instruction = store
            .create_instruction(
                &OperationId::generate(),
                &InstructionText::try_from("retained instruction".to_owned())
                    .unwrap_or_else(|error| panic!("instruction text should validate: {error}")),
                0,
            )
            .await
            .unwrap_or_else(|error| panic!("instruction should persist: {error}"));
        let now = chrono::Utc::now().timestamp_millis();
        let expired = now - 90_i64 * 24 * 60 * 60 * 1000;
        let mut observer = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .unwrap_or_else(|error| panic!("observer should connect: {error}"));
        for timestamp in [expired, now] {
            sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'instruction',?,'fixture','{}',?)")
                .bind(EventId::generate().as_str())
                .bind(instruction.instruction_id.as_str())
                .bind(timestamp)
                .execute(&mut observer)
                .await
                .unwrap_or_else(|error| panic!("fixture event should persist: {error}"));
        }

        let shared_store = Arc::new(Mutex::new(store));
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(
            AutomationRetentionWorker::new(Arc::clone(&shared_store)).run(shutdown.clone()),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let retained: Vec<i64> = sqlx::query_scalar(
                    "SELECT recorded_at_ms FROM automation_events ORDER BY recorded_at_ms",
                )
                .fetch_all(&mut observer)
                .await
                .unwrap_or_else(|error| panic!("worker result should query: {error}"));
                if retained == vec![now] {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("minute worker should prune the expired event"));
        assert_eq!(
            shared_store
                .lock()
                .await
                .read_instruction(&instruction.instruction_id)
                .await
                .unwrap_or_else(|error| panic!("instruction should remain readable: {error}")),
            instruction
        );

        shutdown.cancel();
        task.await
            .unwrap_or_else(|error| panic!("worker should join: {error}"));
        observer
            .close()
            .await
            .unwrap_or_else(|error| panic!("observer should close: {error}"));
        Arc::try_unwrap(shared_store)
            .unwrap_or_else(|_| panic!("worker should release the store reference"))
            .into_inner()
            .close()
            .await
            .unwrap_or_else(|error| panic!("store should close: {error}"));
        std::fs::remove_file(path)
            .unwrap_or_else(|error| panic!("fixture database should remove: {error}"));
    }
}
