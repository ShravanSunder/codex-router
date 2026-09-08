//! Claim a native submission exactly once locally; unresolved effects are never silently replayed.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    AttemptId, AttemptOutcome, CessationEvidence, DeliveryAttempt, DeliveryId, DeliveryStatus,
    EventId, NativeEffectEvidence, PreparationEffect, SubmissionEffect, WakeState, WakeupId,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};

pub struct DeliveryClaim<TTarget, TContent, TGeneration> {
    pub delivery_id: DeliveryId,
    pub wakeup_id: WakeupId,
    pub attempt_id: AttemptId,
    pub target: TTarget,
    pub content: TContent,
    pub mode: String,
    pub generation_guard: Option<TGeneration>,
}
impl AutomationStore {
    pub async fn claim_delivery<TTarget, TContent, TGeneration>(
        &mut self,
        id: &DeliveryId,
        now_ms: i64,
    ) -> Result<Option<DeliveryClaim<TTarget, TContent, TGeneration>>, StorageError>
    where
        TTarget: Clone + Serialize + DeserializeOwned,
        TContent: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
    {
        if now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let row=sqlx::query("SELECT d.*,w.wakeup_status FROM mailbox_deliveries d JOIN wakeup_definitions w ON w.wakeup_id=d.wakeup_id WHERE d.delivery_id=?")
        .bind(id.as_str()).fetch_optional(&mut *transaction).await?.ok_or(StorageError::InvalidRecord)?;
        let status: DeliveryStatus =
            serde_json::from_value(serde_json::Value::String(row.try_get("delivery_status")?))
                .map_err(|_| StorageError::InvalidRecord)?;
        if !matches!(status, DeliveryStatus::Pending | DeliveryStatus::Retryable)
            || row.try_get::<i64, _>("eligible_at_ms")? > now_ms
        {
            transaction.commit().await?;
            return Ok(None);
        }
        let state: WakeState =
            serde_json::from_value(serde_json::Value::String(row.try_get("wakeup_status")?))
                .map_err(|_| StorageError::InvalidRecord)?;
        if state != WakeState::Active
            || row
                .try_get::<Option<i64>, _>("expires_at_ms")?
                .is_some_and(|at| at <= now_ms)
        {
            sqlx::query(
                "UPDATE mailbox_deliveries SET delivery_status='discarded' WHERE delivery_id=?",
            )
            .bind(id.as_str())
            .execute(&mut *transaction)
            .await?;
            sqlx::query("UPDATE wakeup_definitions SET pending_delivery_id=NULL WHERE pending_delivery_id=?").bind(id.as_str()).execute(&mut *transaction).await?;
            transaction.commit().await?;
            return Ok(None);
        }
        let wakeup_id = WakeupId::try_from(row.try_get::<String, _>("wakeup_id")?)
            .map_err(|_| StorageError::InvalidRecord)?;
        let target: TTarget = serde_json::from_str(&row.try_get::<String, _>("target_json")?)
            .map_err(|_| StorageError::InvalidRecord)?;
        let content: TContent = serde_json::from_str(&row.try_get::<String, _>("message_json")?)
            .map_err(|_| StorageError::InvalidRecord)?;
        let guard = row
            .try_get::<Option<String>, _>("generation_guard_json")?
            .map(|value| {
                serde_json::from_str::<TGeneration>(&value).map_err(|_| StorageError::InvalidRecord)
            })
            .transpose()?;
        let mode: String = row.try_get("delivery_mode")?;
        if !matches!(mode.as_str(), "auto" | "steer" | "queue") {
            return Err(StorageError::InvalidRecord);
        }
        let prior = row
            .try_get::<Option<String>, _>("latest_attempt_json")?
            .map(|value| {
                serde_json::from_str::<DeliveryAttempt<TTarget, TGeneration>>(&value)
                    .map_err(|_| StorageError::InvalidRecord)
            })
            .transpose()?;
        if let Some(prior) = &prior {
            if !matches!(
                prior.outcome,
                AttemptOutcome::KnownNotSubmitted {
                    retryable: true,
                    ..
                }
            ) {
                return Err(StorageError::InvalidRecord);
            }
        } else if status == DeliveryStatus::Retryable {
            return Err(StorageError::InvalidRecord);
        }
        let number = prior
            .as_ref()
            .map_or(Some(1), |attempt| attempt.attempt_number.checked_add(1))
            .ok_or(StorageError::InvalidRecord)?;
        if let Some(prior) = prior {
            let event = serde_json::json!({"kind":"deliveryAttempt","attempt":prior});
            sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'delivery',?,'attemptArchived',?,?)")
            .bind(EventId::generate().as_str()).bind(id.as_str()).bind(event.to_string()).bind(now_ms).execute(&mut *transaction).await?;
        }
        let attempt_id = AttemptId::generate();
        let attempt = DeliveryAttempt::<TTarget, TGeneration> {
            attempt_id: attempt_id.clone(),
            attempt_number: number,
            started_at_ms: now_ms,
            completed_at_ms: None,
            discard_on_non_submission: false,
            effects: NativeEffectEvidence {
                target: Some(target.clone()),
                generation: None,
                client_user_message_id: Some(id.as_str().to_owned()),
                native_turn_id: None,
                native_submission_id: None,
                allocation: PreparationEffect::NotRequested,
                resume: PreparationEffect::NotRequested,
                submission: SubmissionEffect::Dispatching,
                cessation: CessationEvidence::NotApplicable,
            },
            outcome: AttemptOutcome::InProgress,
        };
        let encoded = serde_json::to_string(&attempt).map_err(|_| StorageError::InvalidRecord)?;
        let affected=sqlx::query("UPDATE mailbox_deliveries SET delivery_status='dispatching',latest_attempt_json=? WHERE delivery_id=? AND delivery_status IN ('pending','retryable')")
        .bind(encoded).bind(id.as_str()).execute(&mut *transaction).await?.rows_affected();
        if affected != 1 {
            return Err(StorageError::InvalidRecord);
        }
        transaction.commit().await?;
        Ok(Some(DeliveryClaim {
            delivery_id: id.clone(),
            wakeup_id,
            attempt_id,
            target,
            content,
            mode,
            generation_guard: guard,
        }))
    }
}
