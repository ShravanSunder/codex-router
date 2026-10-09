//! Store-first direct-message delivery through Layer 0's PreparedPush path.
use crate::ServiceIdentity;
use automation_storage::StorageError;
use collaboration_protocol::{
    CodexGeneration, MessageDelivery, PushId, PushRecord, PushRecordDraft,
    PushRecordValidationError,
};

pub(crate) enum PushDeliveryFailure {
    StoreUnavailable,
    DeliveryUnavailable,
    InvalidRecord(PushRecordValidationError),
    StoreFailed,
    DeliveryUnknown(PushId),
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

#[cfg(test)]
mod tests {
    use crate::collaboration_application::CollaborationRejection;
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

        let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_ID)
            .expect("service identity")
            .with_automation_store(Arc::clone(&automation))
            .with_subscription_delivery_service(subscription_service, presence);
        let target = session(&identity.service_id);
        let sent = crate::CollaborationApplication::new(identity.clone())
            .messages()
            .message_send(SessionMessageSendParams {
                target: target.clone(),
                message: MessageContent::HumanUser {
                    text: "stored before delivery"
                        .to_owned()
                        .try_into()
                        .expect("message text"),
                },
                mode: MessageDelivery::Auto,
                generation_guard: None,
            })
            .await;
        let response = match sent {
            Ok(result) => json!({ "result": result }),
            Err(failure) => json!({ "error": failure.published_rejection() }),
        };

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
    }
}
