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
pub struct RunPreparationFailure<TTarget, TGeneration> {
    pub run_id: RunId,
    pub effects: NativeEffectEvidence<TTarget, TGeneration>,
    pub explanation: String,
    pub now_ms: i64,
}
impl AutomationStore {
    /// A known allocation/preparation failure never consumed an execution budget.
    pub async fn fail_run_preparation<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunPreparationFailure<TTarget, TGeneration>,
    ) -> Result<bool, StorageError> {
        use agent_automation::{PreparationEffect, SubmissionEffect};
        if request.now_ms < 0
            || request.explanation.is_empty()
            || request.effects.allocation == PreparationEffect::Unknown
            || request.effects.resume == PreparationEffect::Unknown
            || !matches!(
                request.effects.submission,
                SubmissionEffect::NotDispatched | SubmissionEffect::Rejected
            )
        {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        if record.phase != RunPhase::Preparing
            || record.evidence.timing.is_some()
            || record.native_turn_id.is_some()
            || record.evidence.native.target.is_some()
        {
            transaction.commit().await?;
            return Ok(false);
        }
        record.evidence.native = request.effects;
        let outcome = agent_automation::WorkerOutcome::Failed {
            explanation: Some(request.explanation),
        };
        sqlx::query("UPDATE workflow_runs SET run_status='preparationFailed',execution_evidence_json=?,worker_outcome_json=?,completed_at_ms=? WHERE run_id=?")
            .bind(serde_json::to_string(&record.evidence).map_err(|_|StorageError::InvalidRecord)?)
            .bind(serde_json::to_string(&outcome).map_err(|_|StorageError::InvalidRecord)?)
            .bind(request.now_ms).bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        let event = serde_json::json!({"kind":"stateChange","before":record.phase,"after":"preparationFailed"});
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'run',?,'preparationFailed',?,?)")
            .bind(agent_automation::EventId::generate().as_str()).bind(request.run_id.as_str()).bind(event.to_string()).bind(request.now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(true)
    }
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
