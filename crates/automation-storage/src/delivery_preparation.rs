//! Persist selected native identity and possible effects before crossing the external I/O boundary.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    AttemptId, AttemptOutcome, DeliveryAttempt, DeliveryId, PeerWriteEffect,
    ProviderSettlementEffect, RouteEffectEvidence, SubmissionEffect,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};
pub struct DeliveryPreparation<TTarget, TGeneration> {
    pub delivery_id: DeliveryId,
    pub attempt_id: AttemptId,
    pub effects: RouteEffectEvidence<TTarget, TGeneration>,
}
impl AutomationStore {
    pub async fn prepare_delivery<
        TTarget: Serialize + DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: DeliveryPreparation<TTarget, TGeneration>,
    ) -> Result<bool, StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let row=sqlx::query("SELECT latest_attempt_json FROM mailbox_deliveries WHERE delivery_id=? AND delivery_status='dispatching'").bind(request.delivery_id.as_str()).fetch_optional(&mut *transaction).await?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(false);
        };
        let raw: String = row.try_get("latest_attempt_json")?;
        let mut attempt: DeliveryAttempt<TTarget, TGeneration> =
            serde_json::from_str(&raw).map_err(|_| StorageError::InvalidRecord)?;
        if attempt.attempt_id != request.attempt_id
            || !matches!(attempt.outcome, AttemptOutcome::InProgress)
            || attempt.effects.is_some()
        {
            transaction.commit().await?;
            return Ok(false);
        }
        let dispatching = match &request.effects {
            RouteEffectEvidence::CodexAppServer(native) => {
                native.submission == SubmissionEffect::Dispatching
            }
            RouteEffectEvidence::ProviderAcp(provider) => {
                provider.attempt_id == request.attempt_id
                    && matches!(
                        provider.submission,
                        SubmissionEffect::Dispatching | SubmissionEffect::RouterQueued
                    )
                    && provider.settlement == ProviderSettlementEffect::NotObserved
            }
            RouteEffectEvidence::ClaudeCodePeer(peer) => peer.write == PeerWriteEffect::Dispatching,
        };
        if !dispatching {
            return Err(StorageError::InvalidRecord);
        }
        attempt.effects = Some(request.effects);
        let encoded = serde_json::to_string(&attempt).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("UPDATE mailbox_deliveries SET latest_attempt_json=? WHERE delivery_id=?")
            .bind(encoded)
            .bind(request.delivery_id.as_str())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(true)
    }
}
