//! Approvals and questions: decisions a person or agent owes a waiting session.
use super::{CollaborationRejection, CollaborationRejectionReason, PublishedRejection};
use crate::ServiceIdentity;
use crate::interaction_broker::{
    InteractionHistoryError, InteractionHistoryRecord, QuestionHistoryState,
};
use collaboration_protocol::{
    ApprovalDecideParams, ApprovalDecideResult, ApprovalListParams, ApprovalListResponse,
    QuestionAnswerParams, QuestionAnswerResult, QuestionListParams, QuestionListResult,
    QuestionRecord, QuestionResponse, QuestionState,
};
use serde_json::{Map, Value, json};

/// Approval and question operations over the Router's interaction broker.
pub struct InteractionOperations<'service> {
    identity: &'service ServiceIdentity,
}

impl<'service> InteractionOperations<'service> {
    pub(crate) fn new(identity: &'service ServiceIdentity) -> Self {
        Self { identity }
    }

    fn broker(&self) -> Result<&'service crate::ServiceInteractionBroker, InteractionFailure> {
        self.identity
            .approval_broker
            .as_deref()
            .ok_or(InteractionFailure::BrokerUnavailable)
    }

    /// Lists approvals; `includeOptions` selects the detailed records with their offered options.
    pub async fn approval_list(
        &self,
        request: ApprovalListParams,
    ) -> Result<ApprovalListResponse, InteractionFailure> {
        let broker = self.broker()?;
        if request.include_options {
            return broker
                .list_detailed(request.pending)
                .await
                .map(ApprovalListResponse::Detailed)
                .map_err(|_unavailable| InteractionFailure::ApprovalListUnavailable);
        }
        Ok(ApprovalListResponse::Legacy(
            broker.list(request.pending).await,
        ))
    }

    /// Settles one pending approval with the offered option or decision.
    pub async fn approval_decide(
        &self,
        request: ApprovalDecideParams,
    ) -> Result<ApprovalDecideResult, InteractionFailure> {
        self.broker()?
            .decide(request)
            .await
            .map_err(InteractionFailure::ApprovalRejected)
    }

    /// Lists questions, optionally only those still waiting for an answer.
    pub async fn question_list(
        &self,
        request: QuestionListParams,
    ) -> Result<QuestionListResult, InteractionFailure> {
        let records = self.broker()?.list_questions(request.pending).await;
        let questions = records
            .into_iter()
            .map(question_record_view)
            .collect::<Option<Vec<_>>>()
            .ok_or(InteractionFailure::QuestionListUnavailable)?;
        Ok(QuestionListResult { questions })
    }

    /// Answers, declines or cancels one pending question as the named actor.
    pub async fn question_answer(
        &self,
        request: QuestionAnswerParams,
    ) -> Result<QuestionAnswerResult, InteractionFailure> {
        let broker = self.broker()?;
        let state = match &request.response {
            QuestionResponse::Answered { .. } => QuestionState::Answered,
            QuestionResponse::Declined => QuestionState::Declined,
            QuestionResponse::Cancelled => QuestionState::Cancelled,
        };
        broker
            .respond_question(&request.request_id, &request.actor, request.response)
            .await
            .map_err(|failure| InteractionFailure::QuestionRejected(question_rejection(failure)))?;
        Ok(QuestionAnswerResult {
            request_id: request.request_id,
            state,
        })
    }
}

/// Why an approval or question operation failed.
#[derive(Debug, thiserror::Error)]
pub enum InteractionFailure {
    /// This Router runs without an interaction broker.
    #[error("Approval service unavailable")]
    BrokerUnavailable,
    /// The detailed approval records could not be read.
    #[error("Approval service unavailable")]
    ApprovalListUnavailable,
    /// The broker refused the decision.
    #[error("Approval decision rejected")]
    ApprovalRejected(crate::interaction_broker::ApprovalDecisionError),
    /// A stored question could not be projected.
    #[error("Question service unavailable")]
    QuestionListUnavailable,
    /// The broker refused the answer.
    #[error("Question response rejected")]
    QuestionRejected(QuestionRejection),
}

/// Why the broker refused a question answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuestionRejection {
    WrongActor,
    QuestionNotPending,
    AlreadySettled,
    InvalidAnswer { field_id: String },
    Unavailable,
}

impl QuestionRejection {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::WrongActor => "wrongActor",
            Self::QuestionNotPending => "questionNotPending",
            Self::AlreadySettled => "alreadySettled",
            Self::InvalidAnswer { .. } => "invalidAnswer",
            Self::Unavailable => "unavailable",
        }
    }
}

impl InteractionFailure {
    /// The failure's typed wire payload: `{kind, stage, message, …details}`.
    #[must_use]
    pub fn payload(&self) -> Value {
        let message = self.to_string();
        let mut payload = Map::new();
        let kind = match self {
            Self::BrokerUnavailable
            | Self::ApprovalListUnavailable
            | Self::QuestionListUnavailable => "unavailable",
            Self::ApprovalRejected(failure) => failure.code(),
            Self::QuestionRejected(rejection) => rejection.kind(),
        };
        payload.insert("kind".to_owned(), json!(kind));
        payload.insert("stage".to_owned(), json!("inspect"));
        payload.insert("message".to_owned(), json!(message));
        match self {
            Self::ApprovalRejected(failure) => {
                if let Value::Object(detail) = failure.detail() {
                    payload.extend(detail);
                }
            }
            Self::QuestionRejected(QuestionRejection::InvalidAnswer { field_id }) => {
                payload.insert("fieldId".to_owned(), json!(field_id));
            }
            Self::BrokerUnavailable
            | Self::ApprovalListUnavailable
            | Self::QuestionListUnavailable
            | Self::QuestionRejected(_) => {}
        }
        Value::Object(payload)
    }
}

impl CollaborationRejection for InteractionFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        None
    }

    fn published_rejection(&self) -> PublishedRejection {
        match self {
            // The detailed list and question projections have always failed without a payload.
            Self::ApprovalListUnavailable | Self::QuestionListUnavailable => {
                PublishedRejection::bare(PublishedRejection::OPERATION_FAILED, self.to_string())
            }
            Self::BrokerUnavailable | Self::ApprovalRejected(_) | Self::QuestionRejected(_) => {
                PublishedRejection::typed(
                    PublishedRejection::OPERATION_FAILED,
                    self.to_string(),
                    &self.payload(),
                )
            }
        }
    }
}

fn question_rejection(failure: InteractionHistoryError) -> QuestionRejection {
    match failure {
        InteractionHistoryError::WrongActor => QuestionRejection::WrongActor,
        InteractionHistoryError::NotPending => QuestionRejection::QuestionNotPending,
        InteractionHistoryError::AlreadySettled => QuestionRejection::AlreadySettled,
        InteractionHistoryError::InvalidAnswer { field_id } => {
            QuestionRejection::InvalidAnswer { field_id }
        }
        InteractionHistoryError::AlreadyExists
        | InteractionHistoryError::SelfApprover
        | InteractionHistoryError::InvalidOptionId
        | InteractionHistoryError::InvalidQuestion
        | InteractionHistoryError::OptionNotOffered { .. }
        | InteractionHistoryError::PersistentChoiceNotAcknowledged { .. }
        | InteractionHistoryError::Unavailable => QuestionRejection::Unavailable,
    }
}

fn question_record_view(record: InteractionHistoryRecord) -> Option<QuestionRecord> {
    let InteractionHistoryRecord::Question {
        requester,
        approver,
        request,
        state,
    } = record
    else {
        return None;
    };
    let requester = serde_json::to_value(requester)
        .and_then(serde_json::from_value)
        .ok()?;
    let fields = request
        .fields
        .iter()
        .map(|field| {
            serde_json::to_value(field)
                .and_then(serde_json::from_value)
                .ok()
        })
        .collect::<Option<Vec<_>>>()?;
    let state = match state {
        QuestionHistoryState::Pending => QuestionState::Pending,
        QuestionHistoryState::Answered { .. } => QuestionState::Answered,
        QuestionHistoryState::Declined => QuestionState::Declined,
        QuestionHistoryState::Cancelled { .. } => QuestionState::Cancelled,
    };
    Some(QuestionRecord {
        request_id: request.request_id,
        requester,
        approver,
        prompt: request.prompt,
        fields,
        state,
    })
}
