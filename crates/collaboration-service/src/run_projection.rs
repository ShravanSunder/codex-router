//! Run views derive lifecycle payloads from the validated stored record without inventing native IDs.
use crate::wakeup_projection::timestamp;
use agent_automation::{RunPhase, RunRecord};
use collaboration_protocol::{
    CodexGeneration, DeliveryReceipt, DeliveryRouteEvidence, EndpointRef, ExecutionTiming,
    NativeExecution, RetainedSummary, RunExecution, RunExecutionEvidence, RunSnapshot, RunState,
    SessionRef,
};
pub(crate) fn snapshot<TReceipt: Into<DeliveryReceipt>>(
    record: RunRecord<SessionRef, EndpointRef, CodexGeneration, TReceipt>,
) -> Result<RunSnapshot, ()> {
    let route = record
        .evidence
        .route
        .as_ref()
        .map(crate::delivery_route_projection::project)
        .transpose()?;
    let inputs = record
        .inputs
        .map(|inputs| {
            serde_json::from_value::<collaboration_protocol::CapturedRunInputs>(
                serde_json::to_value(inputs).map_err(|_| ())?,
            )
            .map_err(|_| ())
        })
        .transpose()?;
    let outcome = record
        .worker_outcome
        .map(|outcome| {
            serde_json::from_value::<collaboration_protocol::WorkerOutcome>(
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
    let execution = match (&route, &timing) {
        (Some(DeliveryRouteEvidence::CodexAppServer(native)), Some(timing)) => {
            match (native.target.as_ref(), record.native_turn_id.as_ref()) {
                (Some(target), Some(turn)) => Some(RunExecution::CodexAppServer(NativeExecution {
                    target: target.clone(),
                    turn_id: turn.clone(),
                    started_at: timing.dispatch_started_at.clone(),
                    deadline_at: timing.deadline_at.clone(),
                    effective_timeout_seconds: timing.effective_timeout_seconds,
                })),
                _ => None,
            }
        }
        (
            Some(DeliveryRouteEvidence::ProviderAcp {
                target,
                operation_id,
                ..
            }),
            Some(timing),
        ) => Some(RunExecution::ProviderAcp {
            target: target.clone(),
            operation_id: operation_id.clone(),
            started_at: timing.dispatch_started_at.clone(),
            deadline_at: timing.deadline_at.clone(),
            effective_timeout_seconds: timing.effective_timeout_seconds,
        }),
        (Some(DeliveryRouteEvidence::ClaudeCodePeer { session_id, .. }), Some(_)) => {
            let target = inputs.as_ref().and_then(|captured| {
                match &captured.execution_configuration.destination {
                    collaboration_protocol::ExecutionDestination::OwnedThread {
                        target, ..
                    } if String::from(target.session_id.clone()) == session_id.as_str() => {
                        Some(target.clone())
                    }
                    _ => None,
                }
            });
            match (target, record.completed_at_ms) {
                (Some(target), Some(written_at)) => Some(RunExecution::ClaudeCodePeer {
                    target,
                    written_at: timestamp(written_at)?,
                }),
                _ => None,
            }
        }
        _ => None,
    };
    let summary = match (record.summary_text, record.summary_source) {
        (
            Some(text),
            Some(agent_automation::SummarySource::Completed {
                source_target,
                source_reference,
                summary_attempt_id,
            }),
        ) => Some(RetainedSummary {
            text,
            source_run_id: record.run_id.clone(),
            source_target,
            source_reference,
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
        RunPhase::PreparationFailed=>RunState::PreparationFailed{explanation:match outcome { Some(collaboration_protocol::WorkerOutcome::Failed { explanation: Some(explanation) })=>explanation, _=>"Native work was not submitted; inspect preparation evidence.".into()}},
        RunPhase::Uncertain=>RunState::Uncertain{inputs:inputs.ok_or(())?,known_execution:execution,explanation:"Native effects are unresolved; no automatic replay or release of schedule ownership.".into()},
    };
    let snapshot = RunSnapshot {
        run_id: record.run_id,
        schedule_id: record.schedule_id,
        due_at: timestamp(record.due_at_ms)?,
        state,
        execution_evidence: RunExecutionEvidence {
            route,
            timing,
            acceptance: record.evidence.acceptance.map(Into::into),
        },
        summary,
    };
    snapshot.validate_evidence().map_err(|_| ())?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_automation::RunRecord;
    use collaboration_protocol::{DeliveryReceipt, RunExecution};
    use serde_json::json;

    fn captured_inputs(target: &SessionRef) -> serde_json::Value {
        json!({
            "scheduleChangeId":agent_automation::ChangeId::generate(),
            "instructionRevisionId":agent_automation::RevisionId::generate(),
            "instructionText":"Check the work",
            "continuity":{"kind":"none"},
            "executionConfiguration":{
                "destination":{"kind":"ownedThread","target":target,"cwd":"/tmp"},
                "executionTimeoutSeconds":120,"model":"fixture-model","effort":"medium"
            }
        })
    }

    #[test]
    fn provider_and_peer_runs_project_without_native_turns()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = "00000000-0000-4000-8000-000000000001";
        let provider_target: SessionRef = serde_json::from_value(json!({
            "endpoint":{"serviceId":service,"endpointId":"claude-local"},"sessionId":"provider-worker"
        }))?;
        let operation = agent_automation::AttemptId::generate();
        let provider: RunRecord<SessionRef, EndpointRef, CodexGeneration, DeliveryReceipt> =
            serde_json::from_value(json!({
                "runId":agent_automation::RunId::generate(),"scheduleId":agent_automation::ScheduleId::generate(),
                "dueAtMs":1000,"phase":"executing","inputs":captured_inputs(&provider_target),
                "threadBindingId":null,"nativeTurnId":null,
                "evidence":{"route":{"kind":"providerAcp","target":provider_target,
                    "generation":{"serviceEpoch":service,"generation":1},"binding":"binding-one",
                    "attemptId":operation,"submission":"accepted","settlement":"notObserved"},
                    "timing":{"dispatchStartedAtMs":1000,"effectiveTimeoutSeconds":120,"deadlineAtMs":121000},
                    "acceptance":{"outcome":{"kind":"started"},"reachability":"providerAcp",
                        "client":{"kind":"providerAcp","operationId":operation}}},
                "workerOutcome":null,"summaryAttempt":null,"summaryText":null,"summarySource":null,"completedAtMs":null
            }))?;
        let inspected = snapshot(provider).map_err(|_| "provider projection failed")?;
        if !matches!(
            inspected.state,
            RunState::Executing {
                execution: RunExecution::ProviderAcp { .. },
                ..
            }
        ) {
            return Err("provider execution became a native turn".into());
        }
        let wire = serde_json::to_value(&inspected)?;
        let decoded: RunSnapshot = serde_json::from_value(wire.clone())?;
        if serde_json::to_value(decoded)? != wire {
            return Err("provider run wire lost execution evidence".into());
        }

        let peer_target: SessionRef = serde_json::from_value(json!({
            "endpoint":{"serviceId":service,"endpointId":"claude-local"},"sessionId":"peer-worker"
        }))?;
        let peer: RunRecord<SessionRef, EndpointRef, CodexGeneration, DeliveryReceipt> =
            serde_json::from_value(json!({
                "runId":agent_automation::RunId::generate(),"scheduleId":agent_automation::ScheduleId::generate(),
                "dueAtMs":1000,"phase":"finished","inputs":captured_inputs(&peer_target),
                "threadBindingId":null,"nativeTurnId":null,
                "evidence":{"route":{"kind":"claudeCodePeer","sessionId":"peer-worker","processId":42,"write":"written"},
                    "timing":{"dispatchStartedAtMs":1000,"effectiveTimeoutSeconds":120,"deadlineAtMs":121000},
                    "acceptance":null},
                "workerOutcome":{"kind":"peerMessageWritten","explanation":"written"},
                "summaryAttempt":null,"summaryText":null,"summarySource":null,"completedAtMs":2000
            }))?;
        let inspected = snapshot(peer).map_err(|_| "peer projection failed")?;
        if !matches!(
            inspected.state,
            RunState::Finished {
                execution: RunExecution::ClaudeCodePeer { .. },
                summary_run_id: None,
                ..
            }
        ) || inspected.summary.is_some()
        {
            return Err("peer write did not finalize without a summary".into());
        }
        let wire = serde_json::to_value(&inspected)?;
        let decoded: RunSnapshot = serde_json::from_value(wire.clone())?;
        if serde_json::to_value(decoded)? != wire {
            return Err("peer run wire lost write evidence".into());
        }
        Ok(())
    }
}
