//! Direct messages: store-first sends and replies, push inspection, inbox and history.
use super::{CollaborationRejection, CollaborationRejectionReason, PublishedRejection};
use crate::ServiceIdentity;
use crate::push_record_delivery::{PushDeliveryFailure, store_first_and_deliver};
use crate::push_record_resolver::{
    ActivityReadError, ReferenceError, caller_is_local, can_read, delivery_result,
    expand_activity_ranges, link_for, make_notice_list, resolve_reference,
};
use automation_storage::{
    AutomationStore, DirectMessageHistoryQuery, PushInboxQuery, StorageError,
};
use collaboration_protocol::{
    MachineId, MessageContent, MessageDelivery, PushHeaderFacts, PushId, PushKind,
    PushMessageSendResult, PushOrigin, PushRecord, PushRecordDraft, PushRecordHistoryParams,
    PushRecordListParams, PushRecordListResult, PushRecordShowParams, PushRecordShowResult,
    RouterLink, SessionDisplayNameLookup, SessionMessageReplyParams, SessionMessageReplyResult,
    SessionMessageSendParams, SessionRef, session_identity,
};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Direct-message operations over the push store and the subscription delivery owner.
pub struct MessageOperations<'service> {
    identity: &'service ServiceIdentity,
}

impl<'service> MessageOperations<'service> {
    pub(crate) fn new(identity: &'service ServiceIdentity) -> Self {
        Self { identity }
    }

    fn push_store(
        &self,
        stage: MessageFailureStage,
    ) -> Result<&'service Arc<Mutex<AutomationStore>>, MessageFailure> {
        self.identity.automation.as_ref().ok_or_else(|| {
            MessageFailure::new(
                MessageFailureKind::Unavailable,
                stage,
                "Push storage is unavailable",
            )
        })
    }

    /// Stores a direct message, then makes its one delivery attempt.
    pub async fn message_send(
        &self,
        request: SessionMessageSendParams,
    ) -> Result<PushMessageSendResult, MessageFailure> {
        let identity = self.identity;
        if request.target.endpoint.service_id != identity.service_id {
            return Err(MessageFailure::discovery(
                MessageFailureKind::WrongService,
                "Message target belongs to another Router",
            ));
        }
        let (origin, sender_display_name, text) = match request.message {
            MessageContent::Agent { sender, text } => {
                if sender.endpoint.service_id != identity.service_id {
                    return Err(MessageFailure::discovery(
                        MessageFailureKind::WrongService,
                        "Message sender belongs to another Router",
                    ));
                }
                (
                    PushOrigin::Session(sender.clone()),
                    identity
                        .display_names
                        .display_name_for(&sender)
                        .ok()
                        .flatten(),
                    text.as_str().to_owned(),
                )
            }
            MessageContent::HumanUser { text } => {
                (PushOrigin::OwnerUnverified, None, text.as_str().to_owned())
            }
            MessageContent::Router { .. } => {
                return Err(MessageFailure::discovery(
                    MessageFailureKind::InvalidField,
                    "Router-authored content is internal-only",
                ));
            }
        };
        let target = request.target.clone();
        let push_id = next_push_id()?;
        let draft = PushRecordDraft {
            push_id,
            kind: PushKind::DirectMessage,
            origin,
            origin_router_ref: None,
            target: target.clone(),
            reply_to_push_id: None,
            header_facts: PushHeaderFacts::DirectMessage {
                sender_display_name,
            },
            body: Some(text),
            activity: None,
            mode: Some(request.mode),
            guard: request.generation_guard.clone(),
            created_at: chrono::Utc::now(),
        };
        let target_identity = self.target_identity(&target);
        let record =
            store_first_and_deliver(draft, request.mode, request.generation_guard, identity)
                .await
                .map_err(|failure| self.delivery_failure(failure))?;
        delivery_result(&record, target_identity, identity).map_err(|_| {
            MessageFailure::new(
                MessageFailureKind::Unavailable,
                MessageFailureStage::Inspect,
                "Stored push could not be rendered",
            )
        })
    }

    /// Replies to one retained direct message, addressed by push id or Router link.
    pub async fn message_reply(
        &self,
        request: SessionMessageReplyParams,
    ) -> Result<SessionMessageReplyResult, MessageFailure> {
        let identity = self.identity;
        if request.caller.endpoint.service_id != identity.service_id {
            return Err(MessageFailure::discovery(
                MessageFailureKind::WrongService,
                "Reply caller belongs to another Router",
            ));
        }
        let push_id = self.resolve(&request.reference, MessageFailureStage::Discovery)?;
        let store = self.push_store(MessageFailureStage::Discovery)?;
        let source = match store.lock().await.get_push_record(&push_id).await {
            Ok(Some(record)) => record,
            Ok(None) | Err(StorageError::PushNotFound) => {
                return Err(MessageFailure::not_found(MessageFailureStage::Discovery));
            }
            Err(_) => {
                return Err(MessageFailure::discovery(
                    MessageFailureKind::Unavailable,
                    "Push storage could not be read",
                ));
            }
        };
        if !can_read(&source, &request.caller) {
            return Err(MessageFailure::discovery(
                MessageFailureKind::NotPermitted,
                "Not permitted to reply to this push",
            ));
        }
        self.reply_to(request, source).await
    }

    async fn reply_to(
        &self,
        request: SessionMessageReplyParams,
        source: PushRecord,
    ) -> Result<SessionMessageReplyResult, MessageFailure> {
        let identity = self.identity;
        if source.kind != PushKind::DirectMessage {
            return Err(MessageFailure::discovery(
                MessageFailureKind::NotDirectMessage,
                "This push is not a direct message; use the command for its push kind",
            ));
        }
        let PushOrigin::Session(origin) = source.origin.clone() else {
            return Err(MessageFailure::discovery(
                MessageFailureKind::OwnerReplyUnsupported,
                "This message has no reply session; use message send --to <SessionRef>",
            ));
        };
        if source.target != request.caller {
            return Err(MessageFailure::discovery(
                MessageFailureKind::NotPermitted,
                "Only the target session can reply to this direct message",
            ));
        }
        let reply_text = request.text.as_str().to_owned();
        let reply_push_id = next_push_id()?;
        let draft = PushRecordDraft {
            push_id: reply_push_id,
            kind: PushKind::DirectMessage,
            origin: PushOrigin::Session(request.caller.clone()),
            origin_router_ref: None,
            target: origin.clone(),
            reply_to_push_id: Some(source.push_id.clone()),
            header_facts: PushHeaderFacts::DirectMessage {
                sender_display_name: identity
                    .display_names
                    .display_name_for(&request.caller)
                    .ok()
                    .flatten(),
            },
            body: Some(reply_text),
            activity: None,
            mode: Some(MessageDelivery::Auto),
            guard: None,
            created_at: chrono::Utc::now(),
        };
        let target_identity = self.target_identity(&origin);
        let record = store_first_and_deliver(draft, MessageDelivery::Auto, None, identity)
            .await
            .map_err(|failure| self.delivery_failure(failure))?;
        let Some(receipt) = record.last_outcome.clone() else {
            return Err(MessageFailure::discovery(
                MessageFailureKind::OutcomeUnknown,
                "Reply was stored but its delivery result is unavailable",
            ));
        };
        Ok(SessionMessageReplyResult {
            target: origin,
            target_identity,
            push_id: record.push_id.clone(),
            link: link_for(&record, identity),
            delivery_state: record.delivery_state,
            receipt,
        })
    }

    /// Shows one push the caller may read; showing a direct message to its target marks it read.
    pub async fn push_show(
        &self,
        request: PushRecordShowParams,
    ) -> Result<PushRecordShowResult, MessageFailure> {
        let identity = self.identity;
        let stage = MessageFailureStage::Inspect;
        if !caller_is_local(&request.caller, identity) {
            return Err(MessageFailure::new(
                MessageFailureKind::WrongService,
                stage,
                "Caller belongs to another Router",
            ));
        }
        let push_id = self.resolve(&request.reference, stage)?;
        let store = self.push_store(stage)?;
        let mut record = match store.lock().await.get_push_record(&push_id).await {
            Ok(Some(record)) => record,
            Ok(None) | Err(StorageError::PushNotFound) => {
                return Err(MessageFailure::not_found(stage));
            }
            Err(_) => {
                return Err(MessageFailure::new(
                    MessageFailureKind::Unavailable,
                    stage,
                    "Push storage could not be read",
                ));
            }
        };
        let not_permitted = || {
            MessageFailure::new(
                MessageFailureKind::NotPermitted,
                stage,
                "Not permitted to read this push",
            )
        };
        if !can_read(&record, &request.caller) {
            return Err(not_permitted());
        }
        if record.kind == PushKind::DirectMessage && record.target == request.caller {
            record = match store
                .lock()
                .await
                .mark_push_read(&push_id, &request.caller, chrono::Utc::now())
                .await
            {
                Ok(record) => record,
                Err(StorageError::PushNotFound) => return Err(MessageFailure::not_found(stage)),
                Err(StorageError::PushNotPermitted) => return Err(not_permitted()),
                Err(_) => {
                    return Err(MessageFailure::new(
                        MessageFailureKind::Unavailable,
                        stage,
                        "Push read state could not be recorded",
                    ));
                }
            };
        }
        let activity_ranges = expand_activity_ranges(&record, identity)
            .await
            .map_err(|error| {
                let message = match error {
                    ActivityReadError::Unavailable => "Board range storage is unavailable",
                    ActivityReadError::Failed => "Board activity range could not be read",
                };
                MessageFailure::new(MessageFailureKind::Unavailable, stage, message)
            })?;
        Ok(PushRecordShowResult {
            link: link_for(&record, identity),
            record,
            activity_ranges,
        })
    }

    /// Lists the caller's received direct messages, newest first.
    pub async fn message_inbox(
        &self,
        request: PushRecordListParams,
    ) -> Result<PushRecordListResult, MessageFailure> {
        if !caller_is_local(&request.caller, self.identity) {
            return Err(MessageFailure::discovery(
                MessageFailureKind::WrongService,
                "Caller belongs to another Router",
            ));
        }
        let store = self.push_store(MessageFailureStage::Discovery)?;
        let records = store
            .lock()
            .await
            .list_direct_message_inbox(&PushInboxQuery {
                target: request.caller,
                limit: request.limit,
            })
            .await
            .map_err(|error| match error {
                StorageError::InvalidRecord => MessageFailure::discovery(
                    MessageFailureKind::InvalidField,
                    "Inbox limit must be between 1 and 100",
                ),
                _ => MessageFailure::discovery(
                    MessageFailureKind::Unavailable,
                    "Push inbox could not be read",
                ),
            })?;
        self.notice_list(records)
    }

    /// Lists the direct messages exchanged between two local sessions.
    pub async fn message_history(
        &self,
        request: PushRecordHistoryParams,
    ) -> Result<PushRecordListResult, MessageFailure> {
        if !caller_is_local(&request.caller, self.identity)
            || !caller_is_local(&request.with, self.identity)
        {
            return Err(MessageFailure::discovery(
                MessageFailureKind::WrongService,
                "Both sessions must belong to this Router",
            ));
        }
        let store = self.push_store(MessageFailureStage::Discovery)?;
        let records = store
            .lock()
            .await
            .list_direct_message_history(&DirectMessageHistoryQuery {
                caller: request.caller,
                with: request.with,
                limit: request.limit,
            })
            .await
            .map_err(|error| match error {
                StorageError::InvalidRecord => MessageFailure::discovery(
                    MessageFailureKind::InvalidField,
                    "History limit must be between 1 and 100",
                ),
                _ => MessageFailure::discovery(
                    MessageFailureKind::Unavailable,
                    "Push history could not be read",
                ),
            })?;
        self.notice_list(records)
    }

    fn notice_list(
        &self,
        records: Vec<PushRecord>,
    ) -> Result<PushRecordListResult, MessageFailure> {
        let records = make_notice_list(records, self.identity).map_err(|_| {
            MessageFailure::discovery(
                MessageFailureKind::Unavailable,
                "Push notice could not be rendered",
            )
        })?;
        Ok(PushRecordListResult { records })
    }

    fn resolve(
        &self,
        reference: &str,
        stage: MessageFailureStage,
    ) -> Result<PushId, MessageFailure> {
        resolve_reference(reference, self.identity).map_err(|error| match error {
            ReferenceError::Invalid => MessageFailure::new(
                MessageFailureKind::InvalidField,
                stage,
                "Invalid push id or Router link",
            ),
            ReferenceError::Foreign(machine_id) => MessageFailure::new(
                MessageFailureKind::ForeignMachine,
                stage,
                format!("lives on {machine_id}; cross-machine fetch not available yet"),
            ),
        })
    }

    fn target_identity(&self, target: &SessionRef) -> String {
        let display_name = self
            .identity
            .display_names
            .display_name_for(target)
            .ok()
            .flatten();
        session_identity(target, display_name.as_ref())
    }

    fn delivery_failure(&self, failure: PushDeliveryFailure) -> MessageFailure {
        match failure {
            PushDeliveryFailure::StoreUnavailable => MessageFailure::discovery(
                MessageFailureKind::Unavailable,
                "Push storage is unavailable",
            ),
            PushDeliveryFailure::DeliveryUnavailable => MessageFailure::discovery(
                MessageFailureKind::Unavailable,
                "Session delivery is unavailable",
            ),
            PushDeliveryFailure::InvalidRecord(error) => {
                MessageFailure::discovery(MessageFailureKind::InvalidField, error.to_string())
            }
            PushDeliveryFailure::StoreFailed => MessageFailure::new(
                MessageFailureKind::Unavailable,
                MessageFailureStage::Store,
                "Push record could not be stored",
            ),
            PushDeliveryFailure::DeliveryUnknown(push_id) => {
                let link = RouterLink::new(
                    MachineId::from(self.identity.machine_identity.service_id().clone()),
                    push_id,
                );
                MessageFailure::new(
                    MessageFailureKind::OutcomeUnknown,
                    MessageFailureStage::Inspect,
                    format!(
                        "Push was stored; delivery outcome is unknown. Inspect {link} before retrying."
                    ),
                )
            }
        }
    }
}

fn next_push_id() -> Result<PushId, MessageFailure> {
    PushId::try_from(uuid::Uuid::now_v7().to_string()).map_err(|_| {
        MessageFailure::discovery(
            MessageFailureKind::Unavailable,
            "A push id could not be generated",
        )
    })
}

/// Why a direct-message operation failed: `{kind, stage, message}` on the wire.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct MessageFailure {
    pub kind: MessageFailureKind,
    pub stage: MessageFailureStage,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageFailureKind {
    /// A request field is invalid.
    InvalidField,
    /// A named session belongs to another Router.
    WrongService,
    /// The push lives on another machine; cross-machine reads are not available yet.
    ForeignMachine,
    Unavailable,
    /// The push expired after 30 days, or never existed.
    NotFound,
    NotPermitted,
    NotDirectMessage,
    OwnerReplyUnsupported,
    /// The message was stored; whether it was delivered is unknown.
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageFailureStage {
    Discovery,
    Inspect,
    Store,
}

impl MessageFailure {
    fn new(
        kind: MessageFailureKind,
        stage: MessageFailureStage,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            stage,
            message: message.into(),
        }
    }

    fn discovery(kind: MessageFailureKind, message: impl Into<String>) -> Self {
        Self::new(kind, MessageFailureStage::Discovery, message)
    }

    fn not_found(stage: MessageFailureStage) -> Self {
        Self::new(
            MessageFailureKind::NotFound,
            stage,
            "not found (expired after 30 days, or never existed)",
        )
    }
}

impl CollaborationRejection for MessageFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.kind {
            MessageFailureKind::InvalidField => Some(CollaborationRejectionReason::InvalidShape),
            MessageFailureKind::WrongService
            | MessageFailureKind::ForeignMachine
            | MessageFailureKind::Unavailable
            | MessageFailureKind::NotFound
            | MessageFailureKind::NotPermitted
            | MessageFailureKind::NotDirectMessage
            | MessageFailureKind::OwnerReplyUnsupported
            | MessageFailureKind::OutcomeUnknown => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        let code = match self.kind {
            MessageFailureKind::InvalidField | MessageFailureKind::WrongService => {
                PublishedRejection::INVALID_PARAMS
            }
            MessageFailureKind::ForeignMachine
            | MessageFailureKind::Unavailable
            | MessageFailureKind::NotFound
            | MessageFailureKind::NotPermitted
            | MessageFailureKind::NotDirectMessage
            | MessageFailureKind::OwnerReplyUnsupported
            | MessageFailureKind::OutcomeUnknown => PublishedRejection::OPERATION_FAILED,
        };
        PublishedRejection::typed(code, self.message.clone(), self)
    }
}
