//! Run views derive lifecycle payloads from the validated stored record without inventing native IDs.
use crate::wakeup_projection::timestamp;
use agent_automation::{RunPhase, RunRecord};
use communication_protocol::{
    CodexGeneration, EndpointRef, ExecutionTiming, NativeExecution, NativeSendReceipt,
    RetainedSummary, RunExecutionEvidence, RunSnapshot, RunState, SessionRef,
};
pub(crate) fn snapshot(
    record: RunRecord<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>,
) -> Result<RunSnapshot, ()> {
    let inputs = record
        .inputs
        .map(|inputs| {
            serde_json::from_value::<communication_protocol::CapturedRunInputs>(
                serde_json::to_value(inputs).map_err(|_| ())?,
            )
            .map_err(|_| ())
        })
        .transpose()?;
    let outcome = record
        .worker_outcome
        .map(|outcome| {
            serde_json::from_value::<communication_protocol::WorkerOutcome>(
                serde_json::to_value(outcome).map_err(|_| ())?,
            )
            .map_err(|_| ())
        })
        .transpose()?;
    let timing = record
        .evidence
        .timing
        .map(|timing| {
            Ok::<_, ()>(ExecutionTiming {
                dispatch_started_at: timestamp(timing.dispatch_started_at_ms)?,
                effective_timeout_seconds: timing
                    .effective_timeout_seconds
                    .try_into()
                    .map_err(|_| ())?,
                deadline_at: timestamp(timing.deadline_at_ms)?,
            })
        })
        .transpose()?;
    let execution = match (
        &record.evidence.native.target,
        &record.native_turn_id,
        &timing,
    ) {
        (Some(target), Some(turn), Some(timing)) => Some(NativeExecution {
            target: target.clone(),
            turn_id: turn.clone(),
            started_at: timing.dispatch_started_at.clone(),
            deadline_at: timing.deadline_at.clone(),
            effective_timeout_seconds: timing.effective_timeout_seconds,
        }),
        _ => None,
    };
    let summary = match (record.summary_text, record.summary_source) {
        (
            Some(text),
            Some(agent_automation::SummarySource::Completed {
                source_target,
                source_turn_id,
                summary_attempt_id,
            }),
        ) => Some(RetainedSummary {
            text,
            source_run_id: record.run_id.clone(),
            source_target,
            source_turn_id,
            summary_attempt_id,
        }),
        (None, None | Some(agent_automation::SummarySource::Skipped { .. })) => None,
        _ => return Err(()),
    };
    let state=match record.phase{
        RunPhase::Waiting=>RunState::Waiting,
        RunPhase::Preparing=>RunState::Preparing{inputs:inputs.ok_or(())?},
        RunPhase::Executing=>RunState::Executing{inputs:inputs.ok_or(())?,execution:execution.ok_or(())?},
        RunPhase::Stopping=>RunState::Stopping{inputs:inputs.ok_or(())?,execution:execution.ok_or(())?},
        RunPhase::SummaryRequired=>RunState::SummaryRequired{inputs:inputs.ok_or(())?,execution:execution.ok_or(())?,outcome:outcome.ok_or(())?},
        RunPhase::SummaryRunning=>RunState::SummaryRunning{inputs:inputs.ok_or(())?,execution:execution.ok_or(())?,outcome:outcome.ok_or(())?,summary_attempt_id:record.summary_attempt.ok_or(())?.attempt_id},
        RunPhase::SummaryBlocked=>RunState::SummaryBlocked{inputs:inputs.ok_or(())?,execution:execution.ok_or(())?,outcome:outcome.ok_or(())?,explanation:record.summary_attempt.and_then(|attempt|attempt.explanation).unwrap_or_else(||"Summary is blocked; inspect its attempt and cessation evidence before retry or skip.".into())},
        RunPhase::Finished=>RunState::Finished{inputs:inputs.ok_or(())?,execution:execution.ok_or(())?,outcome:outcome.ok_or(())?,summary_run_id:summary.as_ref().map(|summary|summary.source_run_id.clone())},
        RunPhase::PreparationFailed=>RunState::PreparationFailed{explanation:match outcome { Some(communication_protocol::WorkerOutcome::Failed { explanation: Some(explanation) })=>explanation, _=>"Native work was not submitted; inspect preparation evidence.".into()}},
        RunPhase::Uncertain=>RunState::Uncertain{inputs:inputs.ok_or(())?,known_execution:execution,explanation:"Native effects are unresolved; no automatic replay or release of schedule ownership.".into()},
    };
    let snapshot = RunSnapshot {
        run_id: record.run_id,
        schedule_id: record.schedule_id,
        due_at: timestamp(record.due_at_ms)?,
        state,
        execution_evidence: RunExecutionEvidence {
            native: serde_json::from_value(
                serde_json::to_value(record.evidence.native).map_err(|_| ())?,
            )
            .map_err(|_| ())?,
            timing,
            acceptance: record.evidence.acceptance,
        },
        summary,
    };
    snapshot.validate_evidence().map_err(|_| ())?;
    Ok(snapshot)
}
