//! Unresolved effects retain the schedule execution slot until exact observation establishes cessation.
use crate::{AutomationStore, StorageError};
use agent_automation::{NativeEffectEvidence, RunId, RunPhase};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
pub struct RunUncertainty<TTarget, TGeneration> {
    pub run_id: RunId,
    pub effects: NativeEffectEvidence<TTarget, TGeneration>,
}
impl AutomationStore {
    pub async fn retain_run_uncertainty<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunUncertainty<TTarget, TGeneration>,
    ) -> Result<(), StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        if !matches!(
            record.phase,
            RunPhase::Preparing | RunPhase::Executing | RunPhase::Stopping | RunPhase::Uncertain
        ) {
            return Err(StorageError::InvalidRecord);
        }
        record.evidence.native = request.effects;
        sqlx::query("UPDATE workflow_runs SET run_status='uncertain',execution_evidence_json=? WHERE run_id=?").bind(serde_json::to_string(&record.evidence).map_err(|_|StorageError::InvalidRecord)?).bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }
}
