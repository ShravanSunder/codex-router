//! Inspection failures describe recovery without implying any new native submission.
use communication_protocol::{
    AutomationInspectionFailure, AutomationInspectionFailureKind, AutomationInspectionNextAction,
    AutomationInspectionStage, LocalMutationEvidence, LocalMutationState,
};
use serde_json::{Value, json};
pub(crate) fn invalid(field: &str, constraint: &str) -> AutomationInspectionFailure {
    AutomationInspectionFailure {
        kind: AutomationInspectionFailureKind::InvalidField,
        stage: AutomationInspectionStage::Validation,
        message: format!("Invalid {field}: {constraint}"),
        resource_id: None,
        field: Some(field.into()),
        constraint: Some(constraint.into()),
        earliest_retained_cursor: None,
        effects: LocalMutationEvidence::Local {
            mutation: LocalMutationState::None,
        },
        next_action: AutomationInspectionNextAction::CorrectRequest,
    }
}
pub(crate) fn storage(error: automation_storage::StorageError) -> AutomationInspectionFailure {
    let (kind, next_action) = match &error {
        automation_storage::StorageError::InstructionNotFound
        | automation_storage::StorageError::ScheduleNotFound
        | automation_storage::StorageError::WakeNotFound
        | automation_storage::StorageError::RunNotFound
        | automation_storage::StorageError::DeliveryNotFound
        | automation_storage::StorageError::OperationNotFound => (
            AutomationInspectionFailureKind::ResourceNotFound,
            AutomationInspectionNextAction::VerifyResourceAddress,
        ),
        automation_storage::StorageError::Database(_) => (
            AutomationInspectionFailureKind::AutomationUnavailable,
            AutomationInspectionNextAction::RetryLater,
        ),
        _ => (
            AutomationInspectionFailureKind::InvalidRecord,
            AutomationInspectionNextAction::RefreshCurrentState,
        ),
    };
    AutomationInspectionFailure {
        kind,
        stage: AutomationInspectionStage::Inspection,
        message: error.to_string(),
        resource_id: None,
        field: None,
        constraint: None,
        earliest_retained_cursor: None,
        effects: LocalMutationEvidence::Local {
            mutation: LocalMutationState::None,
        },
        next_action,
    }
}
pub(crate) fn unavailable() -> AutomationInspectionFailure {
    let mut failure = invalid(
        "storage",
        "Automation storage is unavailable; retry inspection later.",
    );
    failure.kind = AutomationInspectionFailureKind::AutomationUnavailable;
    failure.stage = AutomationInspectionStage::Inspection;
    failure.next_action = AutomationInspectionNextAction::RetryLater;
    failure
}
pub(crate) fn response(id: Value, failure: AutomationInspectionFailure) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Automation inspection failed","data":failure}})
}
