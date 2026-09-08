//! Schedule configuration and frozen Run inputs; native address types are supplied by adapters.
use crate::{ChangeId, InstructionId, InstructionText, OperationId, RevisionId, RunId, TimingRule};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ExecutionDestination<TTarget, TEndpoint> {
    Unprepared,
    OwnedThread { target: TTarget, cwd: String },
    FreshEachRun { endpoint: TEndpoint, cwd: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleDefinition<TTarget, TEndpoint> {
    pub instruction_id: InstructionId,
    pub timing: TimingRule,
    pub enabled: bool,
    pub destination: ExecutionDestination<TTarget, TEndpoint>,
    pub execution_timeout_seconds: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ContinuityInput<TTarget> {
    None,
    Omitted {
        reason: String,
    },
    LocalSummary {
        text: String,
        #[serde(rename = "sourceRunId")]
        source_run_id: RunId,
        #[serde(rename = "sourceTarget")]
        source_target: TTarget,
        #[serde(rename = "sourceTurnId")]
        source_turn_id: String,
    },
    ImportedSummary {
        text: String,
        #[serde(rename = "sourceRunId")]
        source_run_id: String,
        #[serde(rename = "sourceTarget")]
        source_target: TTarget,
        #[serde(rename = "importOperationId")]
        import_operation_id: OperationId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenExecutionConfiguration<TTarget, TEndpoint> {
    pub destination: ExecutionDestination<TTarget, TEndpoint>,
    pub execution_timeout_seconds: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedRunInputs<TTarget, TEndpoint> {
    pub schedule_change_id: ChangeId,
    pub instruction_revision_id: RevisionId,
    pub instruction_text: InstructionText,
    pub continuity: ContinuityInput<TTarget>,
    pub execution_configuration: FrozenExecutionConfiguration<TTarget, TEndpoint>,
}

/// Stored summary provenance is separate from the text materialized on its Run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SummarySource<TTarget> {
    Completed {
        #[serde(rename = "sourceTarget")]
        source_target: TTarget,
        #[serde(rename = "sourceTurnId")]
        source_turn_id: String,
        #[serde(rename = "summaryAttemptId")]
        summary_attempt_id: crate::AttemptId,
    },
    Skipped {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleRecord<TTarget, TEndpoint> {
    pub schedule_id: crate::ScheduleId,
    pub change_id: ChangeId,
    pub definition: ScheduleDefinition<TTarget, TEndpoint>,
    pub imported_continuity: ContinuityInput<TTarget>,
    pub anchor_at_ms: i64,
    pub next_due_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
