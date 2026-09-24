//! Enable admission requires a prepared owned address or an available fresh-thread endpoint.
use crate::{ScheduleDestination, ScheduleSupport, ScheduledRunExecution};
use agent_automation::{ExecutionDestination, OperationId, ScheduleDefinition, ScheduleId};
use automation_storage::{AutomationStore, BindingAddress, StorageError};
use collaboration_protocol::{EndpointRef, SessionRef, UuidIdentity};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct ActivationRequest<'a> {
    pub definition: &'a ScheduleDefinition<SessionRef, EndpointRef>,
    pub schedule_id: Option<&'a ScheduleId>,
    pub operation_id: &'a OperationId,
    pub service_id: &'a UuidIdentity,
    pub execution: Option<&'a Arc<dyn ScheduledRunExecution>>,
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
    let destination = match &request.definition.destination {
        ExecutionDestination::FreshEachRunUnprepared => {
            return Err(StorageError::InvalidSchedule {
                field: "destination",
                reason: "use schedule update to set a freshEachRun endpoint and absolute workspace before enabling; execution mode remains fresh-per-run",
            });
        }
        ExecutionDestination::Unprepared => {
            return Err(StorageError::InvalidSchedule {
                field: "enabled",
                reason: "prepare the destination before enabling future triggers",
            });
        }
        ExecutionDestination::FreshEachRun { endpoint, .. } => ScheduleDestination::Fresh {
            endpoint: endpoint.clone(),
        },
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
            ScheduleDestination::Existing {
                target: target.clone(),
            }
        }
    };
    let endpoint = match &destination {
        ScheduleDestination::Fresh { endpoint } => endpoint,
        ScheduleDestination::Existing { target } => &target.endpoint,
        ScheduleDestination::Fork { .. } => return Err(StorageError::ActivationUnavailable),
    };
    if &endpoint.service_id != request.service_id {
        return Err(StorageError::InvalidSchedule {
            field: "destination",
            reason: "endpoint belongs to another service",
        });
    }
    match request
        .execution
        .ok_or(StorageError::ActivationUnavailable)?
        .support(&destination)
        .await
        .map_err(|_| StorageError::ActivationUnavailable)?
    {
        ScheduleSupport::Supported { .. } => Ok(()),
        ScheduleSupport::Unsupported { .. } => Err(StorageError::ActivationUnavailable),
    }
}
