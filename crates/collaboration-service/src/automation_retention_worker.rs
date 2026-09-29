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

    /// Prunes events and reply records as separate bounded operations.
    /// A failure pruning reply records is logged and does not stop event maintenance.
    pub async fn prune_batch(&self, now_ms: i64) -> Result<u64, StorageError> {
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
            .ok_or(StorageError::InvalidRecord)?;
        let mut store = self.store.lock().await;
        let pruned_events = store.prune_automation_events(now_ms, 1000).await?;
        let pruned_senders = match store.prune_latest_agent_senders(now, 1000).await {
            Ok(count) => count,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "latest Agent sender maintenance unavailable; expired reply records remain until a later pass"
                );
                0
            }
        };
        pruned_events
            .checked_add(pruned_senders)
            .ok_or(StorageError::InvalidRecord)
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

    #[tokio::test]
    async fn existing_maintenance_pass_prunes_expired_reply_routes() {
        let path = std::env::temp_dir().join(format!(
            "automation-retention-reply-route-{}.sqlite",
            OperationId::generate().as_str()
        ));
        let mut opened = AutomationStore::open(&path)
            .await
            .expect("automation store should open");
        let recipient: collaboration_protocol::SessionRef = serde_json::from_value(
            serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
                "sessionId":"old-recipient"
            }),
        )
        .expect("recipient session");
        let sender: collaboration_protocol::SessionRef = serde_json::from_value(
            serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
                "sessionId":"old-sender"
            }),
        )
        .expect("sender session");
        let now = chrono::Utc::now();
        let old_record = automation_storage::LatestAgentSenderRecord::new(
            recipient.clone(),
            sender,
            now - chrono::Duration::days(31),
        )
        .expect("old sender record");
        opened
            .store_latest_agent_sender(&old_record)
            .await
            .expect("record reply route");
        let shared_store = Arc::new(Mutex::new(opened));
        let worker = AutomationRetentionWorker::new(Arc::clone(&shared_store));

        assert_eq!(
            worker
                .prune_batch(now.timestamp_millis())
                .await
                .expect("maintenance batch"),
            1
        );
        assert!(
            shared_store
                .lock()
                .await
                .latest_agent_sender_for(&recipient, now)
                .await
                .expect("latest sender query")
                .is_none()
        );

        drop(worker);
        Arc::try_unwrap(shared_store)
            .unwrap_or_else(|_| panic!("store must be released"))
            .into_inner()
            .close()
            .await
            .expect("close automation store");
        std::fs::remove_file(path).expect("remove isolated database");
    }

    #[tokio::test]
    async fn failed_reply_record_prune_does_not_stop_event_pruning() {
        let path = std::env::temp_dir().join(format!(
            "automation-retention-independent-prunes-{}.sqlite",
            OperationId::generate().as_str()
        ));
        let mut store = AutomationStore::open(&path)
            .await
            .expect("automation store should open");
        let recipient: collaboration_protocol::SessionRef = serde_json::from_value(
            serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
                "sessionId":"old-recipient"
            }),
        )
        .expect("recipient session");
        let sender: collaboration_protocol::SessionRef = serde_json::from_value(
            serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
                "sessionId":"old-sender"
            }),
        )
        .expect("sender session");
        let now = chrono::Utc::now();
        let old_record = automation_storage::LatestAgentSenderRecord::new(
            recipient.clone(),
            sender,
            now - chrono::Duration::days(31),
        )
        .expect("old sender record");
        store
            .store_latest_agent_sender(&old_record)
            .await
            .expect("reply route");
        let mut observer = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
        )
        .await
        .expect("observer connection");
        sqlx::query(
            "INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'instruction','fixture','fixture','{}',?)",
        )
        .bind(EventId::generate().as_str())
        .bind(now.timestamp_millis() - 90_i64 * 24 * 60 * 60 * 1000)
        .execute(&mut observer)
        .await
        .expect("expired event");
        sqlx::query(
            "CREATE TRIGGER fail_sender_prune BEFORE DELETE ON latest_agent_senders BEGIN SELECT RAISE(FAIL, 'sender prune failure'); END",
        )
        .execute(&mut observer)
        .await
        .expect("sender prune trigger");
        observer.close().await.expect("close observer");

        let shared_store = Arc::new(Mutex::new(store));
        let removed = AutomationRetentionWorker::new(Arc::clone(&shared_store))
            .prune_batch(now.timestamp_millis())
            .await
            .expect("event pruning should succeed independently");
        assert_eq!(removed, 1);
        let mut observer = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
        )
        .await
        .expect("event observer");
        let sender_count: i64 = sqlx::query_scalar("SELECT count(*) FROM latest_agent_senders")
            .fetch_one(&mut observer)
            .await
            .expect("query reply record count");
        assert_eq!(sender_count, 1, "failed reply pruning should leave its row");
        let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM automation_events")
            .fetch_one(&mut observer)
            .await
            .expect("query event count");
        assert_eq!(event_count, 0);
        observer.close().await.expect("close event observer");

        Arc::try_unwrap(shared_store)
            .unwrap_or_else(|_| panic!("store must be released"))
            .into_inner()
            .close()
            .await
            .expect("close store");
        std::fs::remove_file(path).expect("remove isolated database");
    }
}
