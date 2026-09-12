use crate::{BoardId, MessageId, ProjectId, TopicId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BoardFailureKind {
    ArchivedBoard,
    TopLevelMessageCooldown,
    ReferenceTargetNotFound,
    InvalidIdentity,
    InvalidTopicName,
    InvalidRootMessage,
    PositionBeyondLatest,
    InvalidAcknowledgement,
    InvalidCursor,
    InvalidField,
    NameConflict,
    ResourceNotFound,
    ResourceAlreadyExists,
    OutcomeUnknown,
    BoardUnavailable,
    Overloaded,
    ThreadResolved,
    InvalidRecord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BoardFailureStage {
    Validation,
    Admission,
    Storage,
    Inspection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BoardNextAction {
    CorrectRequest,
    InspectResource,
    SelectDifferentName,
    RetryLater,
    UnresolveThread,
    PostThreadMessage,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ResourceIdentity {
    #[serde(rename_all = "camelCase")]
    Project { project_id: ProjectId },
    #[serde(rename_all = "camelCase")]
    Board { board_id: BoardId },
    #[serde(rename_all = "camelCase")]
    Topic { topic_id: TopicId },
    #[serde(rename_all = "camelCase")]
    Message { message_id: MessageId },
    #[serde(rename_all = "camelCase")]
    Thread { root_message_id: MessageId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum BoardErrorDetails {
    None,
    #[serde(rename_all = "camelCase")]
    FieldConstraint {
        field: String,
        requirement: String,
    },
    #[serde(rename_all = "camelCase")]
    Resource {
        resource: ResourceIdentity,
    },
    #[serde(rename_all = "camelCase")]
    Cooldown {
        retry_after_seconds: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoardError {
    pub kind: BoardFailureKind,
    pub stage: BoardFailureStage,
    pub message: String,
    pub next_action: BoardNextAction,
    pub details: BoardErrorDetails,
}

impl BoardError {
    #[must_use]
    pub fn archived_board() -> Self {
        Self::simple(
            BoardFailureKind::ArchivedBoard,
            BoardFailureStage::Admission,
            "This board is archived and read-only.",
            BoardNextAction::CorrectRequest,
        )
    }

    #[must_use]
    pub fn thread_resolved() -> Self {
        Self::simple(
            BoardFailureKind::ThreadResolved,
            BoardFailureStage::Admission,
            "This thread is resolved. Mark it unresolved before adding a thread message.",
            BoardNextAction::UnresolveThread,
        )
    }

    #[must_use]
    pub fn top_level_message_cooldown(retry_after_seconds: u64) -> Self {
        Self {
            kind: BoardFailureKind::TopLevelMessageCooldown,
            stage: BoardFailureStage::Admission,
            message: format!(
                "Wait {retry_after_seconds} seconds or add a thread message to an unresolved thread."
            ),
            next_action: BoardNextAction::PostThreadMessage,
            details: BoardErrorDetails::Cooldown {
                retry_after_seconds,
            },
        }
    }

    #[must_use]
    pub fn invalid_field(field: impl Into<String>, requirement: impl Into<String>) -> Self {
        let field = field.into();
        let requirement = requirement.into();
        Self {
            kind: BoardFailureKind::InvalidField,
            stage: BoardFailureStage::Validation,
            message: format!("Correct {field}: {requirement}."),
            next_action: BoardNextAction::CorrectRequest,
            details: BoardErrorDetails::FieldConstraint { field, requirement },
        }
    }

    #[must_use]
    pub fn resource_not_found(resource: ResourceIdentity) -> Self {
        Self::resource_failure(
            BoardFailureKind::ResourceNotFound,
            BoardFailureStage::Inspection,
            "The requested board resource was not found. Inspect the resource ID and selected service.",
            resource,
        )
    }

    #[must_use]
    pub fn resource_already_exists(resource: ResourceIdentity) -> Self {
        Self::resource_failure(
            BoardFailureKind::ResourceAlreadyExists,
            BoardFailureStage::Admission,
            "That resource ID already exists. Inspect the existing resource; no create effects were repeated.",
            resource,
        )
    }

    #[must_use]
    pub fn invalid_record(resource: ResourceIdentity) -> Self {
        Self::resource_failure(
            BoardFailureKind::InvalidRecord,
            BoardFailureStage::Inspection,
            "Stored board data is invalid. Inspect the identified resource.",
            resource,
        )
    }

    #[must_use]
    pub fn board_unavailable() -> Self {
        Self::simple(
            BoardFailureKind::BoardUnavailable,
            BoardFailureStage::Storage,
            "Board storage is unavailable. Retry later.",
            BoardNextAction::RetryLater,
        )
    }

    fn simple(
        kind: BoardFailureKind,
        stage: BoardFailureStage,
        message: &str,
        next_action: BoardNextAction,
    ) -> Self {
        Self {
            kind,
            stage,
            message: message.to_owned(),
            next_action,
            details: BoardErrorDetails::None,
        }
    }

    fn resource_failure(
        kind: BoardFailureKind,
        stage: BoardFailureStage,
        message: &str,
        resource: ResourceIdentity,
    ) -> Self {
        Self {
            kind,
            stage,
            message: message.to_owned(),
            next_action: BoardNextAction::InspectResource,
            details: BoardErrorDetails::Resource { resource },
        }
    }
}

impl std::fmt::Display for BoardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for BoardError {}
