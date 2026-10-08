//! Serialized typed-history transactions and acknowledged cache with creation-time retention.
use super::*;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    path::PathBuf,
};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct InteractionHistoryData {
    records: BTreeMap<String, InteractionHistoryRecord>,
    created_at: BTreeMap<String, DateTime<Utc>>,
}

impl Deref for InteractionHistoryData {
    type Target = BTreeMap<String, InteractionHistoryRecord>;

    fn deref(&self) -> &Self::Target {
        &self.records
    }
}

impl DerefMut for InteractionHistoryData {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.records
    }
}

impl InteractionHistoryData {
    fn insert_with_timestamp(
        &mut self,
        request_id: String,
        record: InteractionHistoryRecord,
        created_at: DateTime<Utc>,
    ) {
        self.records.insert(request_id.clone(), record);
        self.created_at.insert(request_id, created_at);
    }
}

pub(in crate::interaction_broker) struct InteractionHistoryStore {
    data: Mutex<HistoryDatabaseState>,
    #[cfg(test)]
    cache_publication_pause: Mutex<Option<CachePublicationPause>>,
}

fn parse_history_timestamp(value: &str) -> Result<DateTime<Utc>, InteractionHistoryError> {
    if !value.ends_with('Z') {
        return Err(InteractionHistoryError::Unavailable);
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| InteractionHistoryError::Unavailable)
}

#[path = "interaction_history_codec.rs"]
mod interaction_history_codec;
#[path = "interaction_history_database.rs"]
mod interaction_history_database;
use interaction_history_database::{
    HistoryDatabaseState, commit_delta, initialize_history, read_metadata, read_records,
};
use sqlx::Connection;

#[cfg(test)]
struct CachePublicationPause {
    committed: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

impl InteractionHistoryStore {
    pub(in crate::interaction_broker) async fn load(
        path: PathBuf,
    ) -> Result<Self, InteractionHistoryError> {
        let state = initialize_history(&path.with_file_name("interaction.sqlite"), &path)
            .await
            .map_err(|failure| failure.public_error(true))?;
        Ok(Self {
            data: Mutex::new(state),
            #[cfg(test)]
            cache_publication_pause: Mutex::new(None),
        })
    }

    pub(in crate::interaction_broker) async fn reconcile_pending_on_startup(
        &self,
    ) -> Result<(), InteractionHistoryError> {
        self.mutate_history(|records| {
            let mut next = records.clone();
            for record in next.records.values_mut() {
                match record {
                    InteractionHistoryRecord::Approval { state, .. }
                        if *state == InteractionHistoryState::Pending =>
                    {
                        *state = InteractionHistoryState::Cancelled {
                            reason: InteractionCancelReason::HostRestarted,
                        };
                    }
                    InteractionHistoryRecord::Question { state, .. }
                        if *state == QuestionHistoryState::Pending =>
                    {
                        *state = QuestionHistoryState::Cancelled {
                            reason: InteractionCancelReason::HostRestarted,
                        };
                    }
                    _ => {}
                }
            }
            Ok((next, ()))
        })
        .await
    }

    async fn mutate_history<MutationResult>(
        &self,
        stage: impl FnOnce(
            &InteractionHistoryData,
        )
            -> Result<(InteractionHistoryData, MutationResult), InteractionHistoryError>,
    ) -> Result<MutationResult, InteractionHistoryError> {
        let mut state = self.data.lock().await;
        let previous_revision = state.revision;
        let previous_cache = state.cache.clone();
        let mut transaction = state
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|_| {
                interaction_history_database::HistoryStorageFailure::StorageUnavailable
                    .public_error(false)
            })?;
        let metadata = read_metadata(&mut transaction)
            .await
            .map_err(|failure| failure.public_error(false))?;
        let baseline = if metadata.revision == previous_revision {
            previous_cache
        } else {
            read_records(&mut transaction)
                .await
                .map_err(|failure| failure.public_error(false))?
        };
        let (next, outcome) = match stage(&baseline) {
            Ok((next, outcome)) => (next, Ok(outcome)),
            Err(error) => (baseline.clone(), Err(error)),
        };
        let revision = if outcome.is_ok() {
            commit_delta(&mut transaction, &baseline, &next, metadata.revision)
                .await
                .map_err(|failure| failure.public_error(false))?
        } else {
            metadata.revision
        };
        transaction.commit().await.map_err(|_| {
            interaction_history_database::HistoryStorageFailure::StorageUnavailable
                .public_error(false)
        })?;
        // This test-only pause proves durable commit without cache publication. Production has no await here.
        #[cfg(test)]
        if let Some(pause) = self.cache_publication_pause.lock().await.take() {
            let _ = pause.committed.send(());
            let _ = pause.resume.await;
        }
        state.cache = next;
        state.revision = revision;
        outcome
    }
    pub(in crate::interaction_broker) async fn record(
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
        self.mutate_history(|records| {
            if records.contains_key(record.request_id()) {
                return Err(InteractionHistoryError::AlreadyExists);
            }
            let mut next = records.clone();
            next.insert_with_timestamp(record.request_id().to_owned(), record, Utc::now());
            Ok((next, ()))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn record_refused_approval(
        &self,
        requester: SessionRef,
        approver: Identity,
        refusal: RefusedTypedApproval,
    ) -> Result<(), InteractionHistoryError> {
        let record = InteractionHistoryRecord::RefusedApproval {
            requester,
            approver,
            refusal,
        };
        if !record.is_valid_stored_value() {
            return Err(InteractionHistoryError::Unavailable);
        }
        self.mutate_history(|records| {
            if records.contains_key(record.request_id()) {
                return Err(InteractionHistoryError::AlreadyExists);
            }
            let mut next = records.clone();
            next.insert_with_timestamp(record.request_id().to_owned(), record, Utc::now());
            Ok((next, ()))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn record_cancelled_approval(
        &self,
        requester: SessionRef,
        approver: Identity,
        request: ApprovalRequest,
        reason: &str,
        legacy_metadata: Option<Box<LegacyApprovalMetadata>>,
    ) -> Result<(), InteractionHistoryError> {
        let record = InteractionHistoryRecord::Approval {
            requester,
            approver,
            request,
            state: InteractionHistoryState::Cancelled {
                reason: reason.to_owned().into(),
            },
            legacy_metadata,
        };
        if !record.is_valid_stored_value() {
            return Err(InteractionHistoryError::Unavailable);
        }
        self.mutate_history(|records| {
            if records.contains_key(record.request_id()) {
                return Err(InteractionHistoryError::AlreadyExists);
            }
            let mut next = records.clone();
            next.insert_with_timestamp(record.request_id().to_owned(), record, Utc::now());
            Ok((next, ()))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn cancel_approval(
        &self,
        request_id: &str,
        reason: &str,
    ) -> Result<(), InteractionHistoryError> {
        if reason.trim().is_empty() {
            return Err(InteractionHistoryError::Unavailable);
        }
        self.mutate_history(|records| {
            let mut next = records.clone();
            let Some(InteractionHistoryRecord::Approval { state, .. }) = next.get_mut(request_id)
            else {
                return Err(InteractionHistoryError::NotPending);
            };
            if state != &InteractionHistoryState::Pending {
                return Err(InteractionHistoryError::AlreadySettled);
            }
            *state = InteractionHistoryState::Cancelled {
                reason: reason.to_owned().into(),
            };
            Ok((next, ()))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn cancel_approval_as_approver(
        &self,
        request_id: &str,
        actor: &Identity,
    ) -> Result<(), InteractionHistoryError> {
        self.mutate_history(|records| {
            let record = records
                .get(request_id)
                .ok_or(InteractionHistoryError::NotPending)?;
            let InteractionHistoryRecord::Approval {
                approver, state, ..
            } = record
            else {
                return Err(InteractionHistoryError::NotPending);
            };
            if approver != actor {
                return Err(InteractionHistoryError::WrongActor);
            }
            if state != &InteractionHistoryState::Pending {
                return Err(InteractionHistoryError::AlreadySettled);
            }
            let mut next = records.clone();
            let Some(InteractionHistoryRecord::Approval { state, .. }) = next.get_mut(request_id)
            else {
                return Err(InteractionHistoryError::NotPending);
            };
            *state = InteractionHistoryState::Cancelled {
                reason: InteractionCancelReason::ApproverCancelled,
            };
            Ok((next, ()))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn cancel_approvals(
        &self,
        requester: &SessionRef,
        reason: &str,
    ) -> Result<Vec<String>, InteractionHistoryError> {
        if reason.trim().is_empty() {
            return Err(InteractionHistoryError::Unavailable);
        }
        self.mutate_history(|records| {
            let mut next = records.clone();
            let mut cancelled = Vec::new();
            for (request_id, record) in &mut next.records {
                if let InteractionHistoryRecord::Approval {
                    requester: owner,
                    state,
                    ..
                } = record
                    && owner == requester
                    && state == &InteractionHistoryState::Pending
                {
                    *state = InteractionHistoryState::Cancelled {
                        reason: reason.to_owned().into(),
                    };
                    cancelled.push(request_id.clone());
                }
            }
            if cancelled.is_empty() {
                return Ok((records.clone(), cancelled));
            }
            Ok((next, cancelled))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn list_approvals(
        &self,
        pending_only: bool,
    ) -> Vec<InteractionHistoryRecord> {
        self.data
            .lock()
            .await
            .values()
            .filter(|record| match record {
                InteractionHistoryRecord::Approval { state, .. } => {
                    !pending_only || state == &InteractionHistoryState::Pending
                }
                InteractionHistoryRecord::RefusedApproval { .. } => !pending_only,
                InteractionHistoryRecord::Question { .. } => false,
            })
            .cloned()
            .collect()
    }

    pub(in crate::interaction_broker) async fn interaction(
        &self,
        request_id: &str,
    ) -> Option<InteractionHistoryRecord> {
        self.data.lock().await.get(request_id).cloned()
    }

    pub(in crate::interaction_broker) async fn list_all(&self) -> Vec<InteractionHistoryRecord> {
        self.data.lock().await.values().cloned().collect()
    }

    pub(in crate::interaction_broker) async fn record_question(
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
        self.mutate_history(|records| {
            if records.contains_key(record.request_id()) {
                return Err(InteractionHistoryError::AlreadyExists);
            }
            let mut next = records.clone();
            next.insert_with_timestamp(record.request_id().to_owned(), record, Utc::now());
            Ok((next, ()))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn record_cancelled_question(
        &self,
        requester: SessionRef,
        approver: Identity,
        request: QuestionRequest,
        reason: &str,
    ) -> Result<(), InteractionHistoryError> {
        let record = InteractionHistoryRecord::Question {
            requester,
            approver,
            request,
            state: QuestionHistoryState::Cancelled {
                reason: reason.to_owned().into(),
            },
        };
        if !record.is_valid_stored_value() {
            return Err(InteractionHistoryError::InvalidQuestion);
        }
        self.mutate_history(|records| {
            if records.contains_key(record.request_id()) {
                return Err(InteractionHistoryError::AlreadyExists);
            }
            let mut next = records.clone();
            next.insert_with_timestamp(record.request_id().to_owned(), record, Utc::now());
            Ok((next, ()))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn list_questions(
        &self,
        pending_only: bool,
    ) -> Vec<InteractionHistoryRecord> {
        self.data.lock().await.values()
            .filter(|record| matches!(record, InteractionHistoryRecord::Question { state, .. } if !pending_only || state == &QuestionHistoryState::Pending))
            .cloned().collect()
    }

    pub(in crate::interaction_broker) async fn respond_question(
        &self,
        request_id: &str,
        actor: &Identity,
        response: &QuestionResponse,
    ) -> Result<(), InteractionHistoryError> {
        self.mutate_history(|records| {
            let record = records
                .get(request_id)
                .ok_or(InteractionHistoryError::NotPending)?;
            validate_question_response_record(record, actor, response)?;
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
                    reason: InteractionCancelReason::ApproverCancelled,
                },
            };
            Ok((next, ()))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn validate_question_response(
        &self,
        request_id: &str,
        actor: &Identity,
        response: &QuestionResponse,
    ) -> Result<(), InteractionHistoryError> {
        self.mutate_history(|records| {
            let record = records
                .get(request_id)
                .ok_or(InteractionHistoryError::NotPending)?;
            validate_question_response_record(record, actor, response)?;
            Ok((records.clone(), ()))
        })
        .await
    }
    pub(in crate::interaction_broker) async fn cancel_questions(
        &self,
        requester: &SessionRef,
        reason: &str,
    ) -> Result<Vec<String>, InteractionHistoryError> {
        if reason.trim().is_empty() {
            return Err(InteractionHistoryError::Unavailable);
        }
        self.mutate_history(|records| {
            let mut next = records.clone();
            let mut cancelled = Vec::new();
            for (request_id, record) in &mut next.records {
                if let InteractionHistoryRecord::Question {
                    requester: owner,
                    state,
                    ..
                } = record
                    && owner == requester
                    && state == &QuestionHistoryState::Pending
                {
                    *state = QuestionHistoryState::Cancelled {
                        reason: reason.to_owned().into(),
                    };
                    cancelled.push(request_id.clone());
                }
            }
            if cancelled.is_empty() {
                return Ok((records.clone(), cancelled));
            }
            Ok((next, cancelled))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn cancel_question(
        &self,
        request_id: &str,
        reason: &str,
    ) -> Result<(), InteractionHistoryError> {
        if reason.trim().is_empty() {
            return Err(InteractionHistoryError::Unavailable);
        }
        self.mutate_history(|records| {
            let mut next = records.clone();
            let Some(InteractionHistoryRecord::Question { state, .. }) = next.get_mut(request_id)
            else {
                return Err(InteractionHistoryError::NotPending);
            };
            if state != &QuestionHistoryState::Pending {
                return Err(InteractionHistoryError::AlreadySettled);
            }
            *state = QuestionHistoryState::Cancelled {
                reason: reason.to_owned().into(),
            };
            Ok((next, ()))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn decide(
        &self,
        request_id: &str,
        actor: &Identity,
        option_id: &str,
        acknowledge_persistent: bool,
    ) -> Result<OfferedOptionId, InteractionHistoryError> {
        if option_id.is_empty() {
            return Err(InteractionHistoryError::InvalidOptionId);
        }
        self.mutate_history(|records| {
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
            Ok((next, selected))
        })
        .await
    }

    pub(in crate::interaction_broker) async fn prune_expired(
        &self,
        now: DateTime<Utc>,
        batch_size: usize,
    ) -> Result<u64, InteractionHistoryError> {
        if !(1..=500).contains(&batch_size) {
            return Err(InteractionHistoryError::Unavailable);
        }
        let cutoff = now
            .checked_sub_signed(ChronoDuration::days(30))
            .ok_or(InteractionHistoryError::Unavailable)?;
        self.mutate_history(|data| {
            let mut expired = data
                .created_at
                .iter()
                .filter(|(_, created_at)| **created_at < cutoff)
                .map(|(request_id, created_at)| (created_at.to_owned(), request_id.clone()))
                .collect::<Vec<_>>();
            expired.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
            expired.truncate(batch_size);
            if expired.is_empty() {
                return Ok((data.clone(), 0));
            }
            let mut next = data.clone();
            for (_, request_id) in &expired {
                next.records.remove(request_id);
                next.created_at.remove(request_id);
            }
            let count =
                u64::try_from(expired.len()).map_err(|_| InteractionHistoryError::Unavailable)?;
            Ok((next, count))
        })
        .await
    }
}

#[cfg(test)]
#[path = "interaction_history_store_tests.rs"]
mod tests;
