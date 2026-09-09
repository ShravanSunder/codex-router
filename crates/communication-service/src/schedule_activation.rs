//! Enable admission requires a prepared owned address or an available fresh-thread endpoint.
use crate::NativeControlBackend;
use agent_automation::{ExecutionDestination, OperationId, ScheduleDefinition, ScheduleId};
use automation_storage::{AutomationStore, BindingAddress, StorageError};
use communication_protocol::{EndpointRef, SessionRef, UuidIdentity};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct ActivationRequest<'a> {
    pub definition: &'a ScheduleDefinition<SessionRef, EndpointRef>,
    pub schedule_id: Option<&'a ScheduleId>,
    pub operation_id: &'a OperationId,
    pub service_id: &'a UuidIdentity,
    pub backend: Option<&'a NativeControlBackend>,
}
pub(crate) async fn validate(
    store: &Arc<Mutex<AutomationStore>>,
    request: ActivationRequest<'_>,
) -> Result<(), StorageError> {
    if store
        .lock()
        .await
        .has_operation_receipt(request.operation_id)
        .await?
        || !request.definition.enabled
    {
        return Ok(());
    }
    let endpoint = match &request.definition.destination {
        ExecutionDestination::Unprepared => {
            return Err(StorageError::InvalidSchedule {
                field: "enabled",
                reason: "prepare the destination before enabling future triggers",
            });
        }
        ExecutionDestination::FreshEachRun { endpoint, .. } => endpoint,
        ExecutionDestination::OwnedThread { target, .. } => {
            let Some(schedule_id) = request.schedule_id else {
                return Err(StorageError::InvalidSchedule {
                    field: "destination",
                    reason: "create disabled, then explicitly prepare/adopt the native thread",
                });
            };
            let owned = store
                .lock()
                .await
                .owns_thread_address(BindingAddress {
                    schedule_id,
                    service_id: &String::from(target.endpoint.service_id.clone()),
                    endpoint_id: &String::from(target.endpoint.endpoint_id.clone()),
                    thread_id: &String::from(target.session_id.clone()),
                })
                .await?;
            if !owned {
                return Err(StorageError::InvalidSchedule {
                    field: "destination",
                    reason: "thread address is not owned by this schedule; use explicit destination preparation",
                });
            }
            &target.endpoint
        }
    };
    if &endpoint.service_id != request.service_id {
        return Err(StorageError::InvalidSchedule {
            field: "destination",
            reason: "endpoint belongs to another service",
        });
    }
    let backend = request
        .backend
        .filter(|backend| backend.endpoint == *endpoint)
        .ok_or(StorageError::ActivationUnavailable)?;
    let admission = backend
        .gate
        .acquire()
        .map_err(|_| StorageError::ActivationUnavailable)?;
    let schemas = admission
        .schemas()
        .ok_or(StorageError::ActivationUnavailable)?;
    use codex_native_integration::NativeOperation;
    let required = [
        NativeOperation::ReadThread,
        NativeOperation::ListTurns,
        NativeOperation::StartTurn,
        NativeOperation::InterruptTurn,
        if matches!(
            request.definition.destination,
            ExecutionDestination::FreshEachRun { .. }
        ) {
            NativeOperation::StartThread
        } else {
            NativeOperation::ResumeThread
        },
    ];
    if required
        .iter()
        .any(|operation| !schemas.supports_operation(*operation))
    {
        return Err(StorageError::ActivationUnavailable);
    }
    Ok(())
}
