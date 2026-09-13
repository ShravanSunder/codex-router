//! Cross-field evidence validation is shared by service projections and every SDK deserialization.
use crate::{
    NativeExecution, NativeSendAcceptance, ObservationTimestamp, RetainedSummary,
    RunExecutionEvidence, RunId, RunSnapshot, RunState, ScheduleId, SubmissionEffect,
};
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
            let timing = evidence
                .timing
                .as_ref()
                .ok_or("state.execution requires executionEvidence.timing")?;
            if evidence.native.target.as_ref() != Some(&execution.target) {
                return Err(
                    "state.execution.target disagrees with executionEvidence.native.target",
                );
            }
            if execution.turn_id.is_empty()
                || evidence.native.native_turn_id.as_deref() != Some(execution.turn_id.as_str())
            {
                return Err(
                    "state.execution.nativeTurnId disagrees with executionEvidence.native.nativeTurnId",
                );
            }
            if instant(&execution.started_at)? != instant(&timing.dispatch_started_at)?
                || instant(&execution.deadline_at)? != instant(&timing.deadline_at)?
                || u32::from(execution.effective_timeout_seconds)
                    != u32::from(timing.effective_timeout_seconds)
            {
                return Err("state.execution timing disagrees with executionEvidence.timing");
            }
        }
        if let Some(receipt) = &evidence.acceptance {
            if !matches!(evidence.native.submission, SubmissionEffect::Accepted)
                || evidence.native.target.as_ref() != Some(&receipt.target)
                || evidence.native.generation.as_ref() != Some(&receipt.generation)
                || evidence.native.client_user_message_id.as_deref()
                    != Some(String::from(receipt.client_user_message_id.clone()).as_str())
            {
                return Err(
                    "executionEvidence.acceptance disagrees with native target, generation, correlation or submission",
                );
            }
            let turn_id = match &receipt.acceptance {
                NativeSendAcceptance::NativeInputAccepted { turn_id, .. }
                | NativeSendAcceptance::SteerAccepted { turn_id, .. } => {
                    String::from(turn_id.clone())
                }
                NativeSendAcceptance::QueueAccepted { .. } => {
                    return Err("scheduled execution cannot use a queue receipt as turn evidence");
                }
            };
            if evidence.native.native_turn_id.as_deref() != Some(turn_id.as_str()) {
                return Err(
                    "executionEvidence.acceptance turn disagrees with native turn identity",
                );
            }
        }
        if let Some(summary) = &self.summary
            && (summary.source_run_id != self.run_id
                || evidence.native.target.as_ref() != Some(&summary.source_target)
                || evidence.native.native_turn_id.as_deref()
                    != Some(summary.source_turn_id.as_str()))
        {
            return Err("summary source disagrees with its producing Run execution");
        }
        Ok(())
    }
}

fn execution(state: &RunState) -> Option<&NativeExecution> {
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

fn instant(
    value: &ObservationTimestamp,
) -> Result<chrono::DateTime<chrono::FixedOffset>, &'static str> {
    chrono::DateTime::parse_from_rfc3339(&String::from(value.clone()))
        .map_err(|_| "invalid execution timestamp")
}
