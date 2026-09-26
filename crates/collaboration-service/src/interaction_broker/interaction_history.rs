//! Persisted history for typed interactions. The 0.1.38 approval-history.json
//! format is frozen and is deliberately absent from this module.

use std::{collections::BTreeMap, path::PathBuf};

use message_board::{Identity, SessionRef};
use serde::{Deserialize, Serialize};
use session_event_model::InteractionKind;
use tokio::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InteractionHistoryRecord {
    pub request_id: String,
    pub requester: SessionRef,
    pub approver: Identity,
    pub kind: InteractionKind,
    pub state: InteractionHistoryState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum InteractionHistoryState {
    Pending,
    Decided { option_id: String },
    Cancelled { reason: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionHistoryError {
    AlreadyExists,
    NotPending,
    WrongActor,
    SelfApprover,
    InvalidOptionId,
    WrongKind,
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
            request_id != &record.request_id || !record.is_valid_stored_value()
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
        if matches!(&record.approver, Identity::Session { session } if session == &record.requester)
        {
            return Err(InteractionHistoryError::SelfApprover);
        }
        if record.request_id.is_empty() || record.state != InteractionHistoryState::Pending {
            return Err(InteractionHistoryError::NotPending);
        }
        let mut records = self.records.lock().await;
        if records.contains_key(&record.request_id) {
            return Err(InteractionHistoryError::AlreadyExists);
        }
        let mut next = records.clone();
        next.insert(record.request_id.clone(), record);
        self.persist(&next).await?;
        *records = next;
        Ok(())
    }

    pub(super) async fn decide(
        &self,
        request_id: &str,
        actor: &Identity,
        option_id: &str,
    ) -> Result<(), InteractionHistoryError> {
        if option_id.trim().is_empty() {
            return Err(InteractionHistoryError::InvalidOptionId);
        }
        let mut records = self.records.lock().await;
        let record = records
            .get(request_id)
            .ok_or(InteractionHistoryError::NotPending)?;
        if &record.approver != actor {
            return Err(InteractionHistoryError::WrongActor);
        }
        if record.kind != InteractionKind::Approval {
            return Err(InteractionHistoryError::WrongKind);
        }
        if record.state != InteractionHistoryState::Pending {
            return Err(InteractionHistoryError::NotPending);
        }
        let mut next = records.clone();
        let updated = next
            .get_mut(request_id)
            .ok_or(InteractionHistoryError::NotPending)?;
        updated.state = InteractionHistoryState::Decided {
            option_id: option_id.to_owned(),
        };
        self.persist(&next).await?;
        *records = next;
        Ok(())
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

impl InteractionHistoryRecord {
    fn is_valid_stored_value(&self) -> bool {
        if self.request_id.is_empty()
            || matches!(&self.approver, Identity::Session { session } if session == &self.requester)
        {
            return false;
        }
        match &self.state {
            InteractionHistoryState::Pending => true,
            InteractionHistoryState::Decided { option_id } => {
                self.kind == InteractionKind::Approval && !option_id.trim().is_empty()
            }
            InteractionHistoryState::Cancelled { reason } => !reason.trim().is_empty(),
        }
    }
}
