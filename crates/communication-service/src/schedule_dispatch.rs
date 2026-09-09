//! Local schedule administration; native allocation remains an explicit preparation operation.
use automation_storage::{
    AutomationStore, ScheduleCreate, ScheduleEdit, ScheduleInspection, ScheduleMutation,
    StorageError,
};
use communication_protocol::{
    EndpointRef, LocalMutationEvidence, LocalMutationState, ScheduleCreateRequest,
    ScheduleEnableRequest, ScheduleFailure, ScheduleFailureKind, ScheduleFailureStage,
    ScheduleNextAction, ScheduleShowRequest, ScheduleUpdateRequest, SessionRef,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct ScheduleRequest<'a> {
    pub id: Value,
    pub method: &'a str,
    pub params: Value,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}
pub(crate) async fn dispatch(request: ScheduleRequest<'_>) -> Value {
    let context = FailureContext {
        operation_id: request
            .params
            .get("operationId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        schedule_id: request
            .params
            .get("scheduleId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    };
    let Some(store) = request.store else {
        return failure(
            request.id,
            context,
            StorageError::InvalidRecord,
            LocalMutationState::None,
            None,
        );
    };
    let now = chrono::Utc::now().timestamp_millis();
    let result: Result<ScheduleInspection<SessionRef, EndpointRef>, StorageError> = match request
        .method
    {
        "schedule/create" => {
            let params = match serde_json::from_value::<ScheduleCreateRequest>(request.params) {
                Ok(params) => params,
                Err(_) => return invalid(request.id, context),
            };
            let definition =
                match serde_json::to_value(params.definition).and_then(serde_json::from_value) {
                    Ok(definition) => definition,
                    Err(_) => return invalid(request.id, context),
                };
            store
                .lock()
                .await
                .create_schedule(&ScheduleCreate {
                    operation_id: params.operation_id,
                    definition,
                    imported_continuity: agent_automation::ContinuityInput::None,
                    now_ms: now,
                })
                .await
                .map(|record| ScheduleInspection {
                    record,
                    active_run_id: None,
                    waiting_run_id: None,
                })
        }
        "schedule/show" => {
            let params = match serde_json::from_value::<ScheduleShowRequest>(request.params) {
                Ok(params) => params,
                Err(_) => return invalid(request.id, context),
            };
            store
                .lock()
                .await
                .inspect_schedule(&params.schedule_id)
                .await
        }
        "schedule/update" => {
            let params = match serde_json::from_value::<ScheduleUpdateRequest>(request.params) {
                Ok(params) => params,
                Err(_) => return invalid(request.id, context),
            };
            let definition =
                match serde_json::to_value(params.definition).and_then(serde_json::from_value) {
                    Ok(definition) => definition,
                    Err(_) => return invalid(request.id, context),
                };
            store
                .lock()
                .await
                .mutate_schedule(&ScheduleMutation {
                    operation_id: params.operation_id,
                    schedule_id: params.schedule_id,
                    edit: ScheduleEdit::Replace {
                        expected_change_id: params.expected_change_id,
                        definition,
                    },
                    now_ms: now,
                })
                .await
        }
        "schedule/enable" | "schedule/disable" => {
            let params = match serde_json::from_value::<ScheduleEnableRequest>(request.params) {
                Ok(params) => params,
                Err(_) => return invalid(request.id, context),
            };
            store
                .lock()
                .await
                .mutate_schedule(&ScheduleMutation {
                    operation_id: params.operation_id,
                    schedule_id: params.schedule_id,
                    edit: ScheduleEdit::SetEnabled {
                        enabled: request.method == "schedule/enable",
                    },
                    now_ms: now,
                })
                .await
        }
        _ => return invalid(request.id, context),
    };
    match result {
        Ok(result) => match crate::schedule_projection::snapshot(result) {
            Ok(result) => json!({"jsonrpc":"2.0","id":request.id,"result":result}),
            Err(()) => failure(
                request.id,
                context,
                StorageError::InvalidRecord,
                if request.method == "schedule/show" {
                    LocalMutationState::None
                } else {
                    LocalMutationState::Committed
                },
                None,
            ),
        },
        Err(error) => {
            let mutation = if request.method != "schedule/show"
                && matches!(error, StorageError::Database(_))
            {
                LocalMutationState::Unknown
            } else {
                LocalMutationState::None
            };
            let current = if matches!(error, StorageError::ScheduleChangeConflict) {
                if let Some(id) = &context.schedule_id {
                    store
                        .lock()
                        .await
                        .inspect_schedule::<SessionRef, EndpointRef>(id)
                        .await
                        .ok()
                        .map(|record| record.record.change_id)
                } else {
                    None
                }
            } else {
                None
            };
            failure(request.id, context, error, mutation, current)
        }
    }
}
struct FailureContext {
    operation_id: Option<communication_protocol::OperationId>,
    schedule_id: Option<communication_protocol::ScheduleId>,
}
fn invalid(id: Value, context: FailureContext) -> Value {
    failure(
        id,
        context,
        StorageError::InvalidSchedule {
            field: "request",
            reason: "Use the published closed schedule request shape, UUIDv7 identities and explicit nullable timeout.",
        },
        LocalMutationState::None,
        None,
    )
}
fn failure(
    id: Value,
    context: FailureContext,
    error: StorageError,
    mutation: LocalMutationState,
    current_change_id: Option<communication_protocol::ChangeId>,
) -> Value {
    let (kind, stage, next_action, field, constraint) = match &error {
        StorageError::InvalidSchedule { field, reason } => (
            ScheduleFailureKind::InvalidField,
            ScheduleFailureStage::Validation,
            ScheduleNextAction::CorrectRequest,
            Some((*field).to_owned()),
            Some((*reason).to_owned()),
        ),
        StorageError::InvalidTiming(error) => (
            ScheduleFailureKind::InvalidField,
            ScheduleFailureStage::Validation,
            ScheduleNextAction::CorrectRequest,
            Some("timing".into()),
            Some(error.to_string()),
        ),
        StorageError::ScheduleChangeConflict => (
            ScheduleFailureKind::ChangeConflict,
            ScheduleFailureStage::Admission,
            ScheduleNextAction::InspectSchedule,
            None,
            None,
        ),
        StorageError::OperationConflict => (
            ScheduleFailureKind::OperationConflict,
            ScheduleFailureStage::Admission,
            ScheduleNextAction::InspectOperation,
            None,
            None,
        ),
        StorageError::ScheduleNotFound | StorageError::InstructionNotFound => (
            ScheduleFailureKind::ResourceNotFound,
            ScheduleFailureStage::Inspection,
            ScheduleNextAction::CorrectRequest,
            None,
            None,
        ),
        StorageError::Database(_) => (
            ScheduleFailureKind::AutomationUnavailable,
            ScheduleFailureStage::Storage,
            ScheduleNextAction::InspectOperation,
            None,
            None,
        ),
        _ => (
            ScheduleFailureKind::InvalidRecord,
            ScheduleFailureStage::Storage,
            ScheduleNextAction::InspectSchedule,
            None,
            None,
        ),
    };
    let message = if matches!(
        mutation,
        LocalMutationState::Unknown | LocalMutationState::Committed
    ) {
        "Schedule mutation may exist; inspect or replay the same operation ID before any new request.".into()
    } else {
        error.to_string()
    };
    let data = ScheduleFailure {
        kind,
        stage,
        message,
        operation_id: context.operation_id,
        schedule_id: context.schedule_id,
        current_change_id,
        field,
        constraint,
        effects: LocalMutationEvidence::Local { mutation },
        next_action,
    };
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Schedule operation failed","data":data}})
}
