//! Initial summary admission preserves the Run's occupied execution slot.
use crate::{AutomationStore, StorageError};
use agent_automation::{RunId, SummaryAttempt};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
pub struct SummaryAdmission {
    pub run_id: RunId,
    pub timeout_seconds: u32,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn begin_required_summary<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: DeserializeOwned,
    >(
        &mut self,
        request: &SummaryAdmission,
    ) -> Result<SummaryAttempt<TTarget, TGeneration>, StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        if record.phase != agent_automation::RunPhase::SummaryRequired
            || record.worker_outcome.is_none()
            || record.evidence.native.cessation != agent_automation::CessationEvidence::Confirmed
        {
            return Err(StorageError::InvalidRecord);
        }
        let inventory = crate::run_inventory::load(&mut transaction, &record.schedule_id).await?;
        if inventory.occupying.as_ref() != Some(&request.run_id) {
            return Err(StorageError::InvalidRecord);
        }
        let attempt = make_attempt(SummaryAttemptSeed {
            source_target: record
                .evidence
                .native
                .target
                .ok_or(StorageError::InvalidRecord)?,
            source_turn_id: record.native_turn_id.ok_or(StorageError::InvalidRecord)?,
            timeout_seconds: request.timeout_seconds,
            now_ms: request.now_ms,
        })?;
        sqlx::query("UPDATE workflow_runs SET run_status='summaryRunning',summary_attempt_json=? WHERE run_id=? AND run_status='summaryRequired'").bind(serde_json::to_string(&attempt).map_err(|_|StorageError::InvalidRecord)?).bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(attempt)
    }
}

pub(crate) struct SummaryAttemptSeed<TTarget> {
    pub source_target: TTarget,
    pub source_turn_id: String,
    pub timeout_seconds: u32,
    pub now_ms: i64,
}
pub(crate) fn make_attempt<TTarget, TGeneration>(
    seed: SummaryAttemptSeed<TTarget>,
) -> Result<SummaryAttempt<TTarget, TGeneration>, StorageError> {
    let timing = agent_automation::ExecutionTiming::start(seed.now_ms, seed.timeout_seconds)
        .ok_or(StorageError::InvalidRecord)?;
    Ok(SummaryAttempt {
        attempt_id: agent_automation::AttemptId::generate(),
        source_target: seed.source_target,
        source_turn_id: seed.source_turn_id,
        target: None,
        native_turn_id: None,
        effective_timeout_seconds: seed.timeout_seconds,
        started_at_ms: seed.now_ms,
        deadline_at_ms: timing.deadline_at_ms,
        phase: agent_automation::SummaryPhase::Preparing,
        effects: agent_automation::NativeEffectEvidence {
            target: None,
            generation: None,
            client_user_message_id: None,
            native_turn_id: None,
            native_submission_id: None,
            allocation: agent_automation::PreparationEffect::NotRequested,
            resume: agent_automation::PreparationEffect::NotRequested,
            submission: agent_automation::SubmissionEffect::NotDispatched,
            cessation: agent_automation::CessationEvidence::NotApplicable,
        },
        explanation: None,
    })
}
