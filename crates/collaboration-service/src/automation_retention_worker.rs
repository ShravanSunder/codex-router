//! Bounded event maintenance runs independently of native availability and workflow admission.
use automation_storage::{AutomationStore, StorageError};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct AutomationRetentionWorker {
    store: Arc<Mutex<AutomationStore>>,
    interaction_broker: Option<Arc<crate::ServiceInteractionBroker>>,
}
impl AutomationRetentionWorker {
    pub(crate) fn new(store: Arc<Mutex<AutomationStore>>) -> Self {
        Self {
            store,
            interaction_broker: None,
        }
    }

    pub(crate) fn with_interaction_broker(
        mut self,
        broker: Arc<crate::ServiceInteractionBroker>,
    ) -> Self {
        self.interaction_broker = Some(broker);
        self
    }

    /// Runs every retained-content prune independently in bounded batches.
    pub async fn prune_batch(&self, now_ms: i64) -> Result<u64, StorageError> {
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
            .ok_or(StorageError::InvalidRecord)?;
        let mut pruned_total = 0_u64;
        {
            let mut store = self.store.lock().await;
            for (name, result) in [
                ("automation events", store.prune_automation_events(now_ms, 1000).await),
                ("push records", store.prune_push_records(now, 500).await),
                (
                    "settled mailbox deliveries",
                    store.prune_settled_mailbox_deliveries(now_ms, 500).await,
                ),
                (
                    "operation receipts",
                    store.prune_operation_receipts(now_ms, 500).await,
                ),
            ] {
                match result {
                    Ok(count) => add_pruned_total(&mut pruned_total, count)?,
                    Err(error) => tracing::warn!(error = %error, prune = name, "retention prune failed; the next pass will retry"),
                }
            }
        }
        if let Some(broker) = self.interaction_broker.as_ref() {
            match broker.prune_interaction_history(now, 500).await {
                Ok(count) => add_pruned_total(&mut pruned_total, count)?,
                Err(error) => tracing::warn!(error = %error, "interaction-history retention prune failed; the next pass will retry"),
            }
        }
        Ok(pruned_total)
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
                            "automation retention maintenance unavailable; records remain intact"
                        );
                        break;
                    }
                }
            }
        }
    }
}

fn add_pruned_total(total: &mut u64, count: u64) -> Result<(), StorageError> {
    *total = total.checked_add(count).ok_or(StorageError::InvalidRecord)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use agent_automation::{EventId, InstructionText, OperationId};
    use sqlx::Connection;
    use tokio::sync::Mutex;

    use super::AutomationRetentionWorker;
    use automation_storage::{AutomationStore, PushRecordDraft};
    use collaboration_protocol::{
        EndpointId, EndpointRef, PushHeaderFacts, PushId, PushKind, PushOrigin, SessionId,
        SessionRef, UuidIdentity,
    };
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

    fn session(endpoint_id: &str, session_id: &str) -> SessionRef {
        SessionRef {
            endpoint: EndpointRef {
                service_id: UuidIdentity::try_from(
                    "018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned(),
                )
                .expect("service id"),
                endpoint_id: EndpointId::try_from(endpoint_id.to_owned())
                    .expect("endpoint id"),
            },
            session_id: SessionId::try_from(session_id.to_owned()).expect("session id"),
        }
    }

    fn expired_push(now_ms: i64) -> PushRecordDraft {
        let sender = session("claude-local", "sender");
        PushRecordDraft {
            push_id: PushId::try_from(uuid::Uuid::now_v7().to_string())
                .expect("UUIDv7 push id"),
            kind: PushKind::DirectMessage,
            origin: PushOrigin::Session(sender),
            origin_router_ref: None,
            target: session("codex-local", "target"),
            reply_to_push_id: None,
            header_facts: PushHeaderFacts::DirectMessage {
                sender_display_name: None,
            },
            body: Some("expired push body".to_owned()),
            activity: None,
            created_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                now_ms - 31_i64 * 24 * 60 * 60 * 1000,
            )
            .expect("valid old timestamp"),
        }
    }

    #[tokio::test]
    async fn retention_worker_prunes_expired_push_records() {
        let path = std::env::temp_dir().join(format!(
            "automation-retention-push-{}.sqlite",
            OperationId::generate().as_str()
        ));
        let mut store = AutomationStore::open(&path)
            .await
            .expect("automation store should open");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let draft = expired_push(now_ms);
        let push_id = draft.push_id.clone();
        store
            .insert_push_record(draft)
            .await
            .expect("insert expired push");
        let shared_store = Arc::new(Mutex::new(store));

        assert_eq!(
            AutomationRetentionWorker::new(Arc::clone(&shared_store))
                .prune_batch(now_ms)
                .await
                .expect("run bounded retention pass"),
            1
        );
        assert!(shared_store
            .lock()
            .await
            .get_push_record(&push_id)
            .await
            .expect("read pruned push")
            .is_none());
        Arc::try_unwrap(shared_store)
            .unwrap_or_else(|_| panic!("store should be released"))
            .into_inner()
            .close()
            .await
            .expect("close automation store");
        std::fs::remove_file(path).expect("remove isolated database");
    }

    #[tokio::test]
    async fn failed_push_prune_does_not_stop_expired_event_pruning() {
        let path = std::env::temp_dir().join(format!(
            "automation-retention-independent-prunes-{}.sqlite",
            OperationId::generate().as_str()
        ));
        let mut store = AutomationStore::open(&path)
            .await
            .expect("automation store should open");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let draft = expired_push(now_ms);
        let push_id = draft.push_id.clone();
        store
            .insert_push_record(draft)
            .await
            .expect("insert expired push");
        let mut observer = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
        )
        .await
        .expect("observer connection");
        sqlx::query(
            "INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'run','fixture','fixture','{}',?)",
        )
        .bind(EventId::generate().as_str())
        .bind(now_ms - 90_i64 * 24 * 60 * 60 * 1000)
        .execute(&mut observer)
        .await
        .expect("insert expired event");
        sqlx::query(
            "CREATE TRIGGER fail_push_prune BEFORE DELETE ON router_pushes BEGIN SELECT RAISE(FAIL, 'push prune failure'); END",
        )
        .execute(&mut observer)
        .await
        .expect("install push prune failure trigger");
        observer.close().await.expect("close observer");

        let shared_store = Arc::new(Mutex::new(store));
        let removed = AutomationRetentionWorker::new(Arc::clone(&shared_store))
            .prune_batch(now_ms)
            .await
            .expect("independent event prune succeeds");
        assert_eq!(removed, 1);
        assert!(shared_store
            .lock()
            .await
            .get_push_record(&push_id)
            .await
            .expect("push still exists after its failed prune")
            .is_some());
        let mut observer = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
        )
        .await
        .expect("event observer");
        let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM automation_events")
            .fetch_one(&mut observer)
            .await
            .expect("read event count");
        assert_eq!(event_count, 0);
        observer.close().await.expect("close event observer");
        Arc::try_unwrap(shared_store)
            .unwrap_or_else(|_| panic!("store should be released"))
            .into_inner()
            .close()
            .await
            .expect("close automation store");
        std::fs::remove_file(path).expect("remove isolated database");
    }

}
