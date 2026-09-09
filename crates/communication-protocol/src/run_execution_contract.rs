//! Complete public Run state preserves frozen inputs, actual native receipts and separate summary results.
use crate::{
    ChangeId, ExecutionDestination, NativeEffectEvidence, NativeSendReceipt, ObservationTimestamp,
    OperationId, PositiveSeconds, RevisionId, RunId, ScheduleId, SessionRef,
};
use agent_automation::AttemptId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ContinuityInput {
    None,
    Omitted {
        reason: String,
    },
    LocalSummary {
        text: String,
        source_run_id: RunId,
        source_target: SessionRef,
        source_turn_id: String,
    },
    ImportedSummary {
        text: String,
        source_run_id: String,
        source_target: SessionRef,
        import_operation_id: OperationId,
    },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenExecutionConfiguration {
    pub destination: ExecutionDestination,
    #[serde(deserialize_with = "Option::deserialize")]
    pub execution_timeout_seconds: Option<PositiveSeconds>,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedRunInputs {
    pub schedule_change_id: ChangeId,
    pub instruction_revision_id: RevisionId,
    pub instruction_text: crate::InstructionText,
    pub continuity: ContinuityInput,
    pub execution_configuration: FrozenExecutionConfiguration,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeExecution {
    pub target: SessionRef,
    pub turn_id: String,
    pub started_at: ObservationTimestamp,
    pub deadline_at: ObservationTimestamp,
    pub effective_timeout_seconds: PositiveSeconds,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum WorkerOutcome {
    Completed {
        #[serde(deserialize_with = "Option::deserialize")]
        explanation: Option<String>,
    },
    Failed {
        #[serde(deserialize_with = "Option::deserialize")]
        explanation: Option<String>,
    },
    Interrupted {
        #[serde(deserialize_with = "Option::deserialize")]
        explanation: Option<String>,
    },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RunState {
    Waiting,
    Preparing {
        inputs: CapturedRunInputs,
    },
    Executing {
        inputs: CapturedRunInputs,
        execution: NativeExecution,
    },
    Stopping {
        inputs: CapturedRunInputs,
        execution: NativeExecution,
    },
    SummaryRequired {
        inputs: CapturedRunInputs,
        execution: NativeExecution,
        outcome: WorkerOutcome,
    },
    SummaryRunning {
        inputs: CapturedRunInputs,
        execution: NativeExecution,
        outcome: WorkerOutcome,
        summary_attempt_id: AttemptId,
    },
    SummaryBlocked {
        inputs: CapturedRunInputs,
        execution: NativeExecution,
        outcome: WorkerOutcome,
        explanation: String,
    },
    Finished {
        inputs: CapturedRunInputs,
        execution: NativeExecution,
        outcome: WorkerOutcome,
        #[serde(deserialize_with = "Option::deserialize")]
        summary_run_id: Option<RunId>,
    },
    PreparationFailed {
        explanation: String,
    },
    Uncertain {
        inputs: CapturedRunInputs,
        #[serde(deserialize_with = "Option::deserialize")]
        known_execution: Option<NativeExecution>,
        explanation: String,
    },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionTiming {
    pub dispatch_started_at: ObservationTimestamp,
    pub effective_timeout_seconds: PositiveSeconds,
    pub deadline_at: ObservationTimestamp,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunExecutionEvidence {
    pub native: NativeEffectEvidence,
    #[serde(deserialize_with = "Option::deserialize")]
    pub timing: Option<ExecutionTiming>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub acceptance: Option<NativeSendReceipt>,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedSummary {
    pub text: String,
    pub source_run_id: RunId,
    pub source_target: SessionRef,
    pub source_turn_id: String,
    pub summary_attempt_id: AttemptId,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSnapshot {
    pub run_id: RunId,
    pub schedule_id: ScheduleId,
    pub due_at: ObservationTimestamp,
    pub state: RunState,
    pub execution_evidence: RunExecutionEvidence,
    #[serde(deserialize_with = "Option::deserialize")]
    pub summary: Option<RetainedSummary>,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunShowRequest {
    pub run_id: RunId,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunRecoveryRequest {
    pub operation_id: OperationId,
    pub run_id: RunId,
}
