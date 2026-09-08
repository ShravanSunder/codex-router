//! Pause/cancel discard only undispatched input; resume retains the original timer definition.
use crate::local_operation_receipts::{self, CompletedLocalOperation, LocalOperation};
use crate::wakeup_evaluation::{WakeEvent, append_event, discard_undispatched};
use crate::{AutomationStore, StorageError};
use agent_automation::{DeliveryId, DeliveryStatus, OperationId, WakeRecord, WakeState, WakeupId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WakeAction {
    Pause,
    Resume,
    Cancel,
}
pub struct WakeMutation {
    pub operation_id: OperationId,
    pub wakeup_id: WakeupId,
    pub action: WakeAction,
    pub now_ms: i64,
}
/// Storage-row snapshot. Service adapters validate JSON fields into their native receipt types.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetainedDelivery {
    pub delivery_id: DeliveryId,
    pub status: DeliveryStatus,
    pub latest_attempt_json: Option<String>,
    pub accepted_receipt_json: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WakeMutationResult<TMessage> {
    pub wake: WakeRecord<TMessage>,
    pub discarded: Vec<DeliveryId>,
    pub retained: Vec<RetainedDelivery>,
}

impl AutomationStore {
    pub async fn mutate_wakeup<TMessage: Serialize + DeserializeOwned>(
        &mut self,
        request: &WakeMutation,
    ) -> Result<WakeMutationResult<TMessage>, StorageError> {
        if request.now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let method = match request.action {
            WakeAction::Pause => "wake/pause",
            WakeAction::Resume => "wake/resume",
            WakeAction::Cancel => "wake/cancel",
        };
        let canonical =
            serde_json::to_vec(&request.wakeup_id).map_err(|_| StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(result) = local_operation_receipts::replay(
            &mut transaction,
            LocalOperation {
                id: &request.operation_id,
                method,
                canonical: &canonical,
            },
        )
        .await?
        {
            transaction.commit().await?;
            return Ok(result);
        }
        let current: WakeRecord<TMessage> =
            crate::wakeup_repository::read_current(&mut transaction, &request.wakeup_id).await?;
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(request.now_ms)
            .ok_or(StorageError::InvalidRecord)?;
        let anchor =
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(current.definition.anchor_at_ms)
                .ok_or(StorageError::InvalidRecord)?;
        let (state, next, kind) = if matches!(current.state, WakeState::Active | WakeState::Paused)
            && current
                .definition
                .expires_at_ms
                .is_some_and(|at| at <= request.now_ms)
        {
            (WakeState::Expired, None, Some("expired"))
        } else {
            match (request.action, current.state) {
                (WakeAction::Pause, WakeState::Active) => (WakeState::Paused, None, Some("paused")),
                (WakeAction::Pause, WakeState::Paused)
                | (WakeAction::Resume, WakeState::Active) => {
                    (current.state, current.next_due_at_ms, None)
                }
                (WakeAction::Cancel, WakeState::Active | WakeState::Paused) => {
                    (WakeState::Cancelled, None, Some("cancelled"))
                }
                (WakeAction::Cancel, _) => (current.state, current.next_due_at_ms, None),
                (WakeAction::Resume, WakeState::Paused) => {
                    let calculated = current
                        .definition
                        .timing
                        .next_due(anchor, Some(now))?
                        .map(|at| at.timestamp_millis());
                    if calculated.is_none() {
                        (
                            WakeState::Finished,
                            None,
                            Some(if current.first_fire.is_none() {
                                "finishedWithoutFiring"
                            } else {
                                "finished"
                            }),
                        )
                    } else {
                        (
                            WakeState::Active,
                            calculated.filter(|at| {
                                current
                                    .definition
                                    .expires_at_ms
                                    .is_none_or(|expiry| *at < expiry)
                            }),
                            Some("resumed"),
                        )
                    }
                }
                _ => {
                    return Err(StorageError::WakeLifecycleConflict {
                        state: current.state,
                    });
                }
            }
        };
        let mut discarded = Vec::new();
        if matches!(
            state,
            WakeState::Paused | WakeState::Cancelled | WakeState::Expired
        ) {
            let ids:Vec<String>=sqlx::query_scalar("SELECT delivery_id FROM mailbox_deliveries WHERE wakeup_id=? AND delivery_status IN ('pending','retryable')").bind(request.wakeup_id.as_str()).fetch_all(&mut *transaction).await?;
            for id in ids {
                discarded.push(id.try_into().map_err(|_| StorageError::InvalidRecord)?);
            }
            discard_undispatched(&mut transaction, &request.wakeup_id).await?;
        }
        let mut retained = Vec::new();
        if let Some(id) = &current.pending_delivery_id {
            let row=sqlx::query("SELECT delivery_status,latest_attempt_json,accepted_receipt_json FROM mailbox_deliveries WHERE delivery_id=? AND wakeup_id=?").bind(id.as_str()).bind(request.wakeup_id.as_str()).fetch_one(&mut *transaction).await?;
            let status: DeliveryStatus =
                serde_json::from_value(serde_json::Value::String(row.try_get("delivery_status")?))
                    .map_err(|_| StorageError::InvalidRecord)?;
            if status.may_have_native_effect() {
                retained.push(RetainedDelivery {
                    delivery_id: id.clone(),
                    status,
                    latest_attempt_json: row.try_get("latest_attempt_json")?,
                    accepted_receipt_json: row.try_get("accepted_receipt_json")?,
                });
            }
        }
        if let Some(kind) = kind {
            let status = match state {
                WakeState::Active => "active",
                WakeState::Paused => "paused",
                WakeState::Cancelled => "cancelled",
                WakeState::Expired => "expired",
                WakeState::Finished => "finished",
            };
            sqlx::query("UPDATE wakeup_definitions SET wakeup_status=?,next_due_at_ms=?,evaluated_through_ms=MAX(evaluated_through_ms,?),updated_at_ms=? WHERE wakeup_id=?")
            .bind(status).bind(next).bind(request.now_ms).bind(request.now_ms).bind(request.wakeup_id.as_str()).execute(&mut *transaction).await?;
            append_event(
                &mut transaction,
                WakeEvent {
                    id: &request.wakeup_id,
                    kind,
                    body: &serde_json::json!({"kind":kind}),
                    now_ms: request.now_ms,
                },
            )
            .await?;
        }
        let wake =
            crate::wakeup_repository::read_current(&mut transaction, &request.wakeup_id).await?;
        let result = WakeMutationResult {
            wake,
            discarded,
            retained,
        };
        let encoded = serde_json::to_string(&result).map_err(|_| StorageError::InvalidRecord)?;
        local_operation_receipts::record(
            &mut transaction,
            CompletedLocalOperation {
                operation: LocalOperation {
                    id: &request.operation_id,
                    method,
                    canonical: &canonical,
                },
                resource_id: request.wakeup_id.as_str(),
                result_json: &encoded,
                now_ms: request.now_ms,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(result)
    }
}
