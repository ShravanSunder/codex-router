//! Retention across real SQLite payload rows and the broker's file-backed history.
use super::AutomationRetentionWorker;
use crate::{
    InteractionHistoryRecord, NativeControlBackend, NativeGenerationGate, RefusedApprovalOption,
    RefusedTypedApproval, ServiceInteractionBroker,
};
use agent_automation::{
    ChangeId, DeliveryId, FirstFire, OccurrenceId, TimingRule, WakeDefinition, WakeupId,
};
use automation_storage::AutomationStore;
use chrono::{DateTime, SecondsFormat, Utc};
use collaboration_protocol::{
    EndpointId, EndpointRef, MessageContent, MessageDelivery, MessageText, PushHeaderFacts, PushId,
    PushKind, PushOrigin, PushRecordDraft, SavedMessage, SessionId, SessionRef, UuidIdentity,
};
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokio::sync::Mutex;

type TestResult<TValue = ()> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;
const SERVICE_ID: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";
const RETENTION_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;
const LEGACY_REQUEST: &str = "legacy-interaction";

fn ensure(condition: bool, detail: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(detail.into())
    }
}

fn session(session_id: &str) -> TestResult<SessionRef> {
    Ok(SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from(SERVICE_ID.to_owned())?,
            endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
        },
        session_id: SessionId::try_from(session_id.to_owned())?,
    })
}

fn history_value(
    request_id: &str,
    body: &str,
    created_at: Option<DateTime<Utc>>,
) -> TestResult<Value> {
    let requester = serde_json::from_value(serde_json::to_value(session("requester")?)?)?;
    let approver = serde_json::from_value(json!({"kind":"human", "humanId":"retention-owner"}))?;
    let record = InteractionHistoryRecord::RefusedApproval {
        requester,
        approver,
        refusal: RefusedTypedApproval {
            request_id: request_id.to_owned(),
            title: body.to_owned(),
            description: Some(body.to_owned()),
            subject: None,
            options: vec![RefusedApprovalOption {
                option_id: "reject".to_owned(),
                label: "Reject".to_owned(),
                provider_kind: "fixture".to_owned(),
            }],
            reason: "fixture refusal".to_owned(),
        },
    };
    let mut value = serde_json::to_value(record)?;
    if let Some(created_at) = created_at {
        value
            .as_object_mut()
            .ok_or("history record must encode an object")?
            .insert(
                "createdAt".to_owned(),
                Value::String(created_at.to_rfc3339_opts(SecondsFormat::Nanos, true)),
            );
    }
    Ok(value)
}

async fn read_history(path: &Path) -> TestResult<BTreeMap<String, Value>> {
    Ok(serde_json::from_slice(&tokio::fs::read(path).await?)?)
}

fn legacy_stamp(history: &BTreeMap<String, Value>) -> TestResult<String> {
    Ok(history
        .get(LEGACY_REQUEST)
        .and_then(|record| record.get("createdAt"))
        .and_then(Value::as_str)
        .ok_or("legacy history must contain its persisted first-load stamp")?
        .to_owned())
}

async fn load_broker(
    directory: &Path,
    gate: &NativeGenerationGate,
) -> TestResult<Arc<ServiceInteractionBroker>> {
    ensure(
        gate.acquire().is_err(),
        "retention fixture native gate must stay inactive",
    )?;
    Ok(ServiceInteractionBroker::load(
        UuidIdentity::try_from(SERVICE_ID.to_owned())?,
        NativeControlBackend {
            endpoint: session("unused-backend")?.endpoint,
            gate: gate.clone(),
            codex_home: directory.join("unused-codex-home"),
        },
        directory.join("approval-routes.json"),
    )
    .await?)
}

struct StoredContent {
    push_id: PushId,
    push_body: String,
    mailbox_id: DeliveryId,
    mailbox_json: String,
}

async fn insert_content(
    store: &mut AutomationStore,
    observer: &mut SqliteConnection,
    label: &str,
    created_at_ms: i64,
) -> TestResult<StoredContent> {
    let created_at =
        DateTime::<Utc>::from_timestamp_millis(created_at_ms).ok_or("valid fixture timestamp")?;
    let push_id = PushId::try_from(uuid::Uuid::now_v7().to_string())?;
    let push_body = format!("{label}-push-body");
    let target = session("retention-target")?;
    store
        .insert_push_record(PushRecordDraft {
            push_id: push_id.clone(),
            kind: PushKind::DirectMessage,
            origin: PushOrigin::Session(session("retention-sender")?),
            origin_router_ref: None,
            target: target.clone(),
            mode: Some(MessageDelivery::Auto),
            guard: None,
            reply_to_push_id: None,
            header_facts: PushHeaderFacts::DirectMessage {
                sender_display_name: None,
            },
            body: Some(push_body.clone()),
            activity: None,
            created_at,
        })
        .await?;
    let wakeup_id = WakeupId::generate();
    let change_id = ChangeId::generate();
    let mailbox_id = DeliveryId::generate();
    let occurrence_id = OccurrenceId::generate();
    let content = MessageContent::HumanUser {
        text: MessageText::try_from(format!("{label}-mailbox-body"))?,
    };
    let mailbox_json = serde_json::to_string(&content)?;
    let definition = WakeDefinition {
        wakeup_id: wakeup_id.clone(),
        change_id: change_id.clone(),
        message: SavedMessage {
            target: target.clone(),
            content,
            delivery: MessageDelivery::Queue,
            generation_guard: None,
        },
        timing: TimingRule::At { at: created_at },
        anchor_at_ms: created_at_ms,
        expires_at_ms: None,
        created_at_ms,
    };
    let definition_json = serde_json::to_string(&definition)?;
    let first_fire_json = serde_json::to_string(&FirstFire {
        wakeup_id: wakeup_id.clone(),
        occurrence_id: occurrence_id.clone(),
        due_at_ms: created_at_ms,
        fired_at_ms: created_at_ms,
    })?;
    let target_json = serde_json::to_string(&target)?;
    sqlx::query("INSERT INTO wakeup_definitions (wakeup_id,change_id,wakeup_status,definition_json,anchor_at_ms,evaluated_through_ms,next_due_at_ms,expires_at_ms,first_fire_json,pending_delivery_id,created_at_ms,updated_at_ms) VALUES (?,?,'finished',?,?,?,NULL,NULL,?,NULL,?,?)")
        .bind(wakeup_id.as_str()).bind(change_id.as_str()).bind(definition_json)
        .bind(created_at_ms).bind(created_at_ms).bind(first_fire_json).bind(created_at_ms).bind(created_at_ms)
        .execute(&mut *observer).await?;
    sqlx::query("INSERT INTO mailbox_deliveries (delivery_id,wakeup_id,occurrence_id,due_at_ms,fired_at_ms,target_json,message_json,delivery_mode,generation_guard_json,delivery_status,eligible_at_ms,expires_at_ms,latest_attempt_json,outcome_receipt_json,created_at_ms) VALUES (?,?,?,?,?,?,?,'queue',NULL,'accepted',?,NULL,NULL,NULL,?)")
        .bind(mailbox_id.as_str()).bind(wakeup_id.as_str()).bind(occurrence_id.as_str())
        .bind(created_at_ms).bind(created_at_ms).bind(target_json).bind(&mailbox_json)
        .bind(created_at_ms).bind(created_at_ms).execute(&mut *observer).await?;
    Ok(StoredContent {
        push_id,
        push_body,
        mailbox_id,
        mailbox_json,
    })
}

async fn ensure_physical_contents(
    observer: &mut SqliteConnection,
    expected: &[&StoredContent],
) -> TestResult {
    let pushes: Vec<(String, String)> =
        sqlx::query_as("SELECT push_id,body FROM router_pushes ORDER BY push_id")
            .fetch_all(&mut *observer)
            .await?;
    let mailboxes: Vec<(String, String)> = sqlx::query_as(
        "SELECT delivery_id,message_json FROM mailbox_deliveries ORDER BY delivery_id",
    )
    .fetch_all(&mut *observer)
    .await?;
    let mut expected_pushes = expected
        .iter()
        .map(|record| (record.push_id.as_str().to_owned(), record.push_body.clone()))
        .collect::<Vec<_>>();
    let mut expected_mailboxes = expected
        .iter()
        .map(|record| {
            (
                record.mailbox_id.as_str().to_owned(),
                record.mailbox_json.clone(),
            )
        })
        .collect::<Vec<_>>();
    expected_pushes.sort();
    expected_mailboxes.sort();
    ensure(
        pushes == expected_pushes,
        "retention must remove whole push rows and preserve retained bodies",
    )?;
    ensure(
        mailboxes == expected_mailboxes,
        "retention must remove whole settled mailbox rows and preserve retained bodies",
    )
}

async fn ensure_history_contents(
    broker: &ServiceInteractionBroker,
    path: &Path,
    expected: &[&str],
) -> TestResult {
    let mut memory_ids = broker
        .list_interactions()
        .await
        .iter()
        .map(|record| record.request_id().to_owned())
        .collect::<Vec<_>>();
    let disk = read_history(path).await?;
    let disk_ids = disk.keys().cloned().collect::<Vec<_>>();
    let mut expected_ids = expected
        .iter()
        .map(|id| (*id).to_owned())
        .collect::<Vec<_>>();
    memory_ids.sort();
    expected_ids.sort();
    ensure(
        memory_ids == expected_ids,
        "worker must prune broker's actual in-memory interaction history",
    )?;
    ensure(
        disk_ids == expected_ids,
        "worker must remove expired history records and bodies from real JSON",
    )
}

#[tokio::test]
async fn retention_worker_prunes_real_broker_history_and_sqlite_payloads_with_one_persisted_legacy_stamp()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let history_path = directory.path().join("interaction-history.json");
    let legacy = BTreeMap::from([(
        LEGACY_REQUEST.to_owned(),
        history_value(LEGACY_REQUEST, "legacy-history-body", None)?,
    )]);
    tokio::fs::write(directory.path().join("approval-routes.json"), b"[]").await?;
    tokio::fs::write(directory.path().join("approval-history.json"), b"[]").await?;
    tokio::fs::write(&history_path, serde_json::to_vec_pretty(&legacy)?).await?;
    let gate = NativeGenerationGate::default();
    let first_broker = load_broker(directory.path(), &gate).await?;
    let mut persisted = read_history(&history_path).await?;
    let persisted_stamp = legacy_stamp(&persisted)?;
    let stamped_at = DateTime::parse_from_rfc3339(&persisted_stamp)?.with_timezone(&Utc);
    ensure_history_contents(&first_broker, &history_path, &[LEGACY_REQUEST]).await?;
    drop(first_broker);

    // The worker accepts milliseconds; the observed nanosecond stamp remains
    // on or after this floored cutoff and before the following millisecond.
    let cutoff_ms = stamped_at.timestamp_millis();
    let cutoff = DateTime::<Utc>::from_timestamp_millis(cutoff_ms).ok_or("valid cutoff")?;
    let expired =
        DateTime::<Utc>::from_timestamp_millis(cutoff_ms - 1).ok_or("valid expired time")?;
    let recent =
        DateTime::<Utc>::from_timestamp_millis(cutoff_ms + 1).ok_or("valid recent time")?;
    for (id, body, created_at) in [
        ("expired-interaction", "expired-history-body", expired),
        ("cutoff-interaction", "cutoff-history-body", cutoff),
        ("recent-interaction", "recent-history-body", recent),
    ] {
        persisted.insert(id.to_owned(), history_value(id, body, Some(created_at))?);
    }
    tokio::fs::write(&history_path, serde_json::to_vec_pretty(&persisted)?).await?;
    let broker = load_broker(directory.path(), &gate).await?;
    ensure(
        legacy_stamp(&read_history(&history_path).await?)? == persisted_stamp,
        "broker reload must preserve the original legacy stamp",
    )?;

    let database_path = directory.path().join("automation.sqlite");
    let mut store = AutomationStore::open(&database_path).await?;
    let mut observer = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database_path)
            .foreign_keys(true),
    )
    .await?;
    let expired_content =
        insert_content(&mut store, &mut observer, "expired", cutoff_ms - 1).await?;
    let cutoff_content = insert_content(&mut store, &mut observer, "cutoff", cutoff_ms).await?;
    let recent_content = insert_content(&mut store, &mut observer, "recent", cutoff_ms + 1).await?;
    ensure_physical_contents(
        &mut observer,
        &[&expired_content, &cutoff_content, &recent_content],
    )
    .await?;
    let shared_store = Arc::new(Mutex::new(store));
    let worker = AutomationRetentionWorker::new(shared_store.clone())
        .with_interaction_broker(broker.clone());
    let first_maintenance_ms = cutoff_ms + RETENTION_MILLIS;

    ensure(
        worker.prune_batch(first_maintenance_ms).await? == 3,
        "worker must prune the expired push, mailbox row and interaction independently",
    )?;
    ensure_physical_contents(&mut observer, &[&cutoff_content, &recent_content]).await?;
    ensure_history_contents(
        &broker,
        &history_path,
        &["cutoff-interaction", LEGACY_REQUEST, "recent-interaction"],
    )
    .await?;
    ensure(
        legacy_stamp(&read_history(&history_path).await?)? == persisted_stamp,
        "retention must preserve the retained legacy stamp",
    )?;
    ensure(
        !String::from_utf8(tokio::fs::read(&history_path).await?)?.contains("expired-history-body"),
        "expired interaction body must be physically absent",
    )?;

    ensure(
        worker.prune_batch(first_maintenance_ms + 1).await? == 4,
        "legacy stamp must age normally after thirty days, without reload restamping",
    )?;
    ensure_physical_contents(&mut observer, &[&recent_content]).await?;
    ensure_history_contents(&broker, &history_path, &["recent-interaction"]).await?;
    ensure(
        !String::from_utf8(tokio::fs::read(&history_path).await?)?.contains("legacy-history-body"),
        "aged legacy interaction body must be physically absent",
    )?;
    ensure(
        gate.acquire().is_err(),
        "retention must not activate a native generation",
    )?;
    let retained_definitions: i64 = sqlx::query_scalar("SELECT count(*) FROM wakeup_definitions")
        .fetch_one(&mut observer)
        .await?;
    ensure(
        retained_definitions == 3,
        "payload retention must not remove reusable wake definitions",
    )?;

    drop(worker);
    drop(broker);
    observer.close().await?;
    Arc::try_unwrap(shared_store)
        .map_err(|_| "retention worker must release its store")?
        .into_inner()
        .close()
        .await?;
    Ok(())
}
