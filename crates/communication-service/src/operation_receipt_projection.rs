//! Stored command results use their original snapshots, not a fresh read of the resource.
use automation_storage::{StorageError, StoredOperationRecord, StoredOperationState};
use communication_protocol::{
    AutomationOperationMethod as Method, EndpointRef, OperationEffects, OperationFailure,
    OperationSnapshot, OperationState, OperationSuccess, SavedMessage, SessionRef, UuidIdentity,
};
use serde_json::Value;

pub(crate) fn snapshot(
    record: StoredOperationRecord,
    service: &UuidIdentity,
) -> Result<OperationSnapshot, StorageError> {
    let method: Method = decode(Value::String(record.method))?;
    let state = match record.state {
        StoredOperationState::Admitted { effects } => OperationState::Admitted { effects: project_effects(method, effects)? },
        StoredOperationState::InProgress { effects } => OperationState::InProgress { effects: project_effects(method, effects)? },
        StoredOperationState::Uncertain { effects, failure } => OperationState::Uncertain {
            effects: project_effects(method, effects)?,
            explanation: failure.as_ref().and_then(|failure|failure.get("message")).and_then(Value::as_str).unwrap_or("The original operation has unresolved effects; reconciliation never blindly repeats a native mutation.").into(),
        },
        StoredOperationState::Succeeded { result } => OperationState::Succeeded { outcome: success(method, result, service)? },
        StoredOperationState::Failed { effects, failure } => OperationState::Failed { error: Box::new(project_failure(method, effects, failure)?) },
    };
    let result = OperationSnapshot {
        operation_id: record.operation_id,
        method,
        resource_id: record.resource_id,
        admitted_at: crate::wakeup_projection::timestamp(record.admitted_at_ms)
            .map_err(|_| StorageError::InvalidRecord)?,
        state,
    };
    if !result.has_consistent_outcome() {
        return Err(StorageError::InvalidRecord);
    }
    Ok(result)
}
fn project_effects(method: Method, effects: Value) -> Result<OperationEffects, StorageError> {
    if method == Method::SchedulePrepare {
        Ok(OperationEffects::Native {
            evidence: Box::new(decode(effects)?),
        })
    } else {
        decode(effects)
    }
}
fn project_failure(
    method: Method,
    effects: Value,
    failure: Value,
) -> Result<OperationFailure, StorageError> {
    let effects = project_effects(method, effects)?;
    match method {
        Method::SchedulePrepare => {
            let error: communication_protocol::ScheduleFailure = decode(failure)?;
            Ok(OperationFailure {
                kind: token(error.kind)?,
                stage: token(error.stage)?,
                message: error.message,
                field: error.field,
                constraint: error.constraint,
                next_action: token(error.next_action)?,
                effects,
            })
        }
        Method::Configure => {
            let error: communication_protocol::ConfigurationFailure = decode(failure)?;
            Ok(OperationFailure {
                kind: token(error.kind)?,
                stage: "configuration".into(),
                message: error.message,
                field: None,
                constraint: None,
                next_action: token(error.next_action)?,
                effects,
            })
        }
        _ => Err(StorageError::InvalidRecord),
    }
}
fn success(
    method: Method,
    result: Value,
    service: &UuidIdentity,
) -> Result<OperationSuccess, StorageError> {
    let method_value = Value::String(method.as_str().into());
    match method {
        Method::InstructionCreate | Method::InstructionUpdate => {
            Ok(OperationSuccess::Instruction {
                method: decode(method_value)?,
                result: Box::new(
                    crate::instruction_dispatch::snapshot(decode(result)?)
                        .map_err(|_| StorageError::InvalidRecord)?,
                ),
            })
        }
        Method::ScheduleCreate
        | Method::ScheduleUpdate
        | Method::ScheduleEnable
        | Method::ScheduleDisable
        | Method::SchedulePrepare
        | Method::ScheduleImport => {
            let inspection: automation_storage::ScheduleInspection<SessionRef, EndpointRef> =
                if method == Method::ScheduleCreate {
                    automation_storage::ScheduleInspection {
                        record: decode(result)?,
                        active_run_id: None,
                        waiting_run_id: None,
                    }
                } else {
                    decode(result)?
                };
            Ok(OperationSuccess::Schedule {
                method: decode(method_value)?,
                result: Box::new(
                    crate::schedule_projection::snapshot(inspection)
                        .map_err(|_| StorageError::InvalidRecord)?,
                ),
            })
        }
        Method::WakeSend => {
            let record: agent_automation::WakeRecord<SavedMessage> = decode(result)?;
            let observed = record.definition.created_at_ms;
            Ok(OperationSuccess::WakeCreated {
                method: decode(method_value)?,
                result: Box::new(
                    crate::wakeup_projection::snapshot(record, service, observed)
                        .map_err(|_| StorageError::InvalidRecord)?,
                ),
            })
        }
        Method::WakePause | Method::WakeResume | Method::WakeCancel => {
            Ok(OperationSuccess::WakeChanged {
                method: decode(method_value)?,
                result: Box::new(
                    crate::wakeup_lifecycle_dispatch::project_mutation(decode(result)?, service)
                        .map_err(|_| StorageError::InvalidRecord)?,
                ),
            })
        }
        Method::SummaryRetry | Method::SummarySkip => Ok(OperationSuccess::RunRecovery {
            method: decode(method_value)?,
            result: Box::new(
                crate::run_projection::snapshot(decode(result)?)
                    .map_err(|_| StorageError::InvalidRecord)?,
            ),
        }),
        Method::Configure => Ok(OperationSuccess::Configuration {
            method: decode(method_value)?,
            result: decode(result)?,
        }),
    }
}
fn decode<TResult: serde::de::DeserializeOwned>(value: Value) -> Result<TResult, StorageError> {
    serde_json::from_value(value).map_err(|_| StorageError::InvalidRecord)
}
fn token(value: impl serde::Serialize) -> Result<String, StorageError> {
    match serde_json::to_value(value).map_err(|_| StorageError::InvalidRecord)? {
        Value::String(value) => Ok(value),
        _ => Err(StorageError::InvalidRecord),
    }
}
