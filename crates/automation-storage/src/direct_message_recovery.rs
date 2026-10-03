//! Recover interrupted push attempts and restore known-unsent direct messages.
use crate::{AutomationStore, StorageError, push_record_rows};
use chrono::{DateTime, Utc};
use collaboration_protocol::{
    DeliveryOutcome, DeliveryReceipt, EndpointId, EndpointRef, MessageDelivery, PushId, PushKind,
    PushRecord, SessionId, SessionRef, UuidIdentity,
};
use sqlx::FromRow;

#[derive(FromRow)]
struct DirectMessageTargetRow {
    target_service_id: String,
    target_endpoint_id: String,
    target_session_id: String,
}

impl AutomationStore {
    /// Finds reader owners that must resume known-unsent direct messages.
    pub async fn direct_message_recovery_targets(
        &mut self,
    ) -> Result<Vec<SessionRef>, StorageError> {
        let kind = push_record_rows::serialize_kind(PushKind::DirectMessage)?;
        let rows = sqlx::query_as!(
            DirectMessageTargetRow,
            "SELECT DISTINCT target_service_id,target_endpoint_id,target_session_id FROM router_pushes WHERE kind=? AND delivery_state IN ('pending','held') ORDER BY target_service_id,target_endpoint_id,target_session_id",
            kind
        )
        .fetch_all(&mut self.connection)
        .await?;
        let mut targets = Vec::with_capacity(rows.len());
        for row in rows {
            match direct_message_recovery_target(row) {
                Ok(target) => targets.push(target),
                Err(field) => {
                    tracing::warn!(
                        record_field = field,
                        "direct-message recovery target has invalid stored field"
                    );
                }
            }
        }
        Ok(targets)
    }

    pub async fn list_unsent_direct_messages(
        &mut self,
        target: &SessionRef,
    ) -> Result<Vec<PushRecord>, StorageError> {
        let kind = push_record_rows::serialize_kind(PushKind::DirectMessage)?;
        let (service_id, endpoint_id, session_id) = (
            String::from(target.endpoint.service_id.clone()),
            String::from(target.endpoint.endpoint_id.clone()),
            String::from(target.session_id.clone()),
        );
        let rows = sqlx::query_as!(
            push_record_rows::PushRecordRow,
            "SELECT push_id,kind,origin_kind,origin_service_id,origin_endpoint_id,origin_session_id,origin_router_ref,target_service_id,target_endpoint_id,target_session_id,reply_to_push_id,header_facts_json,body,ranges_json,delivery_state,last_outcome_json,created_at,settled_at,read_at,dm_delivery_mode,dm_generation_guard_json FROM router_pushes WHERE kind=? AND target_service_id=? AND target_endpoint_id=? AND target_session_id=? AND delivery_state IN ('pending','held') ORDER BY created_at,push_id",
            kind, service_id, endpoint_id, session_id
        ).fetch_all(&mut self.connection).await?;
        rows.into_iter()
            .map(push_record_rows::PushRecordRow::into_record)
            .collect()
    }

    /// The process died after an attempt began. Its effect is unknowable; never resend it.
    pub async fn settle_interrupted_pushes(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<u64, StorageError> {
        let receipt = DeliveryReceipt {
            outcome: DeliveryOutcome::Unknown,
            reachability: None,
            client: None,
        };
        let outcome_json = push_record_rows::serialize_json(&receipt)?;
        let settled_at = push_record_rows::serialize_timestamp(now);
        let direct_message = push_record_rows::serialize_kind(PushKind::DirectMessage)?;
        let subscription_activity =
            push_record_rows::serialize_kind(PushKind::SubscriptionActivity)?;
        let subscription_expiry = push_record_rows::serialize_kind(PushKind::SubscriptionExpiry)?;
        let approval = push_record_rows::serialize_kind(PushKind::Approval)?;
        let question = push_record_rows::serialize_kind(PushKind::Question)?;
        let result = sqlx::query!(
            "UPDATE router_pushes SET delivery_state='outcome_unknown',last_outcome_json=?,settled_at=? WHERE delivery_state='attempted' AND last_outcome_json IS NULL AND kind IN (?, ?, ?, ?, ?)",
            outcome_json, settled_at, direct_message, subscription_activity, subscription_expiry,
            approval, question
        ).execute(&mut self.connection).await?;
        Ok(result.rows_affected())
    }

    /// A presence check made no submission. Keep that distinction durable across a crash.
    pub async fn hold_unsent_direct_message(
        &mut self,
        push_id: &PushId,
        receipt: DeliveryReceipt,
    ) -> Result<PushRecord, StorageError> {
        let record = self
            .get_push_record(push_id)
            .await?
            .ok_or(StorageError::PushNotFound)?;
        if record.kind != PushKind::DirectMessage
            || !matches!(
                record.mode,
                Some(MessageDelivery::Auto | MessageDelivery::Queue)
            )
            || record.guard.is_some()
            || !matches!(
                receipt.outcome,
                DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    ..
                }
            )
        {
            return Err(StorageError::InvalidRecord);
        }
        let kind = push_record_rows::serialize_kind(PushKind::DirectMessage)?;
        let outcome_json = push_record_rows::serialize_json(&receipt)?;
        let result = sqlx::query!(
            "UPDATE router_pushes SET delivery_state='held',last_outcome_json=?,settled_at=NULL WHERE push_id=? AND kind=? AND delivery_state IN ('pending','held')",
            outcome_json, push_id.as_str(), kind
        ).execute(&mut self.connection).await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::PushStateConflict);
        }
        self.get_push_record(push_id)
            .await?
            .ok_or(StorageError::PushNotFound)
    }

    /// Reject known-unsent work without inventing an attempted effect.
    pub async fn reject_unsent_direct_message(
        &mut self,
        push_id: &PushId,
        receipt: DeliveryReceipt,
        now: DateTime<Utc>,
    ) -> Result<PushRecord, StorageError> {
        if !matches!(
            receipt.outcome,
            DeliveryOutcome::Rejected(_) | DeliveryOutcome::NotSubmitted { .. }
        ) {
            return Err(StorageError::InvalidRecord);
        }
        let kind = push_record_rows::serialize_kind(PushKind::DirectMessage)?;
        let outcome_json = push_record_rows::serialize_json(&receipt)?;
        let settled_at = push_record_rows::serialize_timestamp(now);
        let result = sqlx::query!(
            "UPDATE router_pushes SET delivery_state='rejected',last_outcome_json=?,settled_at=? WHERE push_id=? AND kind=? AND delivery_state IN ('pending','held')",
            outcome_json, settled_at, push_id.as_str(), kind
        ).execute(&mut self.connection).await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::PushStateConflict);
        }
        self.get_push_record(push_id)
            .await?
            .ok_or(StorageError::PushNotFound)
    }
}

fn direct_message_recovery_target(row: DirectMessageTargetRow) -> Result<SessionRef, &'static str> {
    let service_id =
        UuidIdentity::try_from(row.target_service_id).map_err(|_| "target_service_id")?;
    let endpoint_id =
        EndpointId::try_from(row.target_endpoint_id).map_err(|_| "target_endpoint_id")?;
    let session_id = SessionId::try_from(row.target_session_id).map_err(|_| "target_session_id")?;
    Ok(SessionRef {
        endpoint: EndpointRef {
            service_id,
            endpoint_id,
        },
        session_id,
    })
}
