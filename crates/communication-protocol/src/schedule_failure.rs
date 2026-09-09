//! Actionable schedule failures preserve local mutation uncertainty and stale-edit identity.
use crate::{LocalMutationEvidence, OperationId};
use agent_automation::{ChangeId, ScheduleId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleFailureKind {
    InvalidField,
    OperationConflict,
    ResourceNotFound,
    ChangeConflict,
    AutomationUnavailable,
    InvalidRecord,
    UnsupportedCapability,
    OwnershipConflict,
    Overloaded,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleFailureStage {
    Validation,
    Admission,
    Storage,
    Inspection,
    Preparation,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleNextAction {
    CorrectRequest,
    InspectOperation,
    InspectSchedule,
    InspectEndpointCapabilities,
    SelectDifferentThread,
    RetryLater,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleFailure {
    pub kind: ScheduleFailureKind,
    pub stage: ScheduleFailureStage,
    pub message: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub operation_id: Option<OperationId>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub schedule_id: Option<ScheduleId>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub current_change_id: Option<ChangeId>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub field: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub constraint: Option<String>,
    pub effects: LocalMutationEvidence,
    pub next_action: ScheduleNextAction,
}
