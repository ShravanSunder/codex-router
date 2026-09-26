//! Host startup owns recovery; opening another database connection does not end live attempts.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    AttemptOutcome, DeliveryAttempt, DeliveryId, DeliveryStatus, EventId, SubmissionEffect,
};
use sqlx::{Connection, Row};

impl AutomationStore {
    /// Call only after establishing exclusive Host ownership, before starting dispatch workers.
    /// Dispatch intent is not proof of acceptance or rejection, so recovery never makes it retryable.
    pub async fn recover_interrupted_deliveries(
        &mut self,
        now_ms: i64,
    ) -> Result<Vec<DeliveryId>, StorageError> {
        if now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let rows = sqlx::query("SELECT delivery_id,latest_attempt_json,expires_at_ms FROM mailbox_deliveries WHERE delivery_status='dispatching' ORDER BY delivery_id")
            .fetch_all(&mut *transaction).await?;
        let mut recovered = Vec::with_capacity(rows.len());
        for row in rows {
            let id: DeliveryId = row
                .try_get::<String, _>("delivery_id")?
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?;
            let raw: String = row.try_get("latest_attempt_json")?;
            let mut attempt: DeliveryAttempt<serde_json::Value, serde_json::Value> =
                serde_json::from_str(&raw).map_err(|_| StorageError::InvalidRecord)?;
            if !matches!(attempt.outcome, AttemptOutcome::InProgress) {
                return Err(StorageError::InvalidRecord);
            }
            let expired = row
                .try_get::<Option<i64>, _>("expires_at_ms")?
                .is_some_and(|at| at <= now_ms);
            let (status, eligible_at_ms, reason) = match &mut attempt.effects {
                None => (
                    if expired || attempt.discard_on_non_submission {
                        DeliveryStatus::Discarded
                    } else {
                        DeliveryStatus::Retryable
                    },
                    now_ms
                        .checked_add(crate::delivery_outcomes::retry_delay_ms(
                            attempt.attempt_number,
                            &attempt.attempt_id,
                        ))
                        .ok_or(StorageError::InvalidRecord)?,
                    "Host stopped before a route was selected; no client call was dispatched.",
                ),
                Some(agent_automation::RouteEffectEvidence::CodexAppServer(native)) => {
                    native.submission = SubmissionEffect::Unknown;
                    (
                        DeliveryStatus::Uncertain,
                        now_ms,
                        "Host stopped before the native outcome was recorded; inspect and reconcile this exact attempt before any resend.",
                    )
                }
                Some(agent_automation::RouteEffectEvidence::ProviderAcp(provider)) => {
                    provider.submission = SubmissionEffect::Unknown;
                    (
                        DeliveryStatus::Uncertain,
                        now_ms,
                        "Host stopped before the provider outcome was recorded; inspect and reconcile this exact attempt before any resend.",
                    )
                }
                Some(agent_automation::RouteEffectEvidence::ClaudeCodePeer(peer)) => {
                    peer.write = agent_automation::PeerWriteEffect::Unknown;
                    (
                        DeliveryStatus::Uncertain,
                        now_ms,
                        "Host stopped before the peer write outcome was recorded; inspect this exact attempt before any resend.",
                    )
                }
            };
            attempt.outcome = if matches!(
                status,
                DeliveryStatus::Retryable | DeliveryStatus::Discarded
            ) {
                AttemptOutcome::KnownNotSubmitted {
                    reason: reason.into(),
                    retryable: true,
                }
            } else {
                AttemptOutcome::Unknown {
                    reason: reason.into(),
                }
            };
            attempt.completed_at_ms = Some(now_ms.max(attempt.started_at_ms));
            let encoded =
                serde_json::to_string(&attempt).map_err(|_| StorageError::InvalidRecord)?;
            let status_label = match status {
                DeliveryStatus::Retryable => "retryable",
                DeliveryStatus::Discarded => "discarded",
                DeliveryStatus::Uncertain => "uncertain",
                _ => return Err(StorageError::InvalidRecord),
            };
            sqlx::query("UPDATE mailbox_deliveries SET delivery_status=?,eligible_at_ms=?,latest_attempt_json=? WHERE delivery_id=?")
                .bind(status_label).bind(eligible_at_ms).bind(&encoded).bind(id.as_str()).execute(&mut *transaction).await?;
            if status == DeliveryStatus::Discarded {
                sqlx::query("UPDATE wakeup_definitions SET pending_delivery_id=NULL WHERE pending_delivery_id=?")
                    .bind(id.as_str()).execute(&mut *transaction).await?;
            }
            let event = serde_json::json!({"kind":"deliveryAttempt","attempt":attempt});
            sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'delivery',?,'dispatchInterrupted',?,?)")
                .bind(EventId::generate().as_str()).bind(id.as_str()).bind(event.to_string()).bind(now_ms).execute(&mut *transaction).await?;
            recovered.push(id);
        }
        transaction.commit().await?;
        Ok(recovered)
    }
}
