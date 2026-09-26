//! Persisted typed interactions. The 0.1.38 approval-history.json shape is
//! frozen and deliberately absent from this module.

use std::{collections::BTreeMap, path::PathBuf};

use message_board::{Identity, SessionRef};
use serde::{Deserialize, Serialize};
use session_event_model::{ApprovalRequest, ApprovalScope, OfferedOptionId};
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
}

impl InteractionHistoryRecord {
    #[must_use]
    pub fn request_id(&self) -> &str {
        match self {
            Self::Approval { request, .. } => &request.request_id,
        }
    }

    #[must_use]
    pub fn approval_request(&self) -> &ApprovalRequest {
        match self {
            Self::Approval { request, .. } => request,
        }
    }

    #[must_use]
    pub fn state(&self) -> &InteractionHistoryState {
        match self {
            Self::Approval { state, .. } => state,
        }
    }

    fn approver(&self) -> &Identity {
        match self {
            Self::Approval { approver, .. } => approver,
        }
    }

    fn is_valid_stored_value(&self) -> bool {
        let Self::Approval {
            requester,
            approver,
            request,
            state,
        } = self;
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
    NotPending,
    WrongActor,
    SelfApprover,
    InvalidOptionId,
    OptionNotOffered { offered: Vec<String> },
    PersistentChoiceNotAcknowledged { persistent_target: String },
    Unavailable,
}

impl std::fmt::Display for InteractionHistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "interaction history: {self:?}")
    }
}

impl std::error::Error for InteractionHistoryError {}

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
        Ok(Self {
            path,
            records: Mutex::new(records),
        })
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
        if !record.is_valid_stored_value() || record.state() != &InteractionHistoryState::Pending {
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

    pub(super) async fn list_approvals(&self, pending_only: bool) -> Vec<InteractionHistoryRecord> {
        self.records
            .lock()
            .await
            .values()
            .filter(|record| !pending_only || record.state() == &InteractionHistoryState::Pending)
            .cloned()
            .collect()
    }

    pub(super) async fn approval(&self, request_id: &str) -> Option<InteractionHistoryRecord> {
        self.records.lock().await.get(request_id).cloned()
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
        if record.state() != &InteractionHistoryState::Pending {
            return Err(InteractionHistoryError::NotPending);
        }
        let request = record.approval_request();
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
        let InteractionHistoryRecord::Approval { state, .. } = updated;
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
