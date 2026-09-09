//! Submission outcomes retain actual native acceptance and exact turn identity, never inferred success.
use crate::{AutomationStore, StorageError};
use agent_automation::{NativeEffectEvidence, RunId};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
pub enum RunSubmissionOutcome<TReceipt> {
    Accepted { turn_id: String, receipt: TReceipt },
    Rejected { explanation: String },
    Unknown { explanation: String },
}
pub struct RunSubmissionResult<TTarget, TGeneration, TReceipt> {
    pub run_id: RunId,
    pub effects: NativeEffectEvidence<TTarget, TGeneration>,
    pub outcome: RunSubmissionOutcome<TReceipt>,
}
impl AutomationStore {
    pub async fn record_run_submission<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunSubmissionResult<TTarget, TGeneration, TReceipt>,
    ) -> Result<(), StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        if record.phase != agent_automation::RunPhase::Preparing || record.evidence.timing.is_none()
        {
            return Err(StorageError::InvalidRecord);
        }
        let (phase, turn_id, outcome) = match request.outcome {
            RunSubmissionOutcome::Accepted { turn_id, receipt } => {
                if turn_id.is_empty()
                    || request.effects.submission != agent_automation::SubmissionEffect::Accepted
                    || request.effects.native_turn_id.as_deref() != Some(turn_id.as_str())
                    || serde_json::to_value(&request.effects.target)
                        .map_err(|_| StorageError::InvalidRecord)?
                        != serde_json::to_value(&record.evidence.native.target)
                            .map_err(|_| StorageError::InvalidRecord)?
                    || serde_json::to_value(&request.effects.generation)
                        .map_err(|_| StorageError::InvalidRecord)?
                        != serde_json::to_value(&record.evidence.native.generation)
                            .map_err(|_| StorageError::InvalidRecord)?
                    || request.effects.client_user_message_id
                        != record.evidence.native.client_user_message_id
                {
                    return Err(StorageError::InvalidRecord);
                }
                record.evidence.acceptance = Some(receipt);
                ("executing", Some(turn_id), None)
            }
            RunSubmissionOutcome::Rejected { explanation: _ } => {
                if !matches!(
                    request.effects.submission,
                    agent_automation::SubmissionEffect::Rejected
                        | agent_automation::SubmissionEffect::NotDispatched
                ) {
                    return Err(StorageError::InvalidRecord);
                }
                record.evidence.timing = None;
                record.evidence.acceptance = None;
                ("preparing", None, None::<agent_automation::WorkerOutcome>)
            }
            RunSubmissionOutcome::Unknown { explanation: _ } => {
                ("uncertain", request.effects.native_turn_id.clone(), None)
            }
        };
        record.evidence.native = request.effects;
        let evidence =
            serde_json::to_string(&record.evidence).map_err(|_| StorageError::InvalidRecord)?;
        let timing = record.evidence.timing.as_ref();
        sqlx::query("UPDATE workflow_runs SET run_status=?,native_turn_id=?,execution_evidence_json=?,worker_outcome_json=?,execution_started_at_ms=?,execution_deadline_at_ms=?,effective_timeout_seconds=? WHERE run_id=? AND run_status='preparing'")
            .bind(phase).bind(turn_id).bind(evidence).bind(outcome.map(|outcome|serde_json::to_string(&outcome)).transpose().map_err(|_|StorageError::InvalidRecord)?)
            .bind(timing.map(|value| value.dispatch_started_at_ms))
            .bind(timing.map(|value| value.deadline_at_ms))
            .bind(timing.map(|value| i64::from(value.effective_timeout_seconds)))
            .bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }
}
