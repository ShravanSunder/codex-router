//! Actionable schedule failures preserve local mutation uncertainty and stale-edit identity.
use crate::OperationId;
use agent_automation::{ChangeId, ScheduleId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleFailureKind {
    InvalidField,
    OperationConflict,
    OutcomeUnknown,
    ResourceNotFound,
    ChangeConflict,
    AutomationUnavailable,
    InvalidRecord,
    UnsupportedCapability,
    OwnershipConflict,
    InstructionConflict,
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
    pub details: ScheduleFailureDetails,
    pub effects: ScheduleEffects,
    pub next_action: ScheduleNextAction,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ScheduleFailureDetails {
    None,
    InstructionConflict {
        instruction_id: crate::InstructionId,
        schedule_ids: Vec<ScheduleId>,
    },
    FrameLimit {
        encoded_bytes: usize,
        maximum_bytes: usize,
    },
}

impl ScheduleFailure {
    #[must_use]
    pub fn package_frame_limit(operation_id: Option<OperationId>, encoded_bytes: usize) -> Self {
        Self {
            kind: ScheduleFailureKind::InvalidField,
            stage: ScheduleFailureStage::Validation,
            message: "The encoded package and Control envelope exceed the frame limit; no package mutation was submitted.".into(),
            operation_id,
            schedule_id: None,
            current_change_id: None,
            field: Some("packageUtf8".into()),
            constraint: Some("The complete encoded Control frame must fit within 1048576 bytes.".into()),
            details: ScheduleFailureDetails::FrameLimit { encoded_bytes, maximum_bytes: crate::MAX_CONTROL_FRAME_BYTES },
            effects: ScheduleEffects::Local { mutation: crate::LocalMutationState::None },
            next_action: ScheduleNextAction::CorrectRequest,
        }
    }
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ScheduleEffects {
    Local {
        mutation: crate::LocalMutationState,
    },
    Native {
        evidence: crate::NativeEffectEvidence,
    },
}
