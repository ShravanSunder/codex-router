//! Cross-field evidence validation is shared by service projections and every SDK deserialization.
use crate::{
    DeliveryClientReceipt, DeliveryOutcome, DeliveryRouteEvidence, NativeSendAcceptance,
    ObservationTimestamp, RetainedSummary, RunExecution, RunExecutionEvidence, RunId, RunSnapshot,
    RunState, ScheduleId, SubmissionEffect, SummarySourceReference, WorkerOutcome,
};
use agent_automation::PeerWriteEffect;
use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunSnapshotFields {
    run_id: RunId,
    schedule_id: ScheduleId,
    due_at: ObservationTimestamp,
    state: RunState,
    execution_evidence: RunExecutionEvidence,
    #[serde(deserialize_with = "Option::deserialize")]
    summary: Option<RetainedSummary>,
}

impl<'de> Deserialize<'de> for RunSnapshot {
    fn deserialize<TDeserializer: Deserializer<'de>>(
        deserializer: TDeserializer,
    ) -> Result<Self, TDeserializer::Error> {
        let fields = RunSnapshotFields::deserialize(deserializer)?;
        let snapshot = Self {
            run_id: fields.run_id,
            schedule_id: fields.schedule_id,
            due_at: fields.due_at,
            state: fields.state,
            execution_evidence: fields.execution_evidence,
            summary: fields.summary,
        };
        snapshot
            .validate_evidence()
            .map_err(serde::de::Error::custom)?;
        Ok(snapshot)
    }
}

impl RunSnapshot {
    pub fn validate_evidence(&self) -> Result<(), &'static str> {
        let evidence = &self.execution_evidence;
        if evidence.route.is_none()
            && (evidence.timing.is_some()
                || evidence.acceptance.is_some()
                || self.summary.is_some())
        {
            return Err("unselected execution cannot have timing, acceptance, or summary evidence");
        }
        if let Some(timing) = &evidence.timing {
            let start = instant(&timing.dispatch_started_at)?;
            let deadline = instant(&timing.deadline_at)?;
            if deadline - start
                != chrono::Duration::seconds(i64::from(u32::from(timing.effective_timeout_seconds)))
            {
                return Err("executionEvidence.timing deadline does not match its captured budget");
            }
        }
        if let Some(execution) = execution(&self.state) {
            validate_execution(execution, &self.state, evidence)?;
        }
        if let Some(receipt) = &evidence.acceptance {
            validate_acceptance(receipt, evidence, execution(&self.state))?;
        }
        if let Some(summary) = &self.summary {
            validate_summary(summary, &self.run_id, execution(&self.state))?;
        }
        Ok(())
    }
}

fn execution(state: &RunState) -> Option<&RunExecution> {
    match state {
        RunState::Executing { execution, .. }
        | RunState::Stopping { execution, .. }
        | RunState::SummaryRequired { execution, .. }
        | RunState::SummaryRunning { execution, .. }
        | RunState::SummaryBlocked { execution, .. }
        | RunState::Finished { execution, .. } => Some(execution),
        RunState::Uncertain {
            known_execution, ..
        } => known_execution.as_ref(),
        _ => None,
    }
}

fn validate_execution(
    execution: &RunExecution,
    state: &RunState,
    evidence: &RunExecutionEvidence,
) -> Result<(), &'static str> {
    match (execution, evidence.route.as_ref()) {
        (
            RunExecution::CodexAppServer(execution),
            Some(DeliveryRouteEvidence::CodexAppServer(native)),
        ) => {
            if native.target.as_ref() != Some(&execution.target)
                || execution.turn_id.is_empty()
                || native.native_turn_id.as_deref() != Some(execution.turn_id.as_str())
            {
                return Err("native execution disagrees with selected turn evidence");
            }
            validate_timing(
                &execution.started_at,
                &execution.deadline_at,
                execution.effective_timeout_seconds,
                evidence.timing.as_ref(),
            )
        }
        (
            RunExecution::ProviderAcp {
                target,
                operation_id,
                started_at,
                deadline_at,
                effective_timeout_seconds,
            },
            Some(DeliveryRouteEvidence::ProviderAcp {
                target: selected,
                operation_id: selected_operation,
                ..
            }),
        ) => {
            if target != selected || operation_id != selected_operation {
                return Err("provider execution disagrees with selected operation evidence");
            }
            validate_timing(
                started_at,
                deadline_at,
                *effective_timeout_seconds,
                evidence.timing.as_ref(),
            )
        }
        (
            RunExecution::ClaudeCodePeer { target, written_at },
            Some(DeliveryRouteEvidence::ClaudeCodePeer { session_id, write }),
        ) => {
            if String::from(target.session_id.clone()) != session_id.as_str()
                || *write != PeerWriteEffect::Written
                || !matches!(
                    state,
                    RunState::Finished {
                        outcome: WorkerOutcome::PeerMessageWritten { .. },
                        ..
                    }
                )
            {
                return Err("peer execution must be a final written message");
            }
            let timing = evidence
                .timing
                .as_ref()
                .ok_or("peer write has no dispatch timing")?;
            if instant(written_at)? < instant(&timing.dispatch_started_at)? {
                return Err("peer write predates dispatch");
            }
            Ok(())
        }
        _ => Err("run execution and selected route differ"),
    }
}

fn validate_timing(
    started_at: &ObservationTimestamp,
    deadline_at: &ObservationTimestamp,
    seconds: crate::PositiveSeconds,
    timing: Option<&crate::ExecutionTiming>,
) -> Result<(), &'static str> {
    let timing = timing.ok_or("run execution has no dispatch timing")?;
    if instant(started_at)? != instant(&timing.dispatch_started_at)?
        || instant(deadline_at)? != instant(&timing.deadline_at)?
        || u32::from(seconds) != u32::from(timing.effective_timeout_seconds)
    {
        return Err("run execution timing disagrees with its captured budget");
    }
    Ok(())
}

fn validate_acceptance(
    receipt: &crate::DeliveryReceipt,
    evidence: &RunExecutionEvidence,
    execution: Option<&RunExecution>,
) -> Result<(), &'static str> {
    match (evidence.route.as_ref(), &receipt.client, execution) {
        (
            Some(DeliveryRouteEvidence::CodexAppServer(native)),
            Some(DeliveryClientReceipt::CodexAppServer(client)),
            Some(RunExecution::CodexAppServer(execution)),
        ) => {
            if !matches!(native.submission, SubmissionEffect::Accepted)
                || native.target.as_ref() != Some(&client.target)
                || native.generation.as_ref() != Some(&client.generation)
                || native.client_user_message_id.as_deref()
                    != Some(String::from(client.client_user_message_id.clone()).as_str())
                || !matches!(
                    &receipt.outcome,
                    DeliveryOutcome::StartedOrSteered | DeliveryOutcome::Steered
                )
            {
                return Err("native run acceptance disagrees with selected evidence");
            }
            let turn_id = match &client.acceptance {
                NativeSendAcceptance::NativeInputAccepted { turn_id, .. }
                | NativeSendAcceptance::SteerAccepted { turn_id, .. } => {
                    String::from(turn_id.clone())
                }
                NativeSendAcceptance::QueueAccepted { .. } => {
                    return Err("scheduled run cannot have queue acceptance");
                }
            };
            if native.native_turn_id.as_deref() != Some(turn_id.as_str())
                || execution.turn_id != turn_id
            {
                return Err("native run acceptance turn differs from execution");
            }
            Ok(())
        }
        (
            Some(DeliveryRouteEvidence::ProviderAcp {
                operation_id,
                submission,
                ..
            }),
            Some(DeliveryClientReceipt::ProviderAcp {
                operation_id: accepted,
            }),
            Some(RunExecution::ProviderAcp {
                operation_id: executing,
                ..
            }),
        ) if matches!(submission, SubmissionEffect::Accepted)
            && operation_id == accepted
            && accepted == executing
            && matches!(&receipt.outcome, DeliveryOutcome::Started) =>
        {
            Ok(())
        }
        (
            Some(DeliveryRouteEvidence::ClaudeCodePeer {
                write: PeerWriteEffect::Written,
                ..
            }),
            Some(DeliveryClientReceipt::ClaudeCodePeer),
            Some(RunExecution::ClaudeCodePeer { .. }),
        ) if matches!(&receipt.outcome, DeliveryOutcome::PeerMessageWritten) => Ok(()),
        _ => Err("run acceptance and selected route differ"),
    }
}

fn validate_summary(
    summary: &RetainedSummary,
    run_id: &RunId,
    execution: Option<&RunExecution>,
) -> Result<(), &'static str> {
    if &summary.source_run_id != run_id {
        return Err("summary belongs to another Run");
    }
    match (execution, &summary.source_reference) {
        (
            Some(RunExecution::CodexAppServer(native)),
            SummarySourceReference::NativeTurn { turn_id },
        ) if summary.source_target == native.target && turn_id == &native.turn_id => Ok(()),
        (
            Some(RunExecution::ProviderAcp {
                target,
                operation_id,
                ..
            }),
            SummarySourceReference::ProviderOperation { attempt_id },
        ) if &summary.source_target == target && attempt_id.as_str() == operation_id.as_str() => {
            Ok(())
        }
        _ => Err("summary source disagrees with its producing Run execution"),
    }
}

fn instant(
    value: &ObservationTimestamp,
) -> Result<chrono::DateTime<chrono::FixedOffset>, &'static str> {
    chrono::DateTime::parse_from_rfc3339(&String::from(value.clone()))
        .map_err(|_| "invalid execution timestamp")
}
