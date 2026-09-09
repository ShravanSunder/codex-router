//! Schedule configuration remains separate from derived Run occupancy and timer progress.
use crate::{
    EndpointRef, InstructionId, ObservationTimestamp, OperationId, PositiveSeconds, SessionRef,
    TimingRequest,
};
use agent_automation::{ChangeId, RunId, ScheduleId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ExecutionDestination {
    Unprepared,
    OwnedThread { target: SessionRef, cwd: String },
    FreshEachRun { endpoint: EndpointRef, cwd: String },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleDefinition {
    pub instruction_id: InstructionId,
    pub timing: TimingRequest,
    pub enabled: bool,
    pub destination: ExecutionDestination,
    #[serde(deserialize_with = "Option::deserialize")]
    pub execution_timeout_seconds: Option<PositiveSeconds>,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ImportedContinuity {
    None,
    ImportedSummary {
        text: String,
        source_run_id: String,
        source_target: SessionRef,
        import_operation_id: OperationId,
    },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleSnapshot {
    pub schedule_id: ScheduleId,
    pub change_id: ChangeId,
    pub definition: ScheduleDefinition,
    pub imported_continuity: ImportedContinuity,
    pub anchor_at: ObservationTimestamp,
    #[serde(deserialize_with = "Option::deserialize")]
    pub next_due_at: Option<ObservationTimestamp>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub active_run_id: Option<RunId>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub waiting_run_id: Option<RunId>,
    pub created_at: ObservationTimestamp,
    pub updated_at: ObservationTimestamp,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleCreateRequest {
    pub operation_id: OperationId,
    pub definition: ScheduleDefinition,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleUpdateRequest {
    pub operation_id: OperationId,
    pub schedule_id: ScheduleId,
    pub expected_change_id: ChangeId,
    pub definition: ScheduleDefinition,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleShowRequest {
    pub schedule_id: ScheduleId,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleEnableRequest {
    pub operation_id: OperationId,
    pub schedule_id: ScheduleId,
}
