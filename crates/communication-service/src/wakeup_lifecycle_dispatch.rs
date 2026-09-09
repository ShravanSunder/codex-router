//! Lifecycle mutations retain atomic delivery snapshots and never recall accepted native input.
use crate::wakeup_dispatch::{FailureContext, WakeRequest, failure};
use automation_storage::{StorageError, WakeAction, WakeMutation};
use communication_protocol::{
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
    let params =
        match serde_json::from_value::<communication_protocol::DeliveryShowRequest>(request.params)
        {
            Ok(params) => params,
            Err(_) => {
                return failure(
                    request.id,
                    context,
                    WakeFailureReason::InvalidField {
                        field: "deliveryId".into(),
                        constraint: "Provide exact UUIDv7 deliveryId only.".into(),
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
        Err(_) => failure(
            request.id,
            context,
            WakeFailureReason::ResourceNotFound,
            LocalMutationState::None,
        ),
    }
}

pub(crate) fn project_mutation(
    result: automation_storage::WakeMutationResult<SavedMessage>,
    service_id: &communication_protocol::UuidIdentity,
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
