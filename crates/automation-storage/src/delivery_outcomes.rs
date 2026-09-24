//! Final attempt evidence controls eligibility; an unknown outcome is never a retry signal.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    AcceptedDeliveryEffect, AttemptId, AttemptOutcome, DeliveryAttempt, DeliveryId, EventId,
    RouteEffectEvidence,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};
use std::hash::{Hash, Hasher};

pub enum DeliveryResult<TReceipt> {
    Accepted {
        effect: AcceptedDeliveryEffect,
        receipt: TReceipt,
    },
    KnownNotSubmitted {
        reason: String,
        retryable: bool,
        receipt: Option<TReceipt>,
    },
    Unknown {
        reason: String,
        receipt: Option<TReceipt>,
    },
}
pub struct DeliveryCompletion<TTarget, TGeneration, TReceipt> {
    pub delivery_id: DeliveryId,
    pub attempt_id: AttemptId,
    pub effects: Option<RouteEffectEvidence<TTarget, TGeneration>>,
    pub result: DeliveryResult<TReceipt>,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn complete_delivery<TTarget, TGeneration, TReceipt>(
        &mut self,
        completion: DeliveryCompletion<TTarget, TGeneration, TReceipt>,
    ) -> Result<bool, StorageError>
    where
        TTarget: PartialEq + Serialize + DeserializeOwned,
        TGeneration: PartialEq + Serialize + DeserializeOwned,
        TReceipt: Serialize,
    {
        if completion.now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let row=sqlx::query("SELECT delivery_status,latest_attempt_json,expires_at_ms FROM mailbox_deliveries WHERE delivery_id=?").bind(completion.delivery_id.as_str()).fetch_optional(&mut *transaction).await?.ok_or(StorageError::InvalidRecord)?;
        let status: String = row.try_get("delivery_status")?;
        if !matches!(status.as_str(), "dispatching" | "uncertain") {
            transaction.commit().await?;
            return Ok(false);
        }
        let raw = row
            .try_get::<Option<String>, _>("latest_attempt_json")?
            .ok_or(StorageError::InvalidRecord)?;
        let mut attempt: DeliveryAttempt<TTarget, TGeneration> =
            serde_json::from_str(&raw).map_err(|_| StorageError::InvalidRecord)?;
        if attempt.attempt_id != completion.attempt_id {
            transaction.commit().await?;
            return Ok(false);
        }
        let evidence_result = match &completion.result {
            DeliveryResult::Accepted { effect, .. } => {
                crate::delivery_effect_transition::DeliveryEvidenceResult::Accepted(*effect)
            }
            DeliveryResult::KnownNotSubmitted { .. } => {
                crate::delivery_effect_transition::DeliveryEvidenceResult::KnownNotSubmitted
            }
            DeliveryResult::Unknown { .. } => {
                crate::delivery_effect_transition::DeliveryEvidenceResult::Unknown
            }
        };
        if !crate::delivery_effect_transition::allows_completion(
            attempt.effects.as_ref(),
            completion.effects.as_ref(),
            evidence_result,
        ) {
            return Err(StorageError::InvalidRecord);
        }
        let now_ms = completion.now_ms.max(attempt.started_at_ms);
        let (next_status, outcome, receipt, next_eligible) = match completion.result {
            DeliveryResult::Accepted { receipt, .. } => (
                "accepted",
                AttemptOutcome::Accepted,
                Some(serde_json::to_string(&receipt).map_err(|_| StorageError::InvalidRecord)?),
                now_ms,
            ),
            DeliveryResult::KnownNotSubmitted {
                reason,
                retryable,
                receipt,
            } => {
                let next = if retryable {
                    now_ms
                        .checked_add(retry_delay_ms(attempt.attempt_number, &attempt.attempt_id))
                        .ok_or(StorageError::InvalidRecord)?
                } else {
                    now_ms
                };
                let expired = row
                    .try_get::<Option<i64>, _>("expires_at_ms")?
                    .is_some_and(|at| at <= now_ms);
                (
                    if expired || attempt.discard_on_non_submission {
                        "discarded"
                    } else if retryable {
                        "retryable"
                    } else {
                        "failed"
                    },
                    AttemptOutcome::KnownNotSubmitted { reason, retryable },
                    receipt
                        .map(|value| {
                            serde_json::to_string(&value).map_err(|_| StorageError::InvalidRecord)
                        })
                        .transpose()?,
                    next,
                )
            }
            DeliveryResult::Unknown { reason, receipt } => (
                "uncertain",
                AttemptOutcome::Unknown { reason },
                receipt
                    .map(|value| {
                        serde_json::to_string(&value).map_err(|_| StorageError::InvalidRecord)
                    })
                    .transpose()?,
                now_ms,
            ),
        };
        attempt.effects = completion.effects;
        attempt.outcome = outcome;
        attempt.completed_at_ms = Some(now_ms);
        let encoded = serde_json::to_string(&attempt).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("UPDATE mailbox_deliveries SET delivery_status=?,latest_attempt_json=?,outcome_receipt_json=?,eligible_at_ms=? WHERE delivery_id=?")
        .bind(next_status).bind(&encoded).bind(receipt.as_deref()).bind(next_eligible).bind(completion.delivery_id.as_str()).execute(&mut *transaction).await?;
        if matches!(next_status, "accepted" | "failed" | "discarded") {
            sqlx::query("UPDATE wakeup_definitions SET pending_delivery_id=NULL WHERE pending_delivery_id=?").bind(completion.delivery_id.as_str()).execute(&mut *transaction).await?;
            sqlx::query("UPDATE wakeup_definitions SET wakeup_status='finished' WHERE wakeup_id=(SELECT wakeup_id FROM mailbox_deliveries WHERE delivery_id=?) AND wakeup_status='active' AND next_due_at_ms IS NULL AND pending_delivery_id IS NULL AND first_fire_json IS NOT NULL")
            .bind(completion.delivery_id.as_str()).execute(&mut *transaction).await?;
        }
        let event_receipt = receipt
            .as_deref()
            .map(serde_json::from_str::<serde_json::Value>)
            .transpose()
            .map_err(|_| StorageError::InvalidRecord)?;
        let event =
            serde_json::json!({"kind":"deliveryAttempt","attempt":attempt,"receipt":event_receipt});
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'delivery',?,'attemptCompleted',?,?)")
        .bind(EventId::generate().as_str()).bind(completion.delivery_id.as_str()).bind(event.to_string()).bind(now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(true)
    }
}
pub(crate) fn retry_delay_ms(number: u32, id: &AttemptId) -> i64 {
    let exponent = number.saturating_sub(1).min(6);
    let base = (1000_i64 << exponent).min(60_000);
    let headroom = (60_000 - base).min(base / 5);
    if headroom == 0 {
        return base;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    let spread = u64::try_from(headroom).unwrap_or(0) + 1;
    base + i64::try_from(hasher.finish() % spread).unwrap_or(0)
}
