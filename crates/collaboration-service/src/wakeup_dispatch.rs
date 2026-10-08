//! Wake-family Control dispatch: decodes each request and calls the typed wake operations.
use crate::collaboration_application::{WakeFailureContext, WakeOperations, wake_failure};
use automation_storage::AutomationStore;
use collaboration_protocol::{
    LocalMutationState, UuidIdentity, WakeFailure, WakeFailureReason, WakeSendRequest,
    WakeShowRequest,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) struct WakeRequest<'a> {
    pub id: Value,
    pub method: &'a str,
    pub params: Value,
    pub service_id: &'a UuidIdentity,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}

/// The wake and operation identities a request names, read before it is decoded.
pub(crate) fn failure_context(params: &Value) -> WakeFailureContext {
    WakeFailureContext {
        operation_id: params
            .get("operationId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        wakeup_id: params
            .get("wakeupId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    }
}

pub(crate) async fn dispatch(request: WakeRequest<'_>) -> Value {
    if request.method == "wake/list" {
        return crate::wakeup_list_dispatch::dispatch(request).await;
    }
    if matches!(request.method, "wake/pause" | "wake/resume" | "wake/cancel") {
        return crate::wakeup_lifecycle_dispatch::dispatch(request).await;
    }
    if request.method == "delivery/show" {
        return crate::wakeup_lifecycle_dispatch::inspect_delivery(request).await;
    }
    let context = failure_context(&request.params);
    let wakes = WakeOperations::new(request.service_id, request.store);
    let result = match request.method {
        "wake/send" => {
            let Ok(params) = serde_json::from_value::<WakeSendRequest>(request.params) else {
                return failure(request.id,context,WakeFailureReason::InvalidField { field:"request".into(), constraint:"Provide closed wake fields, a UUIDv7 operation identity and valid bounded timing.".into() },LocalMutationState::None);
            };
            wakes.wake_send(params).await
        }
        "wake/show" => {
            let Ok(params) = serde_json::from_value::<WakeShowRequest>(request.params) else {
                return failure(
                    request.id,
                    context,
                    WakeFailureReason::InvalidField {
                        field: "wakeupId".into(),
                        constraint: "Provide an exact UUIDv7 wake identity; no additional fields."
                            .into(),
                    },
                    LocalMutationState::None,
                );
            };
            wakes.wake_show(params).await
        }
        _ => {
            return json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32601,"message":"Unknown wake operation"}});
        }
    };
    wake_response(request.id, result)
}

pub(crate) fn wake_response(
    id: Value,
    result: Result<impl serde::Serialize, WakeFailure>,
) -> Value {
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(data) => failure_response(id, data),
    }
}

pub(crate) fn failure_response(id: Value, data: WakeFailure) -> Value {
    crate::control_connection::rejection_response(id, &data)
}

pub(crate) fn failure(
    id: Value,
    context: WakeFailureContext,
    reason: WakeFailureReason,
    mutation: LocalMutationState,
) -> Value {
    failure_response(id, wake_failure(context, reason, mutation))
}

pub(crate) fn overloaded(id: Value) -> Value {
    failure(
        id,
        WakeFailureContext::default(),
        WakeFailureReason::Overloaded,
        LocalMutationState::None,
    )
}
