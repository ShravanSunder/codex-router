//! Durable wake requests touch automation storage; timing workers own later native submission.
use agent_automation::WakeupId;
use automation_storage::{AutomationStore, StorageError, WakeCreate};
use communication_protocol::{
    LocalMutationEvidence, LocalMutationState, OperationId, SavedMessage, UuidIdentity,
    WakeFailure, WakeFailureReason, WakeFailureStage, WakeNextAction, WakeSendRequest,
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
pub(crate) struct FailureContext {
    operation_id: Option<OperationId>,
    wakeup_id: Option<WakeupId>,
}
impl FailureContext {
    pub(crate) fn from_params(params: &Value) -> Self {
        Self {
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

    let context = FailureContext {
        operation_id: request
            .params
            .get("operationId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        wakeup_id: request
            .params
            .get("wakeupId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    };
    let Some(store) = request.store else {
        return failure(
            request.id,
            context,
            WakeFailureReason::AutomationUnavailable,
            LocalMutationState::None,
        );
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mutation = request.method == "wake/send";
    let result = match request.method {
        "wake/send" => {
            let params = match serde_json::from_value::<WakeSendRequest>(request.params) {
                Ok(params) => params,
                Err(_) => return failure(request.id,context,WakeFailureReason::InvalidField { field:"request".into(), constraint:"Provide closed wake fields, UUIDv7 operation identity, an explicit nullable generationGuard and valid bounded timing.".into() },LocalMutationState::None),
            };
            let timing = serde_json::to_value(params.timing).and_then(serde_json::from_value);
            let expiry = serde_json::to_value(params.expiry).and_then(serde_json::from_value);
            let (Ok(timing), Ok(expiry)) = (timing, expiry) else {
                return failure(
                    request.id,
                    context,
                    WakeFailureReason::InvalidField {
                        field: "timing".into(),
                        constraint: "Use a valid UTC instant or bounded duration.".into(),
                    },
                    LocalMutationState::None,
                );
            };
            store
                .lock()
                .await
                .create_wakeup(&WakeCreate {
                    operation_id: params.operation_id,
                    message: params.message,
                    timing,
                    expiry,
                    now_ms,
                })
                .await
        }
        "wake/show" => {
            let params = match serde_json::from_value::<WakeShowRequest>(request.params) {
                Ok(params) => params,
                Err(_) => {
                    return failure(
                        request.id,
                        context,
                        WakeFailureReason::InvalidField {
                            field: "wakeupId".into(),
                            constraint:
                                "Provide an exact UUIDv7 wake identity; no additional fields."
                                    .into(),
                        },
                        LocalMutationState::None,
                    );
                }
            };
            store
                .lock()
                .await
                .read_wakeup::<SavedMessage>(&params.wakeup_id)
                .await
        }
        _ => {
            return json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32601,"message":"Unknown wake operation"}});
        }
    };
    match result {
        Ok(record) => {
            // Creation/replay returns the same cursor timestamp as its durable creation receipt.
            let observed = if mutation {
                record.definition.created_at_ms
            } else {
                now_ms
            };
            match crate::wakeup_projection::snapshot(record, request.service_id, observed) {
                Ok(snapshot) => json!({"jsonrpc":"2.0","id":request.id,"result":snapshot}),
                Err(()) => failure(
                    request.id,
                    context,
                    WakeFailureReason::InvalidRecord,
                    if mutation {
                        LocalMutationState::Committed
                    } else {
                        LocalMutationState::None
                    },
                ),
            }
        }
        Err(error) => {
            let effect = if mutation && matches!(error, StorageError::Database(_)) {
                LocalMutationState::Unknown
            } else {
                LocalMutationState::None
            };
            let reason = match error {
                StorageError::WakeNotFound => WakeFailureReason::ResourceNotFound,
                StorageError::OperationConflict => WakeFailureReason::OperationConflict,
                StorageError::Database(_) => WakeFailureReason::AutomationUnavailable,
                StorageError::InvalidTiming(error) => WakeFailureReason::InvalidField {
                    field: "timing".into(),
                    constraint: error.to_string(),
                },
                StorageError::InvalidWake { field, reason } => WakeFailureReason::InvalidField {
                    field: field.into(),
                    constraint: reason.into(),
                },
                _ => WakeFailureReason::InvalidRecord,
            };
            failure(request.id, context, reason, effect)
        }
    }
}
pub(crate) fn failure(
    id: Value,
    context: FailureContext,
    reason: WakeFailureReason,
    mutation: LocalMutationState,
) -> Value {
    let (stage,next_action,message)=match &reason {
        WakeFailureReason::InvalidField {constraint,..}=>(WakeFailureStage::Validation,WakeNextAction::CorrectRequest,constraint.clone()),
        WakeFailureReason::ResourceNotFound=>(WakeFailureStage::Inspection,WakeNextAction::VerifyResourceAddress,"Wake-up was not found in this service; verify the service and wakeupId.".into()),
        WakeFailureReason::OperationConflict=>(WakeFailureStage::Admission,WakeNextAction::InspectOperation,"Operation identity belongs to a different request; inspect it before submitting new work.".into()),
        WakeFailureReason::LifecycleConflict=>(WakeFailureStage::Validation,WakeNextAction::InspectWakeup,"This wake cannot perform that lifecycle transition. Inspect its state; cancelled or expired reminders are not implicitly recreated.".into()),
        WakeFailureReason::Overloaded=>(WakeFailureStage::Admission,WakeNextAction::RetryLater,"Request capacity exceeded; no wake mutation was dispatched.".into()),
        _ if matches!(mutation,LocalMutationState::Unknown|LocalMutationState::Committed)=>(WakeFailureStage::Storage,WakeNextAction::InspectOperation,"Local mutation may exist; inspect or replay the same operation identity. Do not create a new identity blindly.".into()),
        _=>(WakeFailureStage::Storage,WakeNextAction::RetryLater,"Automation storage is unavailable or inconsistent; no new native submission was dispatched.".into()),
    };
    let data = WakeFailure {
        reason,
        stage,
        message,
        operation_id: context.operation_id,
        wakeup_id: context.wakeup_id,
        effects: LocalMutationEvidence::Local { mutation },
        next_action,
    };
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Wake operation failed","data":data}})
}
pub(crate) fn overloaded(id: Value) -> Value {
    failure(
        id,
        FailureContext {
            operation_id: None,
            wakeup_id: None,
        },
        WakeFailureReason::Overloaded,
        LocalMutationState::None,
    )
}
