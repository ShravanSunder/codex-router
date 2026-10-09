//! Persisted typed interactions. The 0.1.38 approval-history.json shape is
//! frozen and deliberately absent from this module.

use std::collections::{BTreeMap, BTreeSet};

pub use collaboration_protocol::QuestionResponse;
use collaboration_protocol::{
    CodexGeneration, OperationId, ProviderIdentity, QuestionAnswerValue,
    SessionRef as ProtocolSessionRef,
};
use message_board::{Identity, SessionRef};
use serde::{Deserialize, Serialize};
use session_event_model::{
    ApprovalRequest, ApprovalScope, ApprovalSubject, InteractionCancelReason, OfferedOptionId,
    QuestionField, QuestionRequest,
};

#[path = "interaction_history_parse.rs"]
mod parse;
pub(super) use parse::parse_interaction_history_file;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum InteractionHistoryRecord {
    Approval {
        requester: SessionRef,
        approver: Identity,
        request: ApprovalRequest,
        state: InteractionHistoryState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        legacy_metadata: Option<Box<LegacyApprovalMetadata>>,
    },
    Question {
        requester: SessionRef,
        approver: Identity,
        request: QuestionRequest,
        state: QuestionHistoryState,
    },
    RefusedApproval {
        requester: SessionRef,
        approver: Identity,
        refusal: RefusedTypedApproval,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyApprovalMetadata {
    pub operation_id: OperationId,
    pub target: ProtocolSessionRef,
    pub generation: CodexGeneration,
    pub expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<ProviderIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefusedTypedApproval {
    pub request_id: String,
    pub title: String,
    pub description: Option<String>,
    pub subject: Option<ApprovalSubject>,
    pub options: Vec<RefusedApprovalOption>,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefusedApprovalOption {
    pub option_id: String,
    pub label: String,
    pub provider_kind: String,
}

impl InteractionHistoryRecord {
    #[must_use]
    pub fn request_id(&self) -> &str {
        match self {
            Self::Approval { request, .. } => &request.request_id,
            Self::Question { request, .. } => &request.request_id,
            Self::RefusedApproval { refusal, .. } => &refusal.request_id,
        }
    }

    #[must_use]
    pub fn approval_request(&self) -> Option<&ApprovalRequest> {
        match self {
            Self::Approval { request, .. } => Some(request),
            Self::Question { .. } => None,
            Self::RefusedApproval { .. } => None,
        }
    }

    #[must_use]
    pub fn approval_state(&self) -> Option<&InteractionHistoryState> {
        match self {
            Self::Approval { state, .. } => Some(state),
            Self::Question { .. } => None,
            Self::RefusedApproval { .. } => None,
        }
    }

    fn approver(&self) -> &Identity {
        match self {
            Self::Approval { approver, .. } => approver,
            Self::Question { approver, .. } => approver,
            Self::RefusedApproval { approver, .. } => approver,
        }
    }

    fn is_valid_stored_value(&self) -> bool {
        match self {
            Self::Approval {
                requester,
                approver,
                request,
                state,
                legacy_metadata,
            } => {
                if request.request_id.is_empty()
                    || matches!(approver, Identity::Session { session } if session == requester)
                    || legacy_metadata.as_ref().is_some_and(|metadata| {
                        !matches!(approver, Identity::Session { .. })
                            || serde_json::to_value(requester).ok()
                                != serde_json::to_value(&metadata.target).ok()
                            || chrono::DateTime::parse_from_rfc3339(&metadata.expires_at).is_err()
                    })
                {
                    return false;
                }
                match state {
                    InteractionHistoryState::Pending => true,
                    InteractionHistoryState::Decided { option_id } => request
                        .options
                        .iter()
                        .any(|option| &option.option_id == option_id),
                    InteractionHistoryState::Cancelled { reason } => {
                        !reason.as_str().trim().is_empty()
                    }
                }
            }
            Self::Question {
                requester,
                approver,
                request,
                state,
            } => {
                !request.request_id.is_empty()
                    && !matches!(approver, Identity::Session { session } if session == requester)
                    && valid_question_fields(request)
                    && match state {
                        QuestionHistoryState::Pending | QuestionHistoryState::Declined => true,
                        QuestionHistoryState::Cancelled { reason } => {
                            !reason.as_str().trim().is_empty()
                        }
                        QuestionHistoryState::Answered { content } => {
                            validate_question_content(request, content).is_ok()
                        }
                    }
            }
            Self::RefusedApproval { refusal, .. } => {
                !refusal.request_id.is_empty() && !refusal.reason.trim().is_empty()
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum QuestionHistoryState {
    Pending,
    Answered {
        content: BTreeMap<String, QuestionAnswerValue>,
    },
    Declined,
    Cancelled {
        reason: InteractionCancelReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum InteractionHistoryState {
    Pending,
    Decided { option_id: OfferedOptionId },
    Cancelled { reason: InteractionCancelReason },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractionHistoryError {
    AlreadyExists,
    AlreadySettled,
    NotPending,
    WrongActor,
    SelfApprover,
    InvalidOptionId,
    InvalidQuestion,
    OptionNotOffered { offered: Vec<String> },
    PersistentChoiceNotAcknowledged { persistent_target: String },
    InvalidAnswer { field_id: String },
    Unavailable,
}

impl std::fmt::Display for InteractionHistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "interaction history: {self:?}")
    }
}

impl std::error::Error for InteractionHistoryError {}

fn question_field_id(field: &QuestionField) -> &str {
    match field {
        QuestionField::Text { field_id, .. }
        | QuestionField::Number { field_id, .. }
        | QuestionField::Boolean { field_id, .. }
        | QuestionField::SingleChoice { field_id, .. }
        | QuestionField::MultiChoice { field_id, .. } => field_id,
    }
}

fn valid_question_fields(request: &QuestionRequest) -> bool {
    if session_event_model::QuestionFields::new(request.fields.iter().cloned().collect()).is_err() {
        return false;
    }
    let mut seen = BTreeSet::new();
    request.fields.iter().all(|field| {
        let field_id = question_field_id(field);
        !field_id.is_empty() && seen.insert(field_id)
    })
}

fn validate_question_content(
    request: &QuestionRequest,
    content: &BTreeMap<String, QuestionAnswerValue>,
) -> Result<(), InteractionHistoryError> {
    for field_id in content.keys() {
        if !request
            .fields
            .iter()
            .any(|field| question_field_id(field) == field_id)
        {
            return Err(InteractionHistoryError::InvalidAnswer {
                field_id: field_id.clone(),
            });
        }
    }
    for field in request.fields.iter() {
        let field_id = question_field_id(field);
        let (required, valid) = match field {
            QuestionField::Text { required, .. } => (
                *required,
                matches!(content.get(field_id), Some(QuestionAnswerValue::Text(_))),
            ),
            QuestionField::Number { required, .. } => (
                *required,
                matches!(content.get(field_id), Some(QuestionAnswerValue::Number(_))),
            ),
            QuestionField::Boolean { required, .. } => (
                *required,
                matches!(content.get(field_id), Some(QuestionAnswerValue::Boolean(_))),
            ),
            QuestionField::SingleChoice {
                required, options, ..
            } => (
                *required,
                matches!(content.get(field_id), Some(QuestionAnswerValue::SelectedOptions { selected_option_ids })
                    if selected_option_ids.len() == 1
                    && selected_option_ids.first().is_some_and(|selected| options.iter().any(|option| &option.option_id == selected))),
            ),
            QuestionField::MultiChoice {
                required,
                options,
                min,
                max,
                ..
            } => (
                *required,
                matches!(content.get(field_id), Some(QuestionAnswerValue::SelectedOptions { selected_option_ids })
                    if selected_option_ids.len() >= min.unwrap_or(usize::from(*required))
                    && max.is_none_or(|maximum| selected_option_ids.len() <= maximum)
                    && selected_option_ids.iter().collect::<BTreeSet<_>>().len() == selected_option_ids.len()
                    && selected_option_ids.iter().all(|selected| options.iter().any(|option| &option.option_id == selected))),
            ),
        };
        if (required && !valid) || (content.contains_key(field_id) && !valid) {
            return Err(InteractionHistoryError::InvalidAnswer {
                field_id: field_id.to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_question_response_record(
    record: &InteractionHistoryRecord,
    actor: &Identity,
    response: &QuestionResponse,
) -> Result<(), InteractionHistoryError> {
    let InteractionHistoryRecord::Question {
        approver,
        request,
        state,
        ..
    } = record
    else {
        return Err(InteractionHistoryError::NotPending);
    };
    if approver != actor {
        return Err(InteractionHistoryError::WrongActor);
    }
    if state != &QuestionHistoryState::Pending {
        return Err(InteractionHistoryError::AlreadySettled);
    }
    if let QuestionResponse::Answered { content } = response {
        validate_question_content(request, content)?;
    }
    Ok(())
}

#[path = "interaction_history_store.rs"]
mod store;
pub(super) use store::InteractionHistoryStore;
