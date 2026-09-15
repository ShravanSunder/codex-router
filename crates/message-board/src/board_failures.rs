use crate::{
    ActivitySequence, BoardId, Identity, MessageId, MessageText, ParticipantRole, ProjectId,
    TopicId,
};
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
    ParticipantRequired,
    OrchestratorRequired,
    OrchestratorAlreadyExists,
    OrchestratorHandoverRequired,
    HandoverTargetNotParticipant,
    StaleOrchestrator,
    SelfReplace,
    OrchestratorRoleChange,
    SessionTopicPost,
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
    CreateThread,
    JoinThread,
    ReplaceOrchestrator,
    LeaveWithHandoverOrResolve,
    JoinHandoverTarget,
    InspectParticipants,
    RepeatJoinWithoutReplace,
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
    #[serde(rename_all = "camelCase")]
    ParticipantRefusal {
        #[serde(flatten)]
        refusal: Box<ParticipantRefusalDetails>,
    },
    #[serde(rename_all = "camelCase")]
    ThreadCreateRefusal {
        #[serde(flatten)]
        refusal: Box<ThreadCreateRefusalDetails>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParticipantRefusalDetails {
    pub root_message_id: MessageId,
    pub missing_root_message_ids: Vec<MessageId>,
    pub actor: Identity,
    pub allowed_roles: Vec<ParticipantRole>,
    pub holder: Option<Identity>,
    pub holder_last_seen_activity: Option<ActivitySequence>,
    pub target: Option<Identity>,
    pub named_holder: Option<Identity>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadCreateRefusalDetails {
    pub topic_id: TopicId,
    pub actor: Identity,
    pub text: MessageText,
    pub allowed_roles: Vec<ParticipantRole>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParticipantRefusal {
    pub kind: BoardFailureKind,
    pub message: String,
    pub next_action: BoardNextAction,
    pub root_message_id: MessageId,
    pub actor: Identity,
    pub holder: Option<Identity>,
    pub holder_last_seen_activity: Option<ActivitySequence>,
    pub target: Option<Identity>,
    pub named_holder: Option<Identity>,
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
    fn all_roles() -> Vec<ParticipantRole> {
        vec![
            ParticipantRole::Orchestrator,
            ParticipantRole::Advisor,
            ParticipantRole::Reviewer,
            ParticipantRole::Participant,
        ]
    }

    #[must_use]
    pub fn participant_required(root_message_id: MessageId, actor: Identity) -> Self {
        Self::participants_required(root_message_id.clone(), vec![root_message_id], actor)
    }

    #[must_use]
    pub fn participants_required(
        root_message_id: MessageId,
        missing_root_message_ids: Vec<MessageId>,
        actor: Identity,
    ) -> Self {
        Self {
            kind: BoardFailureKind::ParticipantRequired,
            stage: BoardFailureStage::Admission,
            message: "This session must Join the Thread before continuing.".to_owned(),
            next_action: BoardNextAction::JoinThread,
            details: BoardErrorDetails::ParticipantRefusal {
                refusal: Box::new(ParticipantRefusalDetails {
                    root_message_id,
                    missing_root_message_ids,
                    actor,
                    allowed_roles: Self::all_roles(),
                    holder: None,
                    holder_last_seen_activity: None,
                    target: None,
                    named_holder: None,
                }),
            },
        }
    }

    #[must_use]
    pub fn session_topic_post(topic_id: TopicId, actor: Identity, text: MessageText) -> Self {
        Self {
            kind: BoardFailureKind::SessionTopicPost,
            stage: BoardFailureStage::Admission,
            message: "A session creates a Thread with thread create and an explicit Role and Watch choice.".to_owned(),
            next_action: BoardNextAction::CreateThread,
            details: BoardErrorDetails::ThreadCreateRefusal {
                refusal: Box::new(ThreadCreateRefusalDetails {
                    topic_id,
                    actor,
                    text,
                    allowed_roles: Self::all_roles(),
                }),
            },
        }
    }

    #[must_use]
    pub fn orchestrator_already_exists(
        root_message_id: MessageId,
        actor: Identity,
        holder: Identity,
        holder_last_seen_activity: ActivitySequence,
    ) -> Self {
        Self {
            kind: BoardFailureKind::OrchestratorAlreadyExists,
            stage: BoardFailureStage::Admission,
            message: "This Thread already has an Orchestrator. Name that holder explicitly to Replace it.".to_owned(),
            next_action: BoardNextAction::ReplaceOrchestrator,
            details: BoardErrorDetails::ParticipantRefusal {
                refusal: Box::new(ParticipantRefusalDetails {
                root_message_id,
                missing_root_message_ids: Vec::new(),
                actor,
                allowed_roles: vec![ParticipantRole::Orchestrator],
                holder: Some(holder),
                holder_last_seen_activity: Some(holder_last_seen_activity),
                target: None,
                named_holder: None,
                }),
            },
        }
    }

    #[must_use]
    pub fn participant_refusal(refusal: ParticipantRefusal) -> Self {
        Self {
            kind: refusal.kind,
            stage: BoardFailureStage::Admission,
            message: refusal.message,
            next_action: refusal.next_action,
            details: BoardErrorDetails::ParticipantRefusal {
                refusal: Box::new(ParticipantRefusalDetails {
                    root_message_id: refusal.root_message_id,
                    missing_root_message_ids: Vec::new(),
                    actor: refusal.actor,
                    allowed_roles: Self::all_roles(),
                    holder: refusal.holder,
                    holder_last_seen_activity: refusal.holder_last_seen_activity,
                    target: refusal.target,
                    named_holder: refusal.named_holder,
                }),
            },
        }
    }
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
                "Wait {retry_after_seconds} seconds before another top-level message in this board, or add a thread message to an existing unresolved thread."
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
