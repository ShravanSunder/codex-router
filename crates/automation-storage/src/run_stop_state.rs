//! Interruption intent belongs to the exact recorded turn; it never releases execution ownership.
use crate::{AutomationStore, StorageError};
use agent_automation::{AttemptId, ProviderSettlementEffect, RouteEffectEvidence, RunId, RunPhase};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunStopIdentity {
    NativeTurn(String),
    ProviderOperation(AttemptId),
}
impl AutomationStore {
    pub async fn begin_run_stop<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        id: &RunId,
        identity: RunStopIdentity,
    ) -> Result<bool, StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                id,
            )
            .await?;
        if record.evidence.route.is_none() {
            return Err(StorageError::InvalidRecord);
        }
        if !matches!(record.phase, RunPhase::Executing | RunPhase::Uncertain) {
            transaction.commit().await?;
            return Ok(false);
        }
        match (
            record
                .evidence
                .route
                .as_mut()
                .ok_or(StorageError::InvalidRecord)?,
            identity,
        ) {
            (RouteEffectEvidence::CodexAppServer(native), RunStopIdentity::NativeTurn(turn_id))
                if record.native_turn_id.as_deref() == Some(turn_id.as_str()) =>
            {
                native.cessation = agent_automation::CessationEvidence::Unconfirmed;
            }
            (
                RouteEffectEvidence::ProviderAcp(provider),
                RunStopIdentity::ProviderOperation(attempt_id),
            ) if provider.attempt_id == attempt_id
                && provider.submission == agent_automation::SubmissionEffect::Accepted =>
            {
                provider.settlement = ProviderSettlementEffect::StopRequested;
            }
            _ => {
                transaction.commit().await?;
                return Ok(false);
            }
        }
        sqlx::query("UPDATE workflow_runs SET run_status='stopping',execution_evidence_json=? WHERE run_id=?").bind(serde_json::to_string(&record.evidence).map_err(|_|StorageError::InvalidRecord)?).bind(id.as_str()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(true)
    }
}
