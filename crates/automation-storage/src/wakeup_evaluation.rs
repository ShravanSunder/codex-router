//! A firing records eligibility and a durable delivery; it does not claim native acceptance.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    DeliveryId, DurableMessage, EventId, FirstFire, OccurrenceId, WakeRecord, WakeState, WakeupId,
};
use serde::de::DeserializeOwned;
use sqlx::{Connection, Row, SqliteConnection};

pub enum WakeEvaluation<TMessage> {
    Unchanged(WakeRecord<TMessage>),
    Fired {
        wake: WakeRecord<TMessage>,
        fire: FirstFire,
        delivery_id: DeliveryId,
    },
    Coalesced {
        wake: WakeRecord<TMessage>,
        delivery_id: DeliveryId,
    },
    Expired(WakeRecord<TMessage>),
}
impl AutomationStore {
    pub async fn evaluate_wakeup<TMessage: DurableMessage + DeserializeOwned>(
        &mut self,
        id: &WakeupId,
        now_ms: i64,
    ) -> Result<WakeEvaluation<TMessage>, StorageError> {
        if now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
            .ok_or(StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let current: WakeRecord<TMessage> =
            crate::wakeup_repository::read_current(&mut transaction, id).await?;
        if matches!(
            current.state,
            WakeState::Cancelled | WakeState::Expired | WakeState::Finished
        ) {
            transaction.commit().await?;
            return Ok(WakeEvaluation::Unchanged(current));
        }
        if current
            .definition
            .expires_at_ms
            .is_some_and(|at| at <= now_ms)
        {
            discard_undispatched(&mut transaction, id).await?;
            sqlx::query("UPDATE wakeup_definitions SET wakeup_status='expired',next_due_at_ms=NULL,updated_at_ms=? WHERE wakeup_id=?").bind(now_ms).bind(id.as_str()).execute(&mut *transaction).await?;
            append_event(
                &mut transaction,
                WakeEvent {
                    id,
                    kind: "expired",
                    body: &serde_json::json!({"kind":"expired"}),
                    now_ms,
                },
            )
            .await?;
            let wake = crate::wakeup_repository::read_current(&mut transaction, id).await?;
            transaction.commit().await?;
            return Ok(WakeEvaluation::Expired(wake));
        }
        let due = current.next_due_at_ms.filter(|at| *at <= now_ms);
        if current.state != WakeState::Active || due.is_none() {
            transaction.commit().await?;
            return Ok(WakeEvaluation::Unchanged(current));
        }
        let due = due.ok_or(StorageError::InvalidRecord)?;
        let anchor =
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(current.definition.anchor_at_ms)
                .ok_or(StorageError::InvalidRecord)?;
        let next = current
            .definition
            .timing
            .next_due(anchor, Some(now))?
            .map(|at| at.timestamp_millis())
            .filter(|at| {
                current
                    .definition
                    .expires_at_ms
                    .is_none_or(|expiry| *at < expiry)
            });
        if let Some(pending) = &current.pending_delivery_id {
            let status:String=sqlx::query_scalar("SELECT delivery_status FROM mailbox_deliveries WHERE delivery_id=? AND wakeup_id=?").bind(pending.as_str()).bind(id.as_str()).fetch_one(&mut *transaction).await?;
            match status.as_str() {
                "pending" | "retryable" | "dispatching" | "uncertain" => {
                    advance(&mut transaction, TimingAdvance { id, next, now_ms }).await?;
                    append_event(&mut transaction,WakeEvent{id,kind:"coalesced",body:&serde_json::json!({"kind":"coalesced","deliveryId":pending,"throughMs":now_ms}),now_ms}).await?;
                    let wake = crate::wakeup_repository::read_current(&mut transaction, id).await?;
                    let delivery_id = pending.clone();
                    transaction.commit().await?;
                    return Ok(WakeEvaluation::Coalesced { wake, delivery_id });
                }
                "accepted" | "discarded" | "failed" => {}
                _ => return Err(StorageError::InvalidRecord),
            }
        }
        let fire = FirstFire {
            wakeup_id: id.clone(),
            occurrence_id: OccurrenceId::generate(),
            due_at_ms: due,
            fired_at_ms: now_ms,
        };
        let delivery_id = DeliveryId::generate();
        let target = serde_json::to_string(current.definition.message.target())
            .map_err(|_| StorageError::InvalidRecord)?;
        let message = serde_json::to_string(current.definition.message.content())
            .map_err(|_| StorageError::InvalidRecord)?;
        let guard = current
            .definition
            .message
            .generation_guard()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("INSERT INTO mailbox_deliveries(delivery_id,wakeup_id,occurrence_id,due_at_ms,fired_at_ms,target_json,message_json,delivery_mode,generation_guard_json,delivery_status,eligible_at_ms,expires_at_ms,latest_attempt_json,accepted_receipt_json,created_at_ms) VALUES (?,?,?,?,?,?,?,?,?,'pending',?,?,NULL,NULL,?)")
        .bind(delivery_id.as_str()).bind(id.as_str()).bind(fire.occurrence_id.as_str()).bind(due).bind(now_ms).bind(target).bind(message).bind(current.definition.message.delivery_mode()).bind(guard).bind(now_ms).bind(current.definition.expires_at_ms).bind(now_ms).execute(&mut *transaction).await?;
        let first = serde_json::to_string(current.first_fire.as_ref().unwrap_or(&fire))
            .map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("UPDATE wakeup_definitions SET first_fire_json=?,pending_delivery_id=? WHERE wakeup_id=?").bind(first).bind(delivery_id.as_str()).bind(id.as_str()).execute(&mut *transaction).await?;
        advance(&mut transaction, TimingAdvance { id, next, now_ms }).await?;
        append_event(
            &mut transaction,
            WakeEvent {
                id,
                kind: "fired",
                body: &serde_json::json!({"kind":"fired","fire":fire}),
                now_ms,
            },
        )
        .await?;
        let wake = crate::wakeup_repository::read_current(&mut transaction, id).await?;
        transaction.commit().await?;
        Ok(WakeEvaluation::Fired {
            wake,
            fire,
            delivery_id,
        })
    }
}
struct TimingAdvance<'a> {
    id: &'a WakeupId,
    next: Option<i64>,
    now_ms: i64,
}
async fn advance(
    connection: &mut SqliteConnection,
    input: TimingAdvance<'_>,
) -> Result<(), StorageError> {
    let TimingAdvance { id, next, now_ms } = input;
    sqlx::query(
        "UPDATE wakeup_definitions SET evaluated_through_ms=?,next_due_at_ms=? WHERE wakeup_id=?",
    )
    .bind(now_ms)
    .bind(next)
    .bind(id.as_str())
    .execute(connection)
    .await?;
    Ok(())
}
pub(crate) async fn discard_undispatched(
    connection: &mut SqliteConnection,
    id: &WakeupId,
) -> Result<(), StorageError> {
    // Preserve native evidence while recording that a later known rejection must not retry.
    // Typed decoding validates the attempt; native address/generation remain adapter-owned JSON.
    let unresolved = sqlx::query("SELECT delivery_id,latest_attempt_json FROM mailbox_deliveries WHERE wakeup_id=? AND delivery_status IN ('dispatching','uncertain')")
        .bind(id.as_str()).fetch_all(&mut *connection).await?;
    for row in unresolved {
        let raw: String = row.try_get("latest_attempt_json")?;
        let mut attempt: agent_automation::DeliveryAttempt<serde_json::Value, serde_json::Value> =
            serde_json::from_str(&raw).map_err(|_| StorageError::InvalidRecord)?;
        attempt.discard_on_non_submission = true;
        let encoded = serde_json::to_string(&attempt).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("UPDATE mailbox_deliveries SET latest_attempt_json=? WHERE delivery_id=?")
            .bind(encoded)
            .bind(row.try_get::<String, _>("delivery_id")?)
            .execute(&mut *connection)
            .await?;
    }
    sqlx::query("UPDATE mailbox_deliveries SET delivery_status='discarded' WHERE wakeup_id=? AND delivery_status IN ('pending','retryable')").bind(id.as_str()).execute(&mut *connection).await?;
    sqlx::query("UPDATE wakeup_definitions SET pending_delivery_id=NULL WHERE wakeup_id=? AND pending_delivery_id IN (SELECT delivery_id FROM mailbox_deliveries WHERE wakeup_id=? AND delivery_status IN ('discarded','accepted','failed'))").bind(id.as_str()).bind(id.as_str()).execute(connection).await?;
    Ok(())
}
pub(crate) struct WakeEvent<'a> {
    pub id: &'a WakeupId,
    pub kind: &'a str,
    pub body: &'a serde_json::Value,
    pub now_ms: i64,
}
pub(crate) async fn append_event(
    connection: &mut SqliteConnection,
    event: WakeEvent<'_>,
) -> Result<(), StorageError> {
    let WakeEvent {
        id,
        kind,
        body,
        now_ms,
    } = event;
    sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'wake',?,?,?,?)").bind(EventId::generate().as_str()).bind(id.as_str()).bind(kind).bind(body.to_string()).bind(now_ms).execute(connection).await?;
    Ok(())
}
