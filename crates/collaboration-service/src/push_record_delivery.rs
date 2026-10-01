//! Store-first direct-message delivery through Layer 0's PreparedPush path.
use crate::{
    DeliveryPrecondition, LoadPolicy, ServiceIdentity, push_record_resolver::line_for,
    session_delivery_contract::UnstoredAttemptEvidenceSink,
};
use automation_storage::StorageError;
use collaboration_protocol::{
    AttemptId, CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, DeliveryReceipt,
    MessageContent, MessageDelivery, MessageText, PushId, PushIdError, PushKind, PushOrigin,
    PushRecord, PushRecordDraft, PushRecordValidationError, SessionDisplayNameLookup,
    SessionMessageReplyParams, SessionMessageReplyResult, SessionMessageSendParams, SessionRef,
    session_identity,
};
use serde_json::{Value, json};

pub(crate) enum PushDeliveryFailure {
    StoreUnavailable,
    DeliveryUnavailable,
    InvalidRecord(PushRecordValidationError),
    StoreFailed,
    LineInvalid,
}

pub(crate) async fn dispatch_message(
    id: Value,
    params: SessionMessageSendParams,
    identity: &ServiceIdentity,
) -> Value {
    if params.target.endpoint.service_id != identity.service_id {
        return crate::push_record_resolver::failure(
            id,
            -32602,
            "wrongService",
            "discovery",
            "Message target belongs to another Router",
        );
    }
    let (origin, sender_display_name, text) = match params.message {
        MessageContent::Agent { sender, text } => {
            if sender.endpoint.service_id != identity.service_id {
                return crate::push_record_resolver::failure(
                    id,
                    -32602,
                    "wrongService",
                    "discovery",
                    "Message sender belongs to another Router",
                );
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
            return crate::push_record_resolver::failure(
                id,
                -32602,
                "invalidField",
                "discovery",
                "Router-authored content is internal-only",
            );
        }
    };
    let target = params.target.clone();
    let push_id = match next_push_id() {
        Ok(push_id) => push_id,
        Err(_) => {
            return crate::push_record_resolver::failure(
                id,
                -32050,
                "unavailable",
                "discovery",
                "A push id could not be generated",
            );
        }
    };
    let draft = PushRecordDraft {
        push_id,
        kind: PushKind::DirectMessage,
        origin,
        origin_router_ref: None,
        target: target.clone(),
        reply_to_push_id: None,
        header_facts: collaboration_protocol::PushHeaderFacts::DirectMessage {
            sender_display_name,
        },
        body: Some(text),
        activity: None,
        created_at: chrono::Utc::now(),
    };
    let target_identity = target_identity(&target, identity);
    match store_first_and_deliver(draft, params.mode, params.generation_guard, identity).await {
        Ok(record) => {
            match crate::push_record_resolver::delivery_result(&record, target_identity, identity) {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(_) => crate::push_record_resolver::failure(
                    id,
                    -32050,
                    "unavailable",
                    "inspect",
                    "Stored push could not be rendered",
                ),
            }
        }
        Err(error) => delivery_failure(id, error),
    }
}

pub(crate) async fn dispatch_reply(
    id: Value,
    params: SessionMessageReplyParams,
    source_record: PushRecord,
    identity: &ServiceIdentity,
) -> Value {
    if source_record.kind != PushKind::DirectMessage {
        return crate::push_record_resolver::failure(
            id,
            -32050,
            "notDirectMessage",
            "discovery",
            "This push is not a direct message; use the command for its push kind",
        );
    }
    let PushOrigin::Session(origin) = source_record.origin.clone() else {
        return crate::push_record_resolver::failure(
            id,
            -32050,
            "ownerReplyUnsupported",
            "discovery",
            "This message has no reply session; use message send --to <SessionRef>",
        );
    };
    if source_record.target != params.caller {
        return crate::push_record_resolver::failure(
            id,
            -32050,
            "notPermitted",
            "discovery",
            "Only the target session can reply to this direct message",
        );
    }
    let reply_text = params.text.as_str().to_owned();
    let reply_push_id = match next_push_id() {
        Ok(push_id) => push_id,
        Err(_) => {
            return crate::push_record_resolver::failure(
                id,
                -32050,
                "unavailable",
                "discovery",
                "A push id could not be generated",
            );
        }
    };
    let draft = PushRecordDraft {
        push_id: reply_push_id.clone(),
        kind: PushKind::DirectMessage,
        origin: PushOrigin::Session(params.caller.clone()),
        origin_router_ref: None,
        target: origin.clone(),
        reply_to_push_id: Some(source_record.push_id.clone()),
        header_facts: collaboration_protocol::PushHeaderFacts::DirectMessage {
            sender_display_name: identity
                .display_names
                .display_name_for(&params.caller)
                .ok()
                .flatten(),
        },
        body: Some(reply_text),
        activity: None,
        created_at: chrono::Utc::now(),
    };
    let target_identity = target_identity(&origin, identity);
    match store_first_and_deliver(draft, MessageDelivery::Auto, None, identity).await {
        Ok(record) => {
            let Some(receipt) = record.last_outcome.clone() else {
                return crate::push_record_resolver::failure(
                    id,
                    -32050,
                    "outcomeUnknown",
                    "discovery",
                    "Reply was stored but its delivery result is unavailable",
                );
            };
            let result = SessionMessageReplyResult {
                target: origin,
                target_identity,
                push_id: record.push_id.clone(),
                link: crate::push_record_resolver::link_for(&record, identity),
                receipt,
            };
            json!({"jsonrpc":"2.0","id":id,"result":result})
        }
        Err(error) => delivery_failure(id, error),
    }
}

pub(crate) async fn store_first_and_deliver(
    draft: PushRecordDraft,
    mode: MessageDelivery,
    generation_guard: Option<CodexGeneration>,
    identity: &ServiceIdentity,
) -> Result<PushRecord, PushDeliveryFailure> {
    let Some(store) = identity.automation.as_ref() else {
        return Err(PushDeliveryFailure::StoreUnavailable);
    };
    let Some(delivery) = identity.session_delivery.as_ref() else {
        return Err(PushDeliveryFailure::DeliveryUnavailable);
    };
    let target = draft.target.clone();
    let push_id = draft.push_id.clone();
    let pending_draft = draft
        .clone()
        .into_pending()
        .map_err(PushDeliveryFailure::InvalidRecord)?;
    let line = line_for(&pending_draft, identity).map_err(|_| PushDeliveryFailure::LineInvalid)?;
    store
        .lock()
        .await
        .insert_push_record(draft)
        .await
        .map_err(|error| match error {
            StorageError::InvalidRecord => {
                PushDeliveryFailure::InvalidRecord(PushRecordValidationError::DirectMessageTooLarge)
            }
            _ => PushDeliveryFailure::StoreFailed,
        })?;
    store
        .lock()
        .await
        .mark_push_attempted(&push_id)
        .await
        .map_err(|_| PushDeliveryFailure::StoreFailed)?;
    let line: MessageText = line
        .try_into()
        .map_err(|_| PushDeliveryFailure::LineInvalid)?;
    let prepared = crate::layer_zero::PreparedPush {
        push_id: push_id.clone(),
        line,
        load_policy: LoadPolicy::MayLoad,
    };
    let correlation = DeliveryCorrelationId::try_from(push_id.as_str().to_owned())
        .map_err(|_| PushDeliveryFailure::LineInvalid)?;
    let request = crate::layer_zero::DeliveryRequest {
        payload: prepared,
        target,
        mode,
        precondition: generation_guard.map_or(DeliveryPrecondition::Unpinned, |expected| {
            DeliveryPrecondition::EndpointGeneration { expected }
        }),
        correlation,
        attempt: AttemptId::generate(),
    };
    let outcome = match delivery
        .deliver_prepared(request, &UnstoredAttemptEvidenceSink)
        .await
    {
        Ok(receipt) => receipt,
        Err(_) => DeliveryReceipt {
            outcome: DeliveryOutcome::Unknown,
            reachability: None,
            client: None,
        },
    };
    let settled_at = chrono::Utc::now();
    store
        .lock()
        .await
        .settle_push_record(&push_id, outcome, settled_at)
        .await
        .map_err(|_| PushDeliveryFailure::StoreFailed)
}

fn next_push_id() -> Result<PushId, PushIdError> {
    PushId::try_from(uuid::Uuid::now_v7().to_string())
}

fn target_identity(target: &SessionRef, identity: &ServiceIdentity) -> String {
    let display_name = identity
        .display_names
        .display_name_for(target)
        .ok()
        .flatten();
    session_identity(target, display_name.as_ref())
}

fn delivery_failure(id: Value, failure: PushDeliveryFailure) -> Value {
    match failure {
        PushDeliveryFailure::StoreUnavailable => crate::push_record_resolver::failure(
            id,
            -32050,
            "unavailable",
            "discovery",
            "Push storage is unavailable",
        ),
        PushDeliveryFailure::DeliveryUnavailable => crate::push_record_resolver::failure(
            id,
            -32050,
            "unavailable",
            "discovery",
            "Session delivery is unavailable",
        ),
        PushDeliveryFailure::InvalidRecord(error) => crate::push_record_resolver::failure(
            id,
            -32602,
            "invalidField",
            "discovery",
            &error.to_string(),
        ),
        PushDeliveryFailure::StoreFailed => crate::push_record_resolver::failure(
            id,
            -32050,
            "unavailable",
            "discovery",
            "Push record could not be stored or settled",
        ),
        PushDeliveryFailure::LineInvalid => crate::push_record_resolver::failure(
            id,
            -32050,
            "unavailable",
            "discovery",
            "Push line could not be prepared",
        ),
    }
}
