//! Submission outcomes retain actual native acceptance and exact turn identity, never inferred success.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    PeerWriteEffect, ProviderSettlementEffect, RouteEffectEvidence, RunId, SubmissionEffect,
    WorkerOutcome,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
pub enum RunSubmissionOutcome<TReceipt> {
    Accepted { turn_id: String, receipt: TReceipt },
    ProviderAdmitted { receipt: TReceipt },
    PeerWritten { written_at_ms: i64 },
    Rejected { explanation: String },
    Unknown { explanation: String },
}
pub struct RunSubmissionResult<TTarget, TGeneration, TReceipt> {
    pub run_id: RunId,
    pub effects: RouteEffectEvidence<TTarget, TGeneration>,
    pub outcome: RunSubmissionOutcome<TReceipt>,
}
impl AutomationStore {
    pub async fn record_run_submission<
        TTarget: PartialEq + Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: PartialEq + Serialize + DeserializeOwned,
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
        let selected = record
            .evidence
            .route
            .as_ref()
            .ok_or(StorageError::InvalidRecord)?;
        if !selected.same_client_identity(&request.effects) {
            return Err(StorageError::InvalidRecord);
        }
        let (phase, turn_id, outcome, completed_at_ms) = match request.outcome {
            RunSubmissionOutcome::Accepted { turn_id, receipt } => {
                let (
                    RouteEffectEvidence::CodexAppServer(recorded_native),
                    RouteEffectEvidence::CodexAppServer(reported_native),
                ) = (selected, &request.effects)
                else {
                    return Err(StorageError::InvalidRecord);
                };
                if turn_id.is_empty()
                    || reported_native.submission != SubmissionEffect::Accepted
                    || reported_native.native_turn_id.as_deref() != Some(turn_id.as_str())
                    || serde_json::to_value(&reported_native.target)
                        .map_err(|_| StorageError::InvalidRecord)?
                        != serde_json::to_value(&recorded_native.target)
                            .map_err(|_| StorageError::InvalidRecord)?
                    || serde_json::to_value(&reported_native.generation)
                        .map_err(|_| StorageError::InvalidRecord)?
                        != serde_json::to_value(&recorded_native.generation)
                            .map_err(|_| StorageError::InvalidRecord)?
                    || reported_native.client_user_message_id
                        != recorded_native.client_user_message_id
                {
                    return Err(StorageError::InvalidRecord);
                }
                record.evidence.acceptance = Some(receipt);
                ("executing", Some(turn_id), None, None)
            }
            RunSubmissionOutcome::ProviderAdmitted { receipt } => {
                let (
                    RouteEffectEvidence::ProviderAcp(before),
                    RouteEffectEvidence::ProviderAcp(after),
                ) = (selected, &request.effects)
                else {
                    return Err(StorageError::InvalidRecord);
                };
                if before.target != after.target
                    || before.generation != after.generation
                    || before.binding != after.binding
                    || before.attempt_id != after.attempt_id
                    || after.submission != SubmissionEffect::Accepted
                    || after.settlement != ProviderSettlementEffect::NotObserved
                {
                    return Err(StorageError::InvalidRecord);
                }
                record.evidence.acceptance = Some(receipt);
                ("executing", None, None, None)
            }
            RunSubmissionOutcome::PeerWritten { written_at_ms } => {
                let (
                    RouteEffectEvidence::ClaudeCodePeer(before),
                    RouteEffectEvidence::ClaudeCodePeer(after),
                ) = (selected, &request.effects)
                else {
                    return Err(StorageError::InvalidRecord);
                };
                let dispatch_started_at_ms = record
                    .evidence
                    .timing
                    .as_ref()
                    .ok_or(StorageError::InvalidRecord)?
                    .dispatch_started_at_ms;
                if written_at_ms < dispatch_started_at_ms
                    || before.session_id != after.session_id
                    || before.process_id != after.process_id
                    || after.write != PeerWriteEffect::Written
                {
                    return Err(StorageError::InvalidRecord);
                }
                (
                    "finished",
                    None,
                    Some(WorkerOutcome::PeerMessageWritten {
                        explanation: "Peer message written; receiver completion was not observed."
                            .into(),
                    }),
                    Some(written_at_ms),
                )
            }
            RunSubmissionOutcome::Rejected { explanation: _ } => {
                let known_none = match &request.effects {
                    RouteEffectEvidence::CodexAppServer(native) => matches!(
                        native.submission,
                        SubmissionEffect::Rejected | SubmissionEffect::NotDispatched
                    ),
                    RouteEffectEvidence::ProviderAcp(provider) => matches!(
                        provider.submission,
                        SubmissionEffect::Rejected | SubmissionEffect::NotDispatched
                    ),
                    RouteEffectEvidence::ClaudeCodePeer(peer) => {
                        peer.write == PeerWriteEffect::NotDispatched
                    }
                };
                if !known_none {
                    return Err(StorageError::InvalidRecord);
                }
                record.evidence.timing = None;
                record.evidence.acceptance = None;
                ("preparing", None, None, None)
            }
            RunSubmissionOutcome::Unknown { explanation: _ } => {
                let turn_id = match &request.effects {
                    RouteEffectEvidence::CodexAppServer(native) => native.native_turn_id.clone(),
                    RouteEffectEvidence::ProviderAcp(_)
                    | RouteEffectEvidence::ClaudeCodePeer(_) => None,
                };
                ("uncertain", turn_id, None, None)
            }
        };
        record.evidence.route = Some(request.effects);
        let evidence =
            serde_json::to_string(&record.evidence).map_err(|_| StorageError::InvalidRecord)?;
        let timing = record.evidence.timing.as_ref();
        sqlx::query("UPDATE workflow_runs SET run_status=?,native_turn_id=?,execution_evidence_json=?,worker_outcome_json=?,execution_started_at_ms=?,execution_deadline_at_ms=?,effective_timeout_seconds=?,completed_at_ms=? WHERE run_id=? AND run_status='preparing'")
            .bind(phase).bind(turn_id).bind(evidence).bind(outcome.map(|outcome|serde_json::to_string(&outcome)).transpose().map_err(|_|StorageError::InvalidRecord)?)
            .bind(timing.map(|value| value.dispatch_started_at_ms))
            .bind(timing.map(|value| value.deadline_at_ms))
            .bind(timing.map(|value| i64::from(value.effective_timeout_seconds)))
            .bind(completed_at_ms)
            .bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }
}
