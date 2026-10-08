//! Wake lifecycle and delivery inspection Control dispatch over the typed wake operations.
use crate::collaboration_application::WakeOperations;
use crate::wakeup_dispatch::{WakeRequest, failure, failure_context, wake_response};
use collaboration_protocol::{LocalMutationState, WakeFailureReason, WakeMutationRequest};
use serde_json::Value;
pub(crate) async fn dispatch(request: WakeRequest<'_>) -> Value {
    let context = failure_context(&request.params);
    let Ok(params) = serde_json::from_value::<WakeMutationRequest>(request.params) else {
        return failure(
            request.id,
            context,
            WakeFailureReason::InvalidField {
                field: "request".into(),
                constraint: "Provide exact UUIDv7 operationId and wakeupId only.".into(),
            },
            LocalMutationState::None,
        );
    };
    let wakes = WakeOperations::new(request.service_id, request.store);
    let result = match request.method {
        "wake/pause" => wakes.wake_pause(params).await,
        "wake/resume" => wakes.wake_resume(params).await,
        "wake/cancel" => wakes.wake_cancel(params).await,
        _ => {
            return failure(
                request.id,
                context,
                WakeFailureReason::InvalidRecord,
                LocalMutationState::None,
            );
        }
    };
    wake_response(request.id, result)
}
pub(crate) async fn inspect_delivery(request: WakeRequest<'_>) -> Value {
    let context = failure_context(&request.params);
    let Ok(params) =
        serde_json::from_value::<collaboration_protocol::DeliveryShowRequest>(request.params)
    else {
        return failure(
                    request.id,
                    context,
                    WakeFailureReason::InvalidField {
                        field: "deliveryId".into(),
                        constraint: "deliveryId must be a canonical lowercase RFC UUIDv7, for example 019f0000-0000-7000-8000-000000000001.".into(),
                    },
                    LocalMutationState::None,
                );
    };
    let result = WakeOperations::new(request.service_id, request.store)
        .delivery_show(params)
        .await;
    wake_response(request.id, result)
}

#[cfg(test)]
mod tests {
    use super::{WakeRequest, inspect_delivery};
    use automation_storage::AutomationStore;
    use collaboration_protocol::UuidIdentity;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn missing_delivery_show_names_the_id_and_push_show_command() {
        let directory = tempfile::tempdir().expect("isolated automation store");
        let store = Arc::new(Mutex::new(
            AutomationStore::open(&directory.path().join("automation.sqlite"))
                .await
                .expect("automation store"),
        ));
        let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())
            .expect("service id");
        let delivery_id = agent_automation::DeliveryId::generate();

        let response = inspect_delivery(WakeRequest {
            id: json!(1),
            method: "delivery/show",
            params: json!({"deliveryId": delivery_id.as_str()}),
            service_id: &service_id,
            store: Some(&store),
        })
        .await;

        let message = response
            .pointer("/error/data/message")
            .and_then(Value::as_str)
            .expect("delivery not-found message");
        assert_eq!(
            message,
            format!(
                "Delivery {} was not found; for a push record, run agent-collaboration show <link>.",
                delivery_id.as_str()
            )
        );
    }

    #[tokio::test]
    async fn malformed_delivery_show_names_delivery_id_and_uuidv7_constraint() {
        let directory = tempfile::tempdir().expect("isolated automation store");
        let store = Arc::new(Mutex::new(
            AutomationStore::open(&directory.path().join("automation.sqlite"))
                .await
                .expect("automation store"),
        ));
        let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())
            .expect("service id");

        let response = inspect_delivery(WakeRequest {
            id: json!(1),
            method: "delivery/show",
            params: json!({"deliveryId":"not-a-uuid"}),
            service_id: &service_id,
            store: Some(&store),
        })
        .await;

        let constraint = "deliveryId must be a canonical lowercase RFC UUIDv7, for example 019f0000-0000-7000-8000-000000000001.";
        assert_eq!(response.pointer("/error/code"), Some(&json!(-32050)));
        assert_eq!(
            response.pointer("/error/data/kind"),
            Some(&json!("invalidField"))
        );
        assert_eq!(
            response.pointer("/error/data/field"),
            Some(&json!("deliveryId"))
        );
        assert_eq!(
            response.pointer("/error/data/constraint"),
            Some(&json!(constraint))
        );
        assert_eq!(
            response.pointer("/error/data/message"),
            Some(&json!(constraint))
        );
    }
}
