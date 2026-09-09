//! Eligible dispatch captures its budget once; waiting time never consumes execution timeout.
use crate::{AutomationStore, StorageError};
use agent_automation::{NativeEffectEvidence, RunExecutionEvidence, RunId};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
pub struct RunDispatchIntent<TTarget, TGeneration> {
    pub run_id: RunId,
    pub effects: NativeEffectEvidence<TTarget, TGeneration>,
    pub configured_timeout_seconds: u32,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn begin_run_dispatch<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunDispatchIntent<TTarget, TGeneration>,
    ) -> Result<RunExecutionEvidence<TTarget, TGeneration, TReceipt>, StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        if record.phase != agent_automation::RunPhase::Preparing
            || record.evidence.timing.is_some()
            || record.evidence.acceptance.is_some()
            || request.effects.target.is_none()
            || request.effects.generation.is_none()
            || request.effects.submission != agent_automation::SubmissionEffect::Dispatching
        {
            return Err(StorageError::InvalidRecord);
        }
        if matches!(
            record.evidence.native.submission,
            agent_automation::SubmissionEffect::Dispatching
                | agent_automation::SubmissionEffect::Accepted
                | agent_automation::SubmissionEffect::Unknown
        ) || matches!(
            record.evidence.native.allocation,
            agent_automation::PreparationEffect::Unknown
        ) || matches!(
            record.evidence.native.resume,
            agent_automation::PreparationEffect::Unknown
        ) {
            return Err(StorageError::InvalidRecord);
        }
        let inputs = record.inputs.ok_or(StorageError::InvalidRecord)?;
        let seconds = inputs
            .execution_configuration
            .execution_timeout_seconds
            .unwrap_or(request.configured_timeout_seconds);
        let timing = agent_automation::ExecutionTiming::start(request.now_ms, seconds)
            .ok_or(StorageError::InvalidRecord)?;
        let deadline = timing.deadline_at_ms;
        let evidence = RunExecutionEvidence {
            native: request.effects,
            timing: Some(timing),
            acceptance: None,
        };
        let encoded = serde_json::to_string(&evidence).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("UPDATE workflow_runs SET execution_started_at_ms=?,execution_deadline_at_ms=?,effective_timeout_seconds=?,execution_evidence_json=? WHERE run_id=? AND run_status='preparing'")
            .bind(request.now_ms).bind(deadline).bind(i64::from(seconds)).bind(encoded).bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(evidence)
    }
}
