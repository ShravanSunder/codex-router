//! Eligible dispatch captures its budget once; waiting time never consumes execution timeout.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    PeerWriteEffect, ProviderSettlementEffect, RouteEffectEvidence, RunExecutionEvidence, RunId,
    SubmissionEffect,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
pub struct RunDispatchIntent<TTarget, TGeneration> {
    pub run_id: RunId,
    pub effects: RouteEffectEvidence<TTarget, TGeneration>,
    pub configured_timeout_seconds: u32,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn begin_run_dispatch<
        TTarget: PartialEq + Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: PartialEq + Serialize + DeserializeOwned,
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
        {
            return Err(StorageError::InvalidRecord);
        }
        let dispatching = match &request.effects {
            RouteEffectEvidence::CodexAppServer(native) => {
                native.target.is_some()
                    && native.generation.is_some()
                    && native.submission == SubmissionEffect::Dispatching
            }
            RouteEffectEvidence::ProviderAcp(provider) => {
                provider.target.is_some()
                    && provider.submission == SubmissionEffect::Dispatching
                    && provider.settlement == ProviderSettlementEffect::NotObserved
            }
            RouteEffectEvidence::ClaudeCodePeer(peer) => peer.write == PeerWriteEffect::Dispatching,
        };
        if !dispatching {
            return Err(StorageError::InvalidRecord);
        }
        let selected = record
            .evidence
            .route
            .as_ref()
            .ok_or(StorageError::InvalidRecord)?;
        let same_selected_route = match (selected, &request.effects) {
            (
                RouteEffectEvidence::CodexAppServer(before),
                RouteEffectEvidence::CodexAppServer(after),
            ) => {
                before.target == after.target
                    && before.generation == after.generation
                    && !matches!(
                        before.submission,
                        SubmissionEffect::Dispatching
                            | SubmissionEffect::Accepted
                            | SubmissionEffect::Unknown
                    )
                    && before.allocation != agent_automation::PreparationEffect::Unknown
                    && before.resume != agent_automation::PreparationEffect::Unknown
            }
            (RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)) => {
                before.target == after.target
                    && before.generation == after.generation
                    && before.binding == after.binding
                    && before.attempt_id == after.attempt_id
                    && before.submission == SubmissionEffect::NotDispatched
                    && after.submission == SubmissionEffect::Dispatching
            }
            (
                RouteEffectEvidence::ClaudeCodePeer(before),
                RouteEffectEvidence::ClaudeCodePeer(after),
            ) => {
                before.session_id == after.session_id
                    && before.process_id == after.process_id
                    && before.write == PeerWriteEffect::NotDispatched
                    && after.write == PeerWriteEffect::Dispatching
            }
            _ => false,
        };
        if !same_selected_route {
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
            route: Some(request.effects),
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
