//! Initial summary admission preserves the Run's occupied execution slot.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    RouteEffectEvidence, RouteSettlementState, RunId, SummaryAttempt, SummarySourceReference,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
pub struct SummaryAdmission {
    pub run_id: RunId,
    pub timeout_seconds: u32,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn begin_required_summary<
        TTarget: Clone + Serialize + DeserializeOwned,
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
        {
            return Err(StorageError::InvalidRecord);
        }
        let route = record
            .evidence
            .route
            .as_ref()
            .ok_or(StorageError::InvalidRecord)?;
        if !matches!(
            route.settlement_state(),
            RouteSettlementState::NativeTurnConfirmed
                | RouteSettlementState::ProviderOperationConfirmed
        ) {
            return Err(StorageError::InvalidRecord);
        }
        let inventory = crate::run_inventory::load(&mut transaction, &record.schedule_id).await?;
        if inventory.occupying.as_ref() != Some(&request.run_id) {
            return Err(StorageError::InvalidRecord);
        }
        let (source_target, source_reference) = match route {
            RouteEffectEvidence::CodexAppServer(native) => (
                native.target.clone().ok_or(StorageError::InvalidRecord)?,
                SummarySourceReference::NativeTurn {
                    turn_id: record.native_turn_id.ok_or(StorageError::InvalidRecord)?,
                },
            ),
            RouteEffectEvidence::ProviderAcp(provider) => (
                provider.target.clone().ok_or(StorageError::InvalidRecord)?,
                SummarySourceReference::ProviderOperation {
                    attempt_id: provider.attempt_id.clone(),
                },
            ),
            RouteEffectEvidence::ClaudeCodePeer(_) => return Err(StorageError::InvalidRecord),
        };
        let attempt = make_attempt(SummaryAttemptSeed {
            source_target,
            source_reference,
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
    pub source_reference: agent_automation::SummarySourceReference,
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
        source_reference: seed.source_reference,
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
