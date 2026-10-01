use automation_storage::{AutomationStore, StorageError};
use chrono::Utc;
use collaboration_protocol::{
    CodexGeneration, EndpointId, EndpointRef, MessageDelivery, PushHeaderFacts, PushId, PushKind,
    PushOrigin, PushRecordDraft, SessionId, SessionRef, UuidIdentity,
};
use serde_json::{Value, json};
use sqlx::{Connection, sqlite::SqliteConnectOptions};
use std::path::PathBuf;

type TestError = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), TestError>;

struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "router-direct-message-intent-{}.sqlite",
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

fn ensure(condition: bool, message: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn session(session_id: &str) -> Result<SessionRef, TestError> {
    Ok(SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?,
            endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
        },
        session_id: SessionId::try_from(session_id.to_owned())?,
    })
}

fn direct_message_draft(
    mode: MessageDelivery,
    guard: Option<CodexGeneration>,
) -> Result<PushRecordDraft, TestError> {
    let value = json!({
        "pushId":PushId::try_from(uuid::Uuid::now_v7().to_string())?,
        "kind":PushKind::DirectMessage,
        "origin":PushOrigin::Session(session("sender")?),
        "originRouterRef":null,
        "target":session("target")?,
        "mode":mode,
        "guard":guard,
        "replyToPushId":null,
        "headerFacts":PushHeaderFacts::DirectMessage { sender_display_name: None },
        "body":"persist this exact delivery intent",
        "activity":null,
        "createdAt":Utc::now(),
    });
    Ok(serde_json::from_value(value)?)
}

#[tokio::test]
async fn direct_message_delivery_intent_survives_store_and_reopen() -> TestResult {
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path).await?;
    let guard: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":3,
    }))?;
    let draft = direct_message_draft(MessageDelivery::Queue, Some(guard.clone()))?;
    let push_id = draft.push_id.clone();
    store.insert_push_record(draft).await?;
    store.close().await?;

    let mut reopened = AutomationStore::open(&database.path).await?;
    let record = reopened
        .get_push_record(&push_id)
        .await?
        .ok_or("stored DM missing after reopen")?;
    let encoded = serde_json::to_value(record)?;
    ensure(
        encoded["mode"] == json!("queue"),
        "queued intent was broadened",
    )?;
    ensure(
        encoded["guard"] == json!(guard),
        "generation guard was lost",
    )?;
    reopened.close().await?;
    Ok(())
}

#[tokio::test]
async fn corrupt_direct_message_mode_and_guard_fail_closed() -> TestResult {
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path).await?;
    let draft = direct_message_draft(MessageDelivery::Auto, None)?;
    let push_id = draft.push_id.clone();
    store.insert_push_record(draft).await?;
    let mut observer =
        sqlx::SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&database.path))
            .await?;
    for (mode, guard) in [
        ("invented", None),
        (
            "auto",
            Some(json!({"serviceEpoch":"invalid","generation":3})),
        ),
    ] {
        let guard_json = guard.map(|guard: Value| guard.to_string());
        sqlx::query(
            "UPDATE router_pushes SET dm_delivery_mode=?,dm_generation_guard_json=? WHERE push_id=?",
        )
        .bind(mode)
        .bind(guard_json)
        .bind(push_id.as_str())
        .execute(&mut observer)
        .await?;
        ensure(
            matches!(
                store.get_push_record(&push_id).await,
                Err(StorageError::InvalidRecord)
            ),
            "corrupt DM intent was accepted or coerced",
        )?;
    }
    observer.close().await?;
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn restart_discovers_every_unsent_dm_target_and_settles_only_attempted_dms() -> TestResult {
    use collaboration_protocol::{DeliveryOutcome, DeliveryReceipt, PushDeliveryState};
    let database = TestDatabase::new();
    let mut store = AutomationStore::open(&database.path).await?;
    let first = direct_message_draft(MessageDelivery::Auto, None)?;
    let first_id = first.push_id.clone();
    store.insert_push_record(first).await?;
    let mut second = direct_message_draft(MessageDelivery::Queue, None)?;
    second.target = session("another-target")?;
    let second_id = second.push_id.clone();
    store.insert_push_record(second).await?;
    store
        .hold_unsent_direct_message(
            &second_id,
            DeliveryReceipt {
                outcome: DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    reason: "known unsent".to_owned(),
                },
                reachability: None,
                client: None,
            },
        )
        .await?;
    let attempted = direct_message_draft(MessageDelivery::Steer, None)?;
    let attempted_id = attempted.push_id.clone();
    store.insert_push_record(attempted).await?;
    store.mark_push_attempted(&attempted_id).await?;
    let mut wake = direct_message_draft(MessageDelivery::Auto, None)?;
    wake.kind = PushKind::Wake;
    wake.origin = PushOrigin::Router(PushKind::Wake);
    wake.origin_router_ref = Some(
        collaboration_protocol::RouterOriginRef::Wake {
            wakeup_id: agent_automation::WakeupId::generate(),
            occurrence_id: agent_automation::OccurrenceId::generate(),
        }
        .canonical_string()?,
    );
    wake.mode = None;
    wake.guard = None;
    wake.header_facts = PushHeaderFacts::Wake;
    let wake_id = wake.push_id.clone();
    store.insert_push_record(wake).await?;
    store.mark_push_attempted(&wake_id).await?;
    store.close().await?;

    let mut restored = AutomationStore::open(&database.path).await?;
    ensure(
        restored
            .settle_interrupted_direct_messages(Utc::now())
            .await?
            == 1,
        "recovery touched the wrong attempts",
    )?;
    let targets = restored.direct_message_recovery_targets().await?;
    ensure(
        targets.len() == 2
            && targets.contains(&session("target")?)
            && targets.contains(&session("another-target")?),
        "recovery missed cross-target DM work",
    )?;
    let pending = restored
        .get_push_record(&first_id)
        .await?
        .ok_or("pending missing")?;
    let held = restored
        .get_push_record(&second_id)
        .await?
        .ok_or("held missing")?;
    let unknown = restored
        .get_push_record(&attempted_id)
        .await?
        .ok_or("attempt missing")?;
    let untouched = restored
        .get_push_record(&wake_id)
        .await?
        .ok_or("wake missing")?;
    ensure(
        pending.delivery_state == PushDeliveryState::Pending,
        "known unsent pending was settled",
    )?;
    ensure(
        held.delivery_state == PushDeliveryState::Held && held.mode == Some(MessageDelivery::Queue),
        "unsubmitted hold did not survive crash",
    )?;
    ensure(
        unknown.delivery_state == PushDeliveryState::OutcomeUnknown
            && unknown
                .last_outcome
                .is_some_and(|receipt| receipt.outcome == DeliveryOutcome::Unknown),
        "attempted DM was replayable",
    )?;
    ensure(
        untouched.delivery_state == PushDeliveryState::Attempted
            && untouched.last_outcome.is_none(),
        "DM recovery changed non-DM ownership",
    )?;
    ensure(
        restored
            .settle_interrupted_direct_messages(Utc::now())
            .await?
            == 0,
        "restore was not idempotent",
    )?;
    restored.close().await?;
    Ok(())
}

#[tokio::test]
async fn direct_message_intent_validation_rejects_missing_mode_and_non_dm_intent() -> TestResult {
    let mut missing = direct_message_draft(MessageDelivery::Auto, None)?;
    missing.mode = None;
    ensure(missing.into_pending().is_err(), "DM without mode accepted")?;
    let mut wake = direct_message_draft(MessageDelivery::Queue, None)?;
    wake.kind = PushKind::Wake;
    wake.origin = PushOrigin::Router(PushKind::Wake);
    wake.header_facts = PushHeaderFacts::Wake;
    ensure(wake.into_pending().is_err(), "non-DM mode accepted")?;
    Ok(())
}
