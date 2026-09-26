//! Reconciliation asks the recorded route for exact settlement and never resubmits work.
use crate::{
    DeliveryContractError, NativeControlBackend, RunObservationContext, RunReconciliation,
    RunSettlement, ScheduledRunExecution,
};
use agent_automation::{RouteEffectEvidence, RunPhase, RunRecord, WorkerOutcome};
use automation_storage::{AutomationStore, RunCompletion, RunStopIdentity, StorageError};
use collaboration_protocol::{CodexGeneration, EndpointRef, SessionRef};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) type StoredRun = RunRecord<
    SessionRef,
    EndpointRef,
    CodexGeneration,
    crate::stored_run_receipt::StoredRunReceipt,
>;

pub(crate) async fn reconcile(
    store: &Arc<Mutex<AutomationStore>>,
    execution: Option<&Arc<dyn ScheduledRunExecution>>,
    backend: Option<&NativeControlBackend>,
    record: StoredRun,
) -> Result<(), StorageError> {
    match record.phase {
        RunPhase::Executing | RunPhase::Stopping | RunPhase::Uncertain => {
            let Some(execution) = execution else {
                return Ok(());
            };
            let Some(recorded) = record.evidence.route.clone() else {
                return Ok(());
            };
            let inputs = record.inputs.clone().ok_or(StorageError::InvalidRecord)?;
            let observation = execution
                .reconcile_run(RunObservationContext {
                    run_id: record.run_id.clone(),
                    phase: record.phase,
                    recorded,
                    inputs,
                })
                .await;
            match observation {
                Ok(RunReconciliation::Settled { settlement }) => {
                    persist_settlement(store, &record, settlement).await?;
                }
                Ok(RunReconciliation::KnownNotSubmitted | RunReconciliation::StillUnknown)
                | Err(DeliveryContractError::ClientOperation) => {}
                Err(_) => return Err(StorageError::InvalidRecord),
            }
        }
        RunPhase::SummaryRunning | RunPhase::SummaryBlocked => {
            let Some(backend) = backend else {
                return Ok(());
            };
            let Some(attempt) = record.summary_attempt.as_ref() else {
                return Err(StorageError::InvalidRecord);
            };
            if attempt
                .target
                .as_ref()
                .is_none_or(|target| target.endpoint != backend.endpoint)
            {
                return Ok(());
            }
            let Ok(admission) = backend.gate.acquire() else {
                return Ok(());
            };
            let timeout_seconds = attempt.effective_timeout_seconds;
            crate::summary_native_worker::step(crate::summary_native_worker::SummaryStep {
                store,
                admission: &admission,
                summary_endpoint: backend.endpoint.clone(),
                source: None,
                record,
                timeout_seconds,
                work: crate::summary_native_worker::SummaryWork::ObserveOnly,
            })
            .await?;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) async fn persist_settlement(
    store: &Arc<Mutex<AutomationStore>>,
    record: &StoredRun,
    settlement: RunSettlement,
) -> Result<bool, StorageError> {
    let outcome = match settlement {
        RunSettlement::Pending => return Ok(false),
        RunSettlement::Completed { .. } => WorkerOutcome::Completed { explanation: None },
        RunSettlement::Failed { reason } => WorkerOutcome::Failed {
            explanation: Some(reason),
        },
        RunSettlement::Interrupted => WorkerOutcome::Interrupted {
            explanation: Some(if record.phase == RunPhase::Stopping {
                "Native history confirms the recorded turn is interrupted; timeout stopping intent was recorded, but attribution to that request is not confirmed.".into()
            } else {
                "Native turn was interrupted.".into()
            }),
        },
        RunSettlement::WrittenWithoutCompletion => WorkerOutcome::PeerMessageWritten {
            explanation: "Peer message written; receiver completion was not observed.".into(),
        },
    };
    let recorded = record
        .evidence
        .route
        .as_ref()
        .ok_or(StorageError::InvalidRecord)?;
    let identity = match recorded {
        RouteEffectEvidence::CodexAppServer(_) => RunStopIdentity::NativeTurn(
            record
                .native_turn_id
                .clone()
                .ok_or(StorageError::InvalidRecord)?,
        ),
        RouteEffectEvidence::ProviderAcp(provider) => {
            RunStopIdentity::ProviderOperation(provider.attempt_id.clone())
        }
        RouteEffectEvidence::ClaudeCodePeer(_) => RunStopIdentity::PeerMessageWritten,
    };
    store
        .lock()
        .await
        .complete_run_settlement::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
            RunCompletion {
                run_id: record.run_id.clone(),
                settlement: identity,
                outcome,
                now_ms: chrono::Utc::now().timestamp_millis(),
            },
        )
        .await
}
