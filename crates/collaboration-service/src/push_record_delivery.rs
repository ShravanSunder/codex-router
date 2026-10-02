//! Store-first direct-message delivery through Layer 0's PreparedPush path.
use crate::ServiceIdentity;
use automation_storage::StorageError;
use collaboration_protocol::{
    CodexGeneration, MessageContent, MessageDelivery, PushId, PushIdError, PushKind, PushOrigin,
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
    DeliveryUnknown(PushId),
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
        mode: Some(params.mode),
        guard: params.generation_guard.clone(),
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
        Err(error) => delivery_failure(id, error, identity),
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
        mode: Some(MessageDelivery::Auto),
        guard: None,
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
                delivery_state: record.delivery_state,
                receipt,
            };
            json!({"jsonrpc":"2.0","id":id,"result":result})
        }
        Err(error) => delivery_failure(id, error, identity),
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
    let Some(service) = identity.subscription_delivery.as_ref() else {
        return Err(PushDeliveryFailure::DeliveryUnavailable);
    };
    let mut draft = draft;
    draft.mode = Some(mode);
    draft.guard = generation_guard;
    let target = draft.target.clone();
    let push_id = draft.push_id.clone();
    draft
        .clone()
        .into_pending()
        .map_err(PushDeliveryFailure::InvalidRecord)?;
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
    service
        .deliver_direct_message(target, push_id.clone())
        .await
        .map_err(|_| PushDeliveryFailure::DeliveryUnknown(push_id))
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

fn delivery_failure(id: Value, failure: PushDeliveryFailure, identity: &ServiceIdentity) -> Value {
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
            "store",
            "Push record could not be stored",
        ),
        PushDeliveryFailure::DeliveryUnknown(push_id) => {
            let link = collaboration_protocol::RouterLink::new(
                collaboration_protocol::MachineId::from(
                    identity.machine_identity.service_id().clone(),
                ),
                push_id,
            );
            crate::push_record_resolver::failure(
                id,
                -32050,
                "outcomeUnknown",
                "inspect",
                &format!(
                    "Push was stored; delivery outcome is unknown. Inspect {link} before retrying."
                ),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::dispatch_message;
    use crate::{
        BoardAvailability, MachineIdentity, ServiceIdentity, SessionDeliveryRouter,
        SessionMessageDelivery, SubscriptionDeliveryService, SubscriptionDeliveryServiceProps,
        SystemSubscriptionClock, TargetPresenceProbe,
    };
    use automation_storage::AutomationStore;
    use collaboration_protocol::{
        EndpointId, EndpointRef, MessageContent, MessageDelivery, PushDeliveryState, RouterLink,
        SessionId, SessionMessageSendParams, SessionRef, UuidIdentity,
    };
    use serde_json::{Value, json};
    use std::{path::Path, sync::Arc};
    use tokio::sync::Mutex;

    const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";

    fn session(service_id: &UuidIdentity) -> SessionRef {
        SessionRef {
            endpoint: EndpointRef {
                service_id: service_id.clone(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("endpoint id"),
            },
            session_id: SessionId::try_from("target-session".to_owned()).expect("session id"),
        }
    }

    async fn automation_store(directory: &Path) -> Arc<Mutex<AutomationStore>> {
        Arc::new(Mutex::new(
            AutomationStore::open(&directory.join("automation.sqlite"))
                .await
                .expect("automation store"),
        ))
    }

    #[tokio::test]
    async fn committed_push_delivery_error_returns_outcome_unknown_with_its_link() {
        let directory = tempfile::tempdir().expect("isolated service directory");
        let automation = automation_store(directory.path()).await;
        let router = Arc::new(SessionDeliveryRouter::new(Vec::new()));
        let delivery: Arc<dyn SessionMessageDelivery> = router.clone();
        let presence: Arc<dyn TargetPresenceProbe> = router;
        let machine_identity = MachineIdentity::new(
            UuidIdentity::try_from(SERVICE_ID.to_owned()).expect("service id"),
            Some("delivery-error-test"),
        )
        .expect("machine identity");
        let subscription_service =
            SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
                board_availability: BoardAvailability::Unavailable,
                push_store: Arc::clone(&automation),
                delivery,
                presence: Arc::clone(&presence),
                machine_identity,
                clock: Arc::new(SystemSubscriptionClock),
            });
        subscription_service
            .start()
            .await
            .expect("delivery owner starts");
        subscription_service.shutdown().await;

        let identity = ServiceIdentity::new(
            SERVICE_ID,
            SERVICE_ID,
            &format!("sha256:{}", "a".repeat(64)),
        )
        .expect("service identity")
        .with_automation_store(Arc::clone(&automation))
        .with_subscription_delivery_service(subscription_service, presence);
        let target = session(&identity.service_id);
        let response = dispatch_message(
            json!("client-1"),
            SessionMessageSendParams {
                target: target.clone(),
                message: MessageContent::HumanUser {
                    text: "stored before delivery"
                        .to_owned()
                        .try_into()
                        .expect("message text"),
                },
                mode: MessageDelivery::Auto,
                generation_guard: None,
            },
            &identity,
        )
        .await;

        let response_message = response
            .pointer("/error/data/message")
            .and_then(Value::as_str)
            .expect("unknown outcome message");
        let link_start = response_message
            .find(&format!("router://{SERVICE_ID}/push/"))
            .expect("unknown outcome message includes a local push link");
        let link_suffix = response_message
            .get(link_start..)
            .expect("push link begins at a UTF-8 boundary");
        let link_end = link_suffix
            .find(" before retrying")
            .expect("push link has an actionable suffix");
        let link_text = link_suffix
            .get(..link_end)
            .expect("push link ends at a UTF-8 boundary");
        let link = RouterLink::parse(link_text).expect("stored push link parses");
        let record = automation
            .lock()
            .await
            .get_push_record(link.push_id())
            .await
            .expect("read committed direct message")
            .expect("push link names the committed record");
        assert_eq!(record.delivery_state, PushDeliveryState::Pending);
        assert!(record.last_outcome.is_none());
        let expected_link = crate::push_record_resolver::link_for(&record, &identity);
        assert_eq!(
            response.pointer("/error/data/kind"),
            Some(&json!("outcomeUnknown"))
        );
        assert_eq!(
            response.pointer("/error/data/stage"),
            Some(&json!("inspect"))
        );
        assert_eq!(
            response.pointer("/error/data/message"),
            Some(&Value::String(format!(
                "Push was stored; delivery outcome is unknown. Inspect {expected_link} before retrying."
            )))
        );
        assert!(collaboration_protocol::control_error_is_valid(
            "message/send",
            &response
        ));
    }
}
