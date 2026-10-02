//! Lifecycle mutations retain atomic delivery snapshots and never recall accepted native input.
use crate::wakeup_dispatch::{FailureContext, WakeRequest, failure};
use automation_storage::{StorageError, WakeAction, WakeMutation};
use collaboration_protocol::{
    LocalMutationState, SavedMessage, WakeFailureReason, WakeMutationRequest, WakeMutationResult,
};
use serde_json::{Value, json};
pub(crate) async fn dispatch(request: WakeRequest<'_>) -> Value {
    let context = FailureContext::from_params(&request.params);
    let Some(store) = request.store else {
        return failure(
            request.id,
            context,
            WakeFailureReason::AutomationUnavailable,
            LocalMutationState::None,
        );
    };
    let params = match serde_json::from_value::<WakeMutationRequest>(request.params) {
        Ok(params) => params,
        Err(_) => {
            return failure(
                request.id,
                context,
                WakeFailureReason::InvalidField {
                    field: "request".into(),
                    constraint: "Provide exact UUIDv7 operationId and wakeupId only.".into(),
                },
                LocalMutationState::None,
            );
        }
    };
    let action = match request.method {
        "wake/pause" => WakeAction::Pause,
        "wake/resume" => WakeAction::Resume,
        "wake/cancel" => WakeAction::Cancel,
        _ => {
            return failure(
                request.id,
                context,
                WakeFailureReason::InvalidRecord,
                LocalMutationState::None,
            );
        }
    };
    let now = chrono::Utc::now().timestamp_millis();
    let result = store
        .lock()
        .await
        .mutate_wakeup::<SavedMessage>(&WakeMutation {
            operation_id: params.operation_id,
            wakeup_id: params.wakeup_id,
            action,
            now_ms: now,
        })
        .await;
    match result {
        Ok(result) => {
            let projected = project_mutation(result, request.service_id);
            match projected {
                Ok(result) => json!({"jsonrpc":"2.0","id":request.id,"result":result}),
                Err(()) => failure(
                    request.id,
                    context,
                    WakeFailureReason::InvalidRecord,
                    LocalMutationState::Committed,
                ),
            }
        }
        Err(error) => {
            let effect = if matches!(error, StorageError::Database(_)) {
                LocalMutationState::Unknown
            } else {
                LocalMutationState::None
            };
            let reason = match error {
                StorageError::WakeNotFound => WakeFailureReason::ResourceNotFound,
                StorageError::OperationConflict => WakeFailureReason::OperationConflict,
                StorageError::WakeLifecycleConflict { .. } => WakeFailureReason::LifecycleConflict,
                StorageError::Database(_) => WakeFailureReason::AutomationUnavailable,
                _ => WakeFailureReason::InvalidRecord,
            };
            failure(request.id, context, reason, effect)
        }
    }
}
pub(crate) async fn inspect_delivery(request: WakeRequest<'_>) -> Value {
    let context = FailureContext::from_params(&request.params);
    let Some(store) = request.store else {
        return failure(
            request.id,
            context,
            WakeFailureReason::AutomationUnavailable,
            LocalMutationState::None,
        );
    };
    let params = match serde_json::from_value::<collaboration_protocol::DeliveryShowRequest>(
        request.params,
    ) {
        Ok(params) => params,
        Err(_) => {
            return failure(
                    request.id,
                    context,
                    WakeFailureReason::InvalidField {
                        field: "deliveryId".into(),
                        constraint: "deliveryId must be a canonical lowercase RFC UUIDv7, for example 019f0000-0000-7000-8000-000000000001.".into(),
                    },
                    LocalMutationState::None,
                );
        }
    };
    let result = store.lock().await.read_delivery(&params.delivery_id).await;
    match result {
        Ok(record) => match crate::delivery_projection::snapshot(record) {
            Ok(result) => json!({"jsonrpc":"2.0","id":request.id,"result":result}),
            Err(()) => failure(
                request.id,
                context,
                WakeFailureReason::InvalidRecord,
                LocalMutationState::None,
            ),
        },
        Err(_) => {
            let mut response = failure(
                request.id,
                context,
                WakeFailureReason::ResourceNotFound,
                LocalMutationState::None,
            );
            if let Some(data) = response
                .pointer_mut("/error/data")
                .and_then(Value::as_object_mut)
            {
                data.insert(
                    "message".to_owned(),
                    json!(format!(
                    "Delivery {} was not found; for a push record, run agent-collaboration show <link>.",
                    params.delivery_id.as_str()
                    )),
                );
            }
            response
        }
    }
}

pub(crate) fn project_mutation(
    result: automation_storage::WakeMutationResult<SavedMessage>,
    service_id: &collaboration_protocol::UuidIdentity,
) -> Result<WakeMutationResult, ()> {
    let wakeup =
        crate::wakeup_projection::snapshot(result.wake, service_id, result.observed_at_ms)?;
    let dispatched_deliveries = result
        .retained
        .into_iter()
        .map(|record| {
            let typed = serde_json::from_value(serde_json::to_value(record).map_err(|_| ())?)
                .map_err(|_| ())?;
            crate::delivery_projection::snapshot(typed)
        })
        .collect::<Result<Vec<_>, ()>>()?;
    Ok::<_, ()>(WakeMutationResult {
        wakeup,
        discarded_delivery_ids: result.discarded,
        dispatched_deliveries,
    })
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
