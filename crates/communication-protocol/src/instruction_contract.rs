//! Instruction RPC contracts specialize domain values without acquiring storage ownership.
use crate::ObservationTimestamp;
use agent_automation::{InstructionId, InstructionText, OperationId, RevisionId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstructionCreateParams {
    pub operation_id: OperationId,
    pub text: InstructionText,
}
#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstructionUpdateParams {
    pub operation_id: OperationId,
    pub instruction_id: InstructionId,
    pub expected_revision_id: RevisionId,
    pub text: InstructionText,
}
#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstructionShowParams {
    pub instruction_id: InstructionId,
}
#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstructionSnapshot {
    pub instruction_id: InstructionId,
    pub revision_id: RevisionId,
    pub text: InstructionText,
    pub created_at: ObservationTimestamp,
    pub updated_at: ObservationTimestamp,
}

#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstructionFailureKind {
    InvalidField,
    OperationConflict,
    ResourceNotFound,
    RevisionConflict,
    AutomationUnavailable,
    InvalidRecord,
    Overloaded,
}
#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstructionStage {
    Validation,
    Storage,
    Inspection,
    Admission,
}
#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstructionNextAction {
    CorrectRequest,
    InspectOperation,
    InspectInstruction,
    RetryLater,
}
#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalMutationState {
    None,
    Committed,
    Unknown,
}
#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum LocalMutationEvidence {
    Local { mutation: LocalMutationState },
}
#[derive(Clone, Debug, schemars::JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstructionFailure {
    pub kind: InstructionFailureKind,
    pub stage: InstructionStage,
    #[schemars(length(min = 1, max = 1024))]
    pub message: String,
    pub operation_id: Option<OperationId>,
    pub instruction_id: Option<InstructionId>,
    pub effects: LocalMutationEvidence,
    pub next_action: InstructionNextAction,
}
