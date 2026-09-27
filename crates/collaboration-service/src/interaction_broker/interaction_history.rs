//! Persisted typed interactions. The 0.1.38 approval-history.json shape is
//! frozen and deliberately absent from this module.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use collaboration_protocol::QuestionAnswerValue;
pub use collaboration_protocol::QuestionResponse;
use message_board::{Identity, SessionRef};
use serde::{Deserialize, Serialize};
use session_event_model::{
    ApprovalRequest, ApprovalScope, OfferedOptionId, QuestionField, QuestionRequest,
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum InteractionHistoryRecord {
    Approval {
        requester: SessionRef,
        approver: Identity,
        request: ApprovalRequest,
        state: InteractionHistoryState,
    },
    Question {
        requester: SessionRef,
        approver: Identity,
        request: QuestionRequest,
        state: QuestionHistoryState,
    },
}

impl InteractionHistoryRecord {
    #[must_use]
    pub fn request_id(&self) -> &str {
        match self {
            Self::Approval { request, .. } => &request.request_id,
            Self::Question { request, .. } => &request.request_id,
        }
    }

    #[must_use]
    pub fn approval_request(&self) -> Option<&ApprovalRequest> {
        match self {
            Self::Approval { request, .. } => Some(request),
            Self::Question { .. } => None,
        }
    }

    #[must_use]
    pub fn approval_state(&self) -> Option<&InteractionHistoryState> {
        match self {
            Self::Approval { state, .. } => Some(state),
            Self::Question { .. } => None,
        }
    }

    fn approver(&self) -> &Identity {
        match self {
            Self::Approval { approver, .. } => approver,
            Self::Question { approver, .. } => approver,
        }
    }

    fn is_valid_stored_value(&self) -> bool {
        match self {
            Self::Approval {
                requester,
                approver,
                request,
                state,
            } => {
                if request.request_id.is_empty()
                    || matches!(approver, Identity::Session { session } if session == requester)
                {
                    return false;
                }
                match state {
                    InteractionHistoryState::Pending => true,
                    InteractionHistoryState::Decided { option_id } => request
                        .options
                        .iter()
                        .any(|option| &option.option_id == option_id),
                    InteractionHistoryState::Cancelled { reason } => !reason.trim().is_empty(),
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
                        QuestionHistoryState::Cancelled { reason } => !reason.trim().is_empty(),
                        QuestionHistoryState::Answered { content } => {
                            validate_question_content(request, content).is_ok()
                        }
                    }
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
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum InteractionHistoryState {
    Pending,
    Decided { option_id: OfferedOptionId },
    Cancelled { reason: String },
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
        | QuestionField::SingleChoice { field_id, .. } => field_id,
    }
}

fn valid_question_fields(request: &QuestionRequest) -> bool {
    let mut seen = BTreeSet::new();
    request.fields.iter().all(|field| {
        let field_id = question_field_id(field);
        !field_id.is_empty()
            && seen.insert(field_id)
            && !matches!(field, QuestionField::SingleChoice { options, .. } if options.is_empty())
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
                matches!(content.get(field_id), Some(QuestionAnswerValue::Text(choice)) if options.iter().any(|option| option == choice)),
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

pub(super) struct InteractionHistoryStore {
    path: PathBuf,
    records: Mutex<BTreeMap<String, InteractionHistoryRecord>>,
}

impl InteractionHistoryStore {
    pub(super) async fn load(path: PathBuf) -> Result<Self, InteractionHistoryError> {
        let records: BTreeMap<String, InteractionHistoryRecord> =
            match tokio::fs::read(&path).await {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|_| InteractionHistoryError::Unavailable)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
                Err(_) => return Err(InteractionHistoryError::Unavailable),
            };
        if records.iter().any(|(request_id, record)| {
            request_id != record.request_id() || !record.is_valid_stored_value()
        }) {
            return Err(InteractionHistoryError::Unavailable);
        }
        let store = Self {
            path,
            records: Mutex::new(records),
        };
        let mut recovered = store.records.lock().await.clone();
        let mut had_pending = false;
        for record in recovered.values_mut() {
            match record {
                InteractionHistoryRecord::Approval { state, .. }
                    if state == &InteractionHistoryState::Pending =>
                {
                    *state = InteractionHistoryState::Cancelled {
                        reason: "hostRestarted".to_owned(),
                    };
                    had_pending = true;
                }
                InteractionHistoryRecord::Question { state, .. }
                    if state == &QuestionHistoryState::Pending =>
                {
                    *state = QuestionHistoryState::Cancelled {
                        reason: "hostRestarted".to_owned(),
                    };
                    had_pending = true;
                }
                _ => {}
            }
        }
        if had_pending {
            store.persist(&recovered).await?;
            *store.records.lock().await = recovered;
        }
        Ok(store)
    }

    pub(super) async fn record(
        &self,
        record: InteractionHistoryRecord,
    ) -> Result<(), InteractionHistoryError> {
        if let InteractionHistoryRecord::Approval {
            requester,
            approver: Identity::Session { session },
            ..
        } = &record
            && session == requester
        {
            return Err(InteractionHistoryError::SelfApprover);
        }
        if !record.is_valid_stored_value()
            || record.approval_state() != Some(&InteractionHistoryState::Pending)
        {
            return Err(InteractionHistoryError::NotPending);
        }
        let mut records = self.records.lock().await;
        if records.contains_key(record.request_id()) {
            return Err(InteractionHistoryError::AlreadyExists);
        }
        let mut next = records.clone();
        next.insert(record.request_id().to_owned(), record);
        self.persist(&next).await?;
        *records = next;
        Ok(())
    }

    pub(super) async fn record_cancelled_approval(
        &self,
        requester: SessionRef,
        approver: Identity,
        request: ApprovalRequest,
        reason: &str,
    ) -> Result<(), InteractionHistoryError> {
        let record = InteractionHistoryRecord::Approval {
            requester,
            approver,
            request,
            state: InteractionHistoryState::Cancelled {
                reason: reason.to_owned(),
            },
        };
        if !record.is_valid_stored_value() {
            return Err(InteractionHistoryError::Unavailable);
        }
        let mut records = self.records.lock().await;
        if records.contains_key(record.request_id()) {
            return Err(InteractionHistoryError::AlreadyExists);
        }
        let mut next = records.clone();
        next.insert(record.request_id().to_owned(), record);
        self.persist(&next).await?;
        *records = next;
        Ok(())
    }

    pub(super) async fn cancel_approval(
        &self,
        request_id: &str,
        reason: &str,
    ) -> Result<(), InteractionHistoryError> {
        if reason.trim().is_empty() {
            return Err(InteractionHistoryError::Unavailable);
        }
        let mut records = self.records.lock().await;
        let mut next = records.clone();
        let Some(InteractionHistoryRecord::Approval { state, .. }) = next.get_mut(request_id)
        else {
            return Err(InteractionHistoryError::NotPending);
        };
        if state != &InteractionHistoryState::Pending {
            return Err(InteractionHistoryError::AlreadySettled);
        }
        *state = InteractionHistoryState::Cancelled {
            reason: reason.to_owned(),
        };
        self.persist(&next).await?;
        *records = next;
        Ok(())
    }

    pub(super) async fn cancel_approvals(
        &self,
        requester: &SessionRef,
        reason: &str,
    ) -> Result<Vec<String>, InteractionHistoryError> {
        if reason.trim().is_empty() {
            return Err(InteractionHistoryError::Unavailable);
        }
        let mut records = self.records.lock().await;
        let mut next = records.clone();
        let mut cancelled = Vec::new();
        for (request_id, record) in &mut next {
            if let InteractionHistoryRecord::Approval {
                requester: owner,
                state,
                ..
            } = record
                && owner == requester
                && state == &InteractionHistoryState::Pending
            {
                *state = InteractionHistoryState::Cancelled {
                    reason: reason.to_owned(),
                };
                cancelled.push(request_id.clone());
            }
        }
        if cancelled.is_empty() {
            return Ok(cancelled);
        }
        self.persist(&next).await?;
        *records = next;
        Ok(cancelled)
    }

    pub(super) async fn list_approvals(&self, pending_only: bool) -> Vec<InteractionHistoryRecord> {
        self.records
            .lock()
            .await
            .values()
            .filter(|record| matches!(record, InteractionHistoryRecord::Approval { state, .. } if !pending_only || state == &InteractionHistoryState::Pending))
            .cloned()
            .collect()
    }

    pub(super) async fn interaction(&self, request_id: &str) -> Option<InteractionHistoryRecord> {
        self.records.lock().await.get(request_id).cloned()
    }

    pub(super) async fn record_question(
        &self,
        requester: SessionRef,
        approver: Identity,
        request: QuestionRequest,
    ) -> Result<(), InteractionHistoryError> {
        if matches!(&approver, Identity::Session { session } if session == &requester) {
            return Err(InteractionHistoryError::SelfApprover);
        }
        let record = InteractionHistoryRecord::Question {
            requester,
            approver,
            request,
            state: QuestionHistoryState::Pending,
        };
        if !record.is_valid_stored_value() {
            return Err(InteractionHistoryError::InvalidQuestion);
        }
        let mut records = self.records.lock().await;
        if records.contains_key(record.request_id()) {
            return Err(InteractionHistoryError::AlreadyExists);
        }
        let mut next = records.clone();
        next.insert(record.request_id().to_owned(), record);
        self.persist(&next).await?;
        *records = next;
        Ok(())
    }

    pub(super) async fn list_questions(&self, pending_only: bool) -> Vec<InteractionHistoryRecord> {
        self.records.lock().await.values()
            .filter(|record| matches!(record, InteractionHistoryRecord::Question { state, .. } if !pending_only || state == &QuestionHistoryState::Pending))
            .cloned().collect()
    }

    pub(super) async fn respond_question(
        &self,
        request_id: &str,
        actor: &Identity,
        response: &QuestionResponse,
    ) -> Result<(), InteractionHistoryError> {
        let mut records = self.records.lock().await;
        let record = records
            .get(request_id)
            .ok_or(InteractionHistoryError::NotPending)?;
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
        let mut next = records.clone();
        let Some(InteractionHistoryRecord::Question { state, .. }) = next.get_mut(request_id)
        else {
            return Err(InteractionHistoryError::NotPending);
        };
        *state = match response {
            QuestionResponse::Answered { content } => QuestionHistoryState::Answered {
                content: content.clone(),
            },
            QuestionResponse::Declined => QuestionHistoryState::Declined,
            QuestionResponse::Cancelled => QuestionHistoryState::Cancelled {
                reason: "approver cancelled".to_owned(),
            },
        };
        self.persist(&next).await?;
        *records = next;
        Ok(())
    }

    pub(super) async fn cancel_questions(
        &self,
        requester: &SessionRef,
        reason: &str,
    ) -> Result<Vec<String>, InteractionHistoryError> {
        if reason.trim().is_empty() {
            return Err(InteractionHistoryError::Unavailable);
        }
        let mut records = self.records.lock().await;
        let mut next = records.clone();
        let mut cancelled = Vec::new();
        for (request_id, record) in &mut next {
            if let InteractionHistoryRecord::Question {
                requester: owner,
                state,
                ..
            } = record
                && owner == requester
                && state == &QuestionHistoryState::Pending
            {
                *state = QuestionHistoryState::Cancelled {
                    reason: reason.to_owned(),
                };
                cancelled.push(request_id.clone());
            }
        }
        if cancelled.is_empty() {
            return Ok(cancelled);
        }
        self.persist(&next).await?;
        *records = next;
        Ok(cancelled)
    }

    pub(super) async fn cancel_question(
        &self,
        request_id: &str,
        reason: &str,
    ) -> Result<(), InteractionHistoryError> {
        if reason.trim().is_empty() {
            return Err(InteractionHistoryError::Unavailable);
        }
        let mut records = self.records.lock().await;
        let mut next = records.clone();
        let Some(InteractionHistoryRecord::Question { state, .. }) = next.get_mut(request_id)
        else {
            return Err(InteractionHistoryError::NotPending);
        };
        if state != &QuestionHistoryState::Pending {
            return Err(InteractionHistoryError::AlreadySettled);
        }
        *state = QuestionHistoryState::Cancelled {
            reason: reason.to_owned(),
        };
        self.persist(&next).await?;
        *records = next;
        Ok(())
    }

    pub(super) async fn decide(
        &self,
        request_id: &str,
        actor: &Identity,
        option_id: &str,
        acknowledge_persistent: bool,
    ) -> Result<OfferedOptionId, InteractionHistoryError> {
        if option_id.is_empty() {
            return Err(InteractionHistoryError::InvalidOptionId);
        }
        let mut records = self.records.lock().await;
        let record = records
            .get(request_id)
            .ok_or(InteractionHistoryError::NotPending)?;
        if record.approver() != actor {
            return Err(InteractionHistoryError::WrongActor);
        }
        if record.approval_state() != Some(&InteractionHistoryState::Pending) {
            return Err(InteractionHistoryError::AlreadySettled);
        }
        let request = record
            .approval_request()
            .ok_or(InteractionHistoryError::NotPending)?;
        let option = request
            .options
            .iter()
            .find(|option| option.option_id.as_str() == option_id)
            .ok_or_else(|| InteractionHistoryError::OptionNotOffered {
                offered: request
                    .options
                    .iter()
                    .map(|option| option.option_id.as_str().to_owned())
                    .collect(),
            })?;
        if let ApprovalScope::Persistent { where_stored } = &option.choice.scope
            && !acknowledge_persistent
        {
            return Err(InteractionHistoryError::PersistentChoiceNotAcknowledged {
                persistent_target: where_stored.as_str().to_owned(),
            });
        }
        let selected = option.option_id.clone();
        let mut next = records.clone();
        let updated = next
            .get_mut(request_id)
            .ok_or(InteractionHistoryError::NotPending)?;
        let InteractionHistoryRecord::Approval { state, .. } = updated else {
            return Err(InteractionHistoryError::NotPending);
        };
        *state = InteractionHistoryState::Decided {
            option_id: selected.clone(),
        };
        self.persist(&next).await?;
        *records = next;
        Ok(selected)
    }

    async fn persist(
        &self,
        records: &BTreeMap<String, InteractionHistoryRecord>,
    ) -> Result<(), InteractionHistoryError> {
        let bytes =
            serde_json::to_vec_pretty(records).map_err(|_| InteractionHistoryError::Unavailable)?;
        let temporary = self.path.with_extension("json.tmp");
        tokio::fs::write(&temporary, bytes)
            .await
            .map_err(|_| InteractionHistoryError::Unavailable)?;
        tokio::fs::rename(&temporary, &self.path)
            .await
            .map_err(|_| InteractionHistoryError::Unavailable)
    }
}
