//! Exact persisted turn observations may finish a Run; reconciliation cannot start or interrupt work.
use crate::{NativeAdmission, NativeControlBackend};
use agent_automation::{RunPhase, RunRecord, WorkerOutcome};
use automation_storage::{AutomationStore, RunCompletion, StorageError};
use communication_protocol::{CodexGeneration, EndpointRef, NativeSendReceipt, SessionRef};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) async fn reconcile(
    store: &Arc<Mutex<AutomationStore>>,
    backend: Option<&NativeControlBackend>,
    record: RunRecord<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>,
) -> Result<(), StorageError> {
    let Some(backend) = backend else {
        return Ok(());
    };
    let Some(target) = record.evidence.native.target.as_ref() else {
        return Ok(());
    };
    if target.endpoint != backend.endpoint {
        return Ok(());
    }
    let Ok(admission) = backend.gate.acquire() else {
        return Ok(());
    };
    match record.phase {
        RunPhase::Executing | RunPhase::Stopping | RunPhase::Uncertain => {
            observe_worker(store, &admission, &record).await?;
        }
        RunPhase::SummaryRunning | RunPhase::SummaryBlocked => {
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
            let timeout_seconds = attempt.effective_timeout_seconds;
            crate::summary_native_worker::step(crate::summary_native_worker::SummaryStep {
                store,
                admission: &admission,
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

/// Shared by background observation and explicit reconciliation; never issues a native mutation.
/// True means exact terminal evidence was found, including a concurrently recorded completion.
pub(crate) async fn observe_worker(
    store: &Arc<Mutex<AutomationStore>>,
    admission: &NativeAdmission,
    record: &RunRecord<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>,
) -> Result<bool, StorageError> {
    let (Some(target), Some(turn_id)) = (&record.evidence.native.target, &record.native_turn_id)
    else {
        return Ok(false);
    };
    let retired = admission.retirement();
    let observed = tokio::select! {
        biased;
        _ = retired.cancelled() => return Ok(false),
        observed = tokio::time::timeout(std::time::Duration::from_secs(20), crate::scheduled_native_observation::read_turn(admission, target, turn_id)) => observed,
    };
    let Ok(Ok(Some(turn))) = observed else {
        return Ok(false);
    };
    if retired.is_cancelled() {
        return Ok(false);
    }
    let outcome = match turn.get("status").and_then(serde_json::Value::as_str) {
        Some("completed") => WorkerOutcome::Completed { explanation: None },
        Some("failed") => WorkerOutcome::Failed { explanation: Some("Native turn failed; inspect its recorded output.".into()) },
        Some("interrupted") => WorkerOutcome::Interrupted { explanation: Some(if record.phase == RunPhase::Stopping { "Native history confirms the recorded turn is interrupted; timeout stopping intent was recorded, but attribution to that request is not confirmed." } else { "Native turn was interrupted." }.into()) },
        _ => return Ok(false),
    };
    store
        .lock()
        .await
        .complete_run_turn::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
            RunCompletion {
                run_id: record.run_id.clone(),
                native_turn_id: turn_id.clone(),
                outcome,
                now_ms: chrono::Utc::now().timestamp_millis(),
            },
        )
        .await?;
    Ok(true)
}
