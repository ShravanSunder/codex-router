//! Run recovery errors never imply that unknown native work was stopped.
use crate::{LocalMutationEvidence, OperationId, RunId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunFailureKind {
    InvalidField,
    ResourceNotFound,
    OperationConflict,
    RecoveryNotAllowed,
    AutomationUnavailable,
    InvalidRecord,
    Overloaded,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunFailureStage {
    Validation,
    Inspection,
    Recovery,
    Admission,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunNextAction {
    CorrectRequest,
    InspectRun,
    InspectOperation,
    RetryLater,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunFailure {
    pub kind: RunFailureKind,
    pub stage: RunFailureStage,
    pub message: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub operation_id: Option<OperationId>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub run_id: Option<RunId>,
    pub effects: LocalMutationEvidence,
    pub next_action: RunNextAction,
}
