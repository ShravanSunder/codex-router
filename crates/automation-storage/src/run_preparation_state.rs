//! Run-owned allocation evidence prevents a restart from silently creating another execution thread.
use crate::{AutomationStore, StorageError, ThreadBindingClaim};
use agent_automation::{NativeEffectEvidence, RunId, RunPhase};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
pub struct RunPreparationIntent<TTarget, TGeneration> {
    pub run_id: RunId,
    pub effects: NativeEffectEvidence<TTarget, TGeneration>,
}
pub struct RunPreparedTarget<TTarget, TGeneration> {
    pub run_id: RunId,
    pub effects: NativeEffectEvidence<TTarget, TGeneration>,
    pub binding: ThreadBindingClaim,
}
impl AutomationStore {
    pub async fn begin_run_preparation<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunPreparationIntent<TTarget, TGeneration>,
    ) -> Result<bool, StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        if record.phase != RunPhase::Preparing
            || record.evidence.timing.is_some()
            || record.evidence.native.target.is_some()
            || matches!(
                record.evidence.native.allocation,
                agent_automation::PreparationEffect::Unknown
                    | agent_automation::PreparationEffect::Accepted
            )
        {
            transaction.commit().await?;
            return Ok(false);
        }
        record.evidence.native = request.effects;
        sqlx::query("UPDATE workflow_runs SET execution_evidence_json=? WHERE run_id=?")
            .bind(serde_json::to_string(&record.evidence).map_err(|_| StorageError::InvalidRecord)?)
            .bind(request.run_id.as_str())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(true)
    }
    pub async fn record_run_target<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunPreparedTarget<TTarget, TGeneration>,
    ) -> Result<bool, StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        if !matches!(record.phase, RunPhase::Preparing | RunPhase::Uncertain)
            || record.evidence.timing.is_some()
            || record.schedule_id != request.binding.schedule_id
            || request.effects.target.is_none()
        {
            transaction.commit().await?;
            return Ok(false);
        }
        let binding = crate::thread_binding_repository::claim_in_transaction(
            &mut transaction,
            &request.binding,
        )
        .await?;
        record.evidence.native = request.effects;
        sqlx::query("UPDATE workflow_runs SET run_status='preparing',thread_binding_id=?,execution_evidence_json=? WHERE run_id=?").bind(binding.as_str()).bind(serde_json::to_string(&record.evidence).map_err(|_|StorageError::InvalidRecord)?).bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(true)
    }
}
