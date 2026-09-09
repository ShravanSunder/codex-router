//! Admission errors retain the method's published feedback shape before any operation is dispatched.
use communication_protocol::*;
use serde_json::{Value, json};

pub(crate) fn response(id: Value, method: &str, params: &Value) -> Value {
    const MESSAGE: &str = "Request capacity exceeded; this request was not dispatched. Reconnect and retry the same operation identity when capacity is available.";
    if matches!(method, "control/initialize") {
        return standard(id, MESSAGE);
    }
    if method == "wake/subscribe" {
        return params.get("wakeupId").cloned().and_then(|value| serde_json::from_value(value).ok())
            .map_or_else(|| standard(id.clone(), "Wake wait capacity exceeded; reconnect the wait without recreating or cancelling the reminder."), |wakeup_id| crate::wakeup_subscription::unavailable(id.clone(), wakeup_id));
    }
    if matches!(
        method,
        "instruction/list"
            | "schedule/list"
            | "run/list"
            | "revision/list"
            | "delivery/list"
            | "automation/events"
            | "delivery/attempts"
            | "run/summaries"
            | "operation/show"
            | "operation/reconcile"
            | "delivery/reconcile"
            | "run/reconcile"
    ) {
        let mut failure = crate::automation_inspection_failure::unavailable();
        failure.kind = AutomationInspectionFailureKind::Overloaded;
        failure.message = MESSAGE.into();
        return crate::automation_inspection_failure::response(id, failure);
    }
    if matches!(method, "automation/configure" | "automation/status") {
        return typed(
            id,
            ConfigurationFailure {
                kind: ConfigurationFailureKind::AutomationUnavailable,
                message: MESSAGE.into(),
                operation_id: identity(params, "operationId"),
                file_state: ConfigurationFileState::NotReplaced,
                next_action: ConfigurationNextAction::RetryLater,
            },
        );
    }
    if method.starts_with("schedule/") {
        return typed(
            id,
            ScheduleFailure {
                kind: ScheduleFailureKind::Overloaded,
                stage: ScheduleFailureStage::Admission,
                message: MESSAGE.into(),
                operation_id: identity(params, "operationId"),
                schedule_id: identity(params, "scheduleId"),
                current_change_id: None,
                field: None,
                constraint: None,
                details: ScheduleFailureDetails::None,
                effects: ScheduleEffects::Local {
                    mutation: LocalMutationState::None,
                },
                next_action: ScheduleNextAction::RetryLater,
            },
        );
    }
    if method.starts_with("run/") {
        return typed(
            id,
            RunFailure {
                kind: RunFailureKind::Overloaded,
                stage: RunFailureStage::Admission,
                message: MESSAGE.into(),
                operation_id: identity(params, "operationId"),
                run_id: identity(params, "runId"),
                effects: LocalMutationEvidence::Local {
                    mutation: LocalMutationState::None,
                },
                next_action: RunNextAction::RetryLater,
            },
        );
    }
    if method.starts_with("wake/") || method.starts_with("delivery/") {
        return crate::wakeup_dispatch::overloaded(id);
    }
    if method.starts_with("instruction/") {
        return crate::instruction_dispatch::overloaded(id);
    }
    let data = if method == "codex/messageSend" {
        json!({"kind":"overloaded","stage":"discovery","message":MESSAGE,"effects":{"resume":"notRequested","submission":"notDispatched"}})
    } else {
        json!({"kind":"overloaded","stage":"discovery","message":MESSAGE})
    };
    typed(id, data)
}
fn identity<TIdentity: serde::de::DeserializeOwned>(
    params: &Value,
    field: &str,
) -> Option<TIdentity> {
    params
        .get(field)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}
fn typed(id: Value, data: impl serde::Serialize) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Request capacity exceeded","data":data}})
}
fn standard(id: Value, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":message}})
}
