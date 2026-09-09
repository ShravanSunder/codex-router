//! Storage event formats project into the closed public event vocabulary, never an untyped dump.
use automation_storage::{AutomationStore, EventPosition, StorageError, StoredAutomationEvent};
use communication_protocol::{
    AutomationEvent, AutomationEventDetails, AutomationEventSubject, AutomationEventSubjectKind,
    UuidIdentity,
};
use serde_json::Value;

pub(crate) async fn project(
    store: &mut AutomationStore,
    event: StoredAutomationEvent,
    service: &UuidIdentity,
) -> Result<AutomationEvent, StorageError> {
    let subject_kind: AutomationEventSubjectKind =
        decode(Value::String(event.subject_kind.clone()))?;
    let details = match event.body.get("kind").and_then(Value::as_str) {
        Some("scheduleEdit") => AutomationEventDetails::ScheduleEdit {
            before: decode(required(&event.body, "before")?)?,
            after: decode(required(&event.body, "after")?)?,
        },
        Some("stateChange") => decode(event.body.clone())?,
        Some("deliveryAttempt") => {
            let id = event
                .subject_id
                .clone()
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?;
            let attempt = decode(required(&event.body, "attempt")?)?;
            AutomationEventDetails::DeliveryAttempt {
                attempt: Box::new(
                    crate::attempt_history_projection::delivery(store, &id, attempt).await?,
                ),
            }
        }
        Some("summaryAttempt") => {
            let id = event
                .subject_id
                .clone()
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?;
            let attempt = decode(required(&event.body, "attempt")?)?;
            AutomationEventDetails::SummaryAttempt {
                attempt: Box::new(crate::attempt_history_projection::summary(
                    &id, attempt, false,
                )?),
            }
        }
        Some("configurationChange") => {
            let configuration: communication_protocol::AutomationConfiguration =
                decode(required(&event.body, "configuration")?)?;
            AutomationEventDetails::ConfigurationChange {
                execution_timeout_seconds: configuration.execution_timeout_seconds,
                summary_timeout_seconds: configuration.summary_timeout_seconds,
            }
        }
        _ => match (subject_kind, event.event_kind.as_str()) {
            (AutomationEventSubjectKind::Instruction, "created" | "updated") => {
                let instruction: agent_automation::InstructionDocument =
                    decode(event.body.clone())?;
                if instruction.instruction_id.as_str() != event.subject_id {
                    return Err(StorageError::InvalidRecord);
                }
                AutomationEventDetails::StateChange {
                    before: None,
                    after: instruction.revision_id.as_str().into(),
                }
            }
            (AutomationEventSubjectKind::Schedule, "created") => {
                AutomationEventDetails::ScheduleEdit {
                    before: None,
                    after: decode(required(&event.body, "definition")?)?,
                }
            }
            (AutomationEventSubjectKind::Wake, kind) => {
                let state = match kind {
                    "paused" => "paused",
                    "cancelled" => "cancelled",
                    "expired" => "expired",
                    "finished" | "finishedWithoutFiring" => "finished",
                    "fired" | "coalesced" | "resumed" => "active",
                    _ => return Err(StorageError::InvalidRecord),
                };
                AutomationEventDetails::StateChange {
                    before: None,
                    after: state.into(),
                }
            }
            _ => return Err(StorageError::InvalidRecord),
        },
    };
    let description = match subject_kind {
        AutomationEventSubjectKind::Instruction => {
            "Instruction revision recorded; revision/list retains its text."
        }
        AutomationEventSubjectKind::Schedule => {
            "Schedule configuration recorded; admitted Run inputs remain unchanged."
        }
        AutomationEventSubjectKind::Wake if event.event_kind == "fired" => {
            "Reminder became eligible; firing does not imply native acceptance."
        }
        AutomationEventSubjectKind::Wake => {
            "Reminder lifecycle changed; accepted native input is not recalled."
        }
        AutomationEventSubjectKind::Run => {
            "Workflow evidence recorded; native completion is not proof of objective success."
        }
        AutomationEventSubjectKind::Delivery => {
            "Delivery attempt evidence recorded; uncertain input is not automatically resent."
        }
        AutomationEventSubjectKind::Configuration => {
            "Automation defaults changed; active attempts retain their captured budgets."
        }
    };
    Ok(AutomationEvent {
        event_id: event.event_id,
        cursor: crate::automation_event_cursor::encode(
            service,
            &EventPosition {
                sequence: event.sequence,
                observed_at_ms: event.recorded_at_ms,
            },
        )
        .map_err(|_| StorageError::InvalidRecord)?,
        recorded_at: crate::wakeup_projection::timestamp(event.recorded_at_ms)
            .map_err(|_| StorageError::InvalidRecord)?,
        subject: AutomationEventSubject {
            kind: subject_kind,
            id: event.subject_id,
        },
        change: event.event_kind,
        description: description.into(),
        details,
    })
}
fn decode<TResult: serde::de::DeserializeOwned>(value: Value) -> Result<TResult, StorageError> {
    serde_json::from_value(value).map_err(|_| StorageError::InvalidRecord)
}
fn required(value: &Value, field: &str) -> Result<Value, StorageError> {
    value.get(field).cloned().ok_or(StorageError::InvalidRecord)
}
