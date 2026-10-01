#![allow(clippy::expect_used)]

use automation_storage::{AutomationStore, PushDeliveryState, PushRecordDraft, StorageError};
use chrono::{Duration, Utc};
use collaboration_protocol::{
    DeliveryClientReceipt, DeliveryOutcome, DeliveryReceipt, EndpointId, EndpointRef,
    PushHeaderFacts, PushId, PushKind, PushOrigin, SessionId, SessionReachability, SessionRef,
    UuidIdentity,
};
use sqlx::{Connection, sqlite::SqliteConnectOptions};
use std::path::PathBuf;

struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "automation-push-records-{}.sqlite",
                uuid::Uuid::now_v7()
            )),
        }
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[tokio::test]
async fn migration_adds_router_push_records_and_removes_latest_sender_table() {
    let database = TestDatabase::new();
    let store = AutomationStore::open(&database.path)
        .await
        .expect("automation store migrates");
    store.close().await.expect("close migrated store");

    let mut observer = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database.path)
            .foreign_keys(true),
    )
    .await
    .expect("open isolated schema observer");
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('router_pushes', 'latest_agent_senders') ORDER BY name",
    )
    .fetch_all(&mut observer)
    .await
    .expect("read migrated table inventory");

    assert_eq!(tables, ["router_pushes"]);
    observer.close().await.expect("close schema observer");
}

fn session(endpoint_id: &str, session_id: &str) -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned())
                .expect("service UUID"),
            endpoint_id: EndpointId::try_from(endpoint_id.to_owned()).expect("endpoint id"),
        },
        session_id: SessionId::try_from(session_id.to_owned()).expect("session id"),
    }
}

fn push_draft(
    push_id: PushId,
    target: SessionRef,
    origin: PushOrigin,
    body: &str,
    created_at: chrono::DateTime<Utc>,
) -> PushRecordDraft {
    PushRecordDraft {
        push_id,
        kind: PushKind::DirectMessage,
        origin,
        origin_router_ref: None,
        target,
        reply_to_push_id: None,
        header_facts: PushHeaderFacts::DirectMessage {
            sender_display_name: None,
        },
        body: Some(body.to_owned()),
        activity: None,
        created_at,
    }
}

fn push_id() -> PushId {
    PushId::try_from(uuid::Uuid::now_v7().to_string()).expect("UUIDv7 push id")
}

#[tokio::test]
async fn inserted_direct_message_is_a_store_first_immutable_snapshot() {
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path)
        .await
        .expect("automation store migrates");
    let sender = session("claude-local", "sender-thread");
    let target = session("codex-local", "target-thread");
    let push_id = push_id();
    let created_at = Utc::now() - Duration::seconds(1);
    let draft = push_draft(
        push_id.clone(),
        target.clone(),
        PushOrigin::Session(sender.clone()),
        "first snapshot",
        created_at,
    );

    let inserted = store
        .insert_push_record(draft.clone())
        .await
        .expect("insert before delivery");
    assert_eq!(inserted.delivery_state, PushDeliveryState::Pending);
    assert_eq!(inserted.body.as_deref(), Some("first snapshot"));
    assert_eq!(inserted.origin, PushOrigin::Session(sender.clone()));

    store
        .mark_push_attempted(&push_id)
        .await
        .expect("record the delivery attempt");
    let receipt = DeliveryReceipt {
        outcome: DeliveryOutcome::PeerMessageWritten,
        reachability: Some(SessionReachability::ClaudeCodePeer),
        client: Some(DeliveryClientReceipt::ClaudeCodePeer),
    };
    let settled = store
        .settle_push_record(&push_id, receipt, Utc::now())
        .await
        .expect("settle the accepted delivery");
    assert_eq!(settled.delivery_state, PushDeliveryState::Delivered);

    let conflicting_draft = push_draft(
        push_id.clone(),
        target,
        PushOrigin::Session(sender),
        "replacement text must not overwrite the snapshot",
        created_at,
    );
    assert!(matches!(
        store.insert_push_record(conflicting_draft).await,
        Err(StorageError::PushAlreadyExists)
    ));
    let read_back = store
        .get_push_record(&push_id)
        .await
        .expect("read stored push")
        .expect("push remains stored");
    assert_eq!(read_back.body.as_deref(), Some("first snapshot"));
    assert_eq!(read_back.delivery_state, PushDeliveryState::Delivered);
    store.close().await.expect("close automation store");
}

#[tokio::test]
async fn header_facts_round_trip_through_stored_json_uses_camel_case() {
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path)
        .await
        .expect("automation store migrates");
    let header_facts = PushHeaderFacts::DirectMessage {
        sender_display_name: None,
    };
    let push_id = push_id();
    let mut draft = push_draft(
        push_id.clone(),
        session("codex-local", "target-thread"),
        PushOrigin::Session(session("claude-local", "sender-thread")),
        "stored body",
        Utc::now(),
    );
    draft.header_facts = header_facts.clone();
    store
        .insert_push_record(draft)
        .await
        .expect("store push header facts");

    let mut observer = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database.path)
            .foreign_keys(true),
    )
    .await
    .expect("open isolated JSON observer");
    let stored_json: String =
        sqlx::query_scalar("SELECT header_facts_json FROM router_pushes WHERE push_id=?")
            .bind(push_id.as_str())
            .fetch_one(&mut observer)
            .await
            .expect("read serialized header facts");
    let stored_value: serde_json::Value =
        serde_json::from_str(&stored_json).expect("stored header facts JSON");
    assert_eq!(stored_value["kind"], "directMessage");
    assert_eq!(stored_value["senderDisplayName"], serde_json::Value::Null);
    assert!(stored_value.get("sender_display_name").is_none());

    let decoded = store
        .get_push_record(&push_id)
        .await
        .expect("decode stored push")
        .expect("stored push remains available");
    assert_eq!(decoded.header_facts, header_facts);
    observer
        .close()
        .await
        .expect("close isolated JSON observer");
    store.close().await.expect("close automation store");
}

#[tokio::test]
async fn prune_removes_only_push_records_strictly_older_than_thirty_days() {
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path)
        .await
        .expect("automation store migrates");
    let sender = session("claude-local", "sender-thread");
    let target = session("codex-local", "target-thread");
    let now = Utc::now();
    let cutoff = now - Duration::days(30);
    let expired_id = push_id();
    let boundary_id = push_id();
    let recent_id = push_id();
    for (id, timestamp, body) in [
        (
            expired_id.clone(),
            cutoff - Duration::nanoseconds(1),
            "expired",
        ),
        (boundary_id.clone(), cutoff, "boundary"),
        (recent_id.clone(), now - Duration::seconds(1), "recent"),
    ] {
        store
            .insert_push_record(push_draft(
                id,
                target.clone(),
                PushOrigin::Session(sender.clone()),
                body,
                timestamp,
            ))
            .await
            .expect("insert retention fixture");
    }

    assert_eq!(
        store
            .prune_push_records(now, 500)
            .await
            .expect("prune one bounded batch"),
        1
    );
    assert!(
        store
            .get_push_record(&expired_id)
            .await
            .expect("read expired")
            .is_none()
    );
    assert!(
        store
            .get_push_record(&boundary_id)
            .await
            .expect("read boundary")
            .is_some()
    );
    assert!(
        store
            .get_push_record(&recent_id)
            .await
            .expect("read recent")
            .is_some()
    );
    store.close().await.expect("close automation store");
}

#[tokio::test]
async fn unknown_delivery_outcome_is_settled_and_not_pending_for_replay() {
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path)
        .await
        .expect("automation store migrates");
    let sender = session("claude-local", "sender-thread");
    let target = session("codex-local", "target-thread");
    let push_id = push_id();
    store
        .insert_push_record(push_draft(
            push_id.clone(),
            target.clone(),
            PushOrigin::Session(sender),
            "reconcile once",
            Utc::now(),
        ))
        .await
        .expect("store before attempt");
    store
        .mark_push_attempted(&push_id)
        .await
        .expect("record attempted state");
    let settled = store
        .settle_push_record(
            &push_id,
            DeliveryReceipt {
                outcome: DeliveryOutcome::Unknown,
                reachability: None,
                client: None,
            },
            Utc::now(),
        )
        .await
        .expect("settle outcome unknown");

    assert_eq!(settled.delivery_state, PushDeliveryState::OutcomeUnknown);
    assert!(
        store
            .list_pending_push_records(&target)
            .await
            .expect("list pending")
            .is_empty()
    );
    assert!(
        store
            .list_held_push_records(&target)
            .await
            .expect("list held")
            .is_empty()
    );
    store.close().await.expect("close automation store");
}

#[tokio::test]
async fn direct_message_over_sixty_four_kib_is_rejected_before_persistence() {
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path)
        .await
        .expect("automation store migrates");
    let sender = session("claude-local", "sender-thread");
    let target = session("codex-local", "target-thread");
    let push_id = push_id();
    let draft = push_draft(
        push_id.clone(),
        target,
        PushOrigin::Session(sender),
        &"x".repeat(65_537),
        Utc::now(),
    );

    assert!(matches!(
        store.insert_push_record(draft).await,
        Err(StorageError::InvalidRecord)
    ));
    assert!(
        store
            .get_push_record(&push_id)
            .await
            .expect("check absent record")
            .is_none()
    );
    store.close().await.expect("close automation store");
}

#[tokio::test]
async fn insert_retries_sqlite_busy_only_within_the_five_second_window() {
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path)
        .await
        .expect("automation store migrates");
    let mut writer = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database.path)
            .create_if_missing(true)
            .foreign_keys(true),
    )
    .await
    .expect("open independent lock holder");
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut writer)
        .await
        .expect("hold SQLite writer lock");
    let sender = session("claude-local", "sender-thread");
    let target = session("codex-local", "target-thread");
    let push_id = push_id();
    let draft = push_draft(
        push_id.clone(),
        target,
        PushOrigin::Session(sender),
        "must not be submitted while persistence is busy",
        Utc::now(),
    );

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        store.insert_push_record(draft),
    )
    .await
    .expect("busy retries finish within the specified bound");
    assert!(matches!(result, Err(StorageError::Database(_))));
    sqlx::query("ROLLBACK")
        .execute(&mut writer)
        .await
        .expect("release SQLite writer lock");
    writer.close().await.expect("close lock holder");
    assert!(
        store
            .get_push_record(&push_id)
            .await
            .expect("read after busy")
            .is_none()
    );
    store.close().await.expect("close automation store");
}

async fn insert_mailbox_fixture(
    observer: &mut sqlx::SqliteConnection,
    wake_id: &str,
    delivery_id: &str,
    created_at_ms: i64,
    delivery_status: &str,
    pending: bool,
) -> Result<(), sqlx::Error> {
    let change_id = uuid::Uuid::now_v7().to_string();
    let occurrence_id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO wakeup_definitions (wakeup_id,change_id,wakeup_status,definition_json,anchor_at_ms,evaluated_through_ms,next_due_at_ms,expires_at_ms,first_fire_json,pending_delivery_id,created_at_ms,updated_at_ms) VALUES (?,?,'active','{}',?,?,NULL,NULL,NULL,NULL,?,?)",
    )
    .bind(wake_id)
    .bind(change_id)
    .bind(created_at_ms)
    .bind(created_at_ms)
    .bind(created_at_ms)
    .bind(created_at_ms)
    .execute(&mut *observer)
    .await?;
    sqlx::query(
        "INSERT INTO mailbox_deliveries (delivery_id,wakeup_id,occurrence_id,due_at_ms,fired_at_ms,target_json,message_json,delivery_mode,generation_guard_json,delivery_status,eligible_at_ms,expires_at_ms,latest_attempt_json,outcome_receipt_json,created_at_ms) VALUES (?,?,?, ?, ?, '{}','{}','queue',NULL,?,?,NULL,NULL,NULL,?)",
    )
    .bind(delivery_id)
    .bind(wake_id)
    .bind(occurrence_id)
    .bind(created_at_ms)
    .bind(created_at_ms)
    .bind(delivery_status)
    .bind(created_at_ms)
    .bind(created_at_ms)
    .execute(&mut *observer)
    .await?;
    if pending {
        sqlx::query("UPDATE wakeup_definitions SET pending_delivery_id=? WHERE wakeup_id=?")
            .bind(delivery_id)
            .bind(wake_id)
            .execute(&mut *observer)
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn thirty_day_cleanup_deletes_whole_settled_mailbox_and_operation_receipts() {
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path)
        .await
        .expect("automation store migrates");
    let mut observer = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database.path)
            .foreign_keys(true),
    )
    .await
    .expect("open isolated retention observer");
    let now_ms = 1_700_000_000_000_i64;
    let cutoff_ms = now_ms - 30 * 24 * 60 * 60 * 1000;
    let expired_ms = cutoff_ms - 1;
    let expired_delivery_id = uuid::Uuid::now_v7().to_string();
    let boundary_delivery_id = uuid::Uuid::now_v7().to_string();
    let recent_delivery_id = uuid::Uuid::now_v7().to_string();
    let pending_delivery_id = uuid::Uuid::now_v7().to_string();

    for (created_at_ms, status, pending, delivery_id) in [
        (expired_ms, "accepted", false, expired_delivery_id.as_str()),
        (cutoff_ms, "accepted", false, boundary_delivery_id.as_str()),
        (now_ms - 1, "accepted", false, recent_delivery_id.as_str()),
        (expired_ms, "pending", true, pending_delivery_id.as_str()),
    ] {
        let wake_id = uuid::Uuid::now_v7().to_string();
        insert_mailbox_fixture(
            &mut observer,
            &wake_id,
            delivery_id,
            created_at_ms,
            status,
            pending,
        )
        .await
        .expect("insert mailbox fixture");
    }

    let expired_receipt_id = uuid::Uuid::now_v7().to_string();
    let boundary_receipt_id = uuid::Uuid::now_v7().to_string();
    let recent_receipt_id = uuid::Uuid::now_v7().to_string();
    for (operation_id, committed_at_ms) in [
        (expired_receipt_id.as_str(), expired_ms),
        (boundary_receipt_id.as_str(), cutoff_ms),
        (recent_receipt_id.as_str(), now_ms - 1),
    ] {
        sqlx::query(
            "INSERT INTO operation_receipts (operation_id,method_name,canonical_request,resource_id,operation_status,effect_evidence_json,final_result_json,final_error_json,committed_at_ms) VALUES (?,'message/send',x'01','resource','succeeded','{}',NULL,NULL,?)",
        )
        .bind(operation_id)
        .bind(committed_at_ms)
        .execute(&mut observer)
        .await
        .expect("insert whole receipt fixture");
    }
    observer.close().await.expect("close retention observer");

    assert_eq!(
        store
            .prune_settled_mailbox_deliveries(now_ms, 500)
            .await
            .expect("prune settled mailbox rows"),
        1
    );
    assert_eq!(
        store
            .prune_operation_receipts(now_ms, 500)
            .await
            .expect("prune whole operation receipts"),
        1
    );
    let mut observer = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database.path)
            .foreign_keys(true),
    )
    .await
    .expect("reopen retention observer");
    for (delivery_id, expected_count) in [
        (expired_delivery_id.as_str(), 0_i64),
        (boundary_delivery_id.as_str(), 1),
        (recent_delivery_id.as_str(), 1),
        (pending_delivery_id.as_str(), 1),
    ] {
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM mailbox_deliveries WHERE delivery_id=?")
                .bind(delivery_id)
                .fetch_one(&mut observer)
                .await
                .expect("read mailbox retention result");
        assert_eq!(count, expected_count);
    }
    for (operation_id, expected_count) in [
        (expired_receipt_id.as_str(), 0_i64),
        (boundary_receipt_id.as_str(), 1),
        (recent_receipt_id.as_str(), 1),
    ] {
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM operation_receipts WHERE operation_id=?")
                .bind(operation_id)
                .fetch_one(&mut observer)
                .await
                .expect("read operation retention result");
        assert_eq!(count, expected_count);
    }
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut observer)
            .await
            .expect("check retained foreign keys")
            .is_empty()
    );
    observer.close().await.expect("close retention observer");
    store.close().await.expect("close automation store");
}
