//! Shared reporting for durable conversation operation commands.
use collaboration_client::ClientError;
use collaboration_client::protocol::{
    ConversationOperationFailure, ConversationOperationFailureKind, OperationId,
};
use std::io::Write;

pub(crate) fn report_success(value: serde_json::Value, json_output: bool) -> i32 {
    let value = crate::endpoint_commands::result_envelope(value);
    let rendered = if json_output {
        value.to_string()
    } else {
        serde_json::to_string_pretty(&value).unwrap_or_default()
    };
    if writeln!(std::io::stdout(), "{rendered}").is_ok() {
        0
    } else {
        3
    }
}

pub(crate) fn report_client_failure(
    error: ClientError,
    operation_id: &OperationId,
    may_have_dispatched: bool,
    json_output: bool,
) -> i32 {
    if let ClientError::Rejected {
        data: Some(data), ..
    } = &error
        && let Ok(failure) = serde_json::from_value::<ConversationOperationFailure>(data.clone())
    {
        return report_typed_failure(&failure, json_output);
    }
    let (kind, stage, effect, exit) = if may_have_dispatched {
        ("outcomeUnknown", "settlement", "unknown", 5)
    } else {
        ("unavailable", "binding", "none", 3)
    };
    let failure = serde_json::json!({
        "kind":kind,"stage":stage,"effect":effect,"message":error.to_string(),
        "operationId":operation_id
    });
    let rendered = if json_output {
        serde_json::json!({"kind":"error","error":failure}).to_string()
    } else {
        format!("Error: {kind}\nOperation ID: {operation_id:?}\n{error}")
    };
    let _written = if json_output {
        writeln!(std::io::stdout(), "{rendered}")
    } else {
        writeln!(std::io::stderr(), "{rendered}")
    };
    exit
}

fn report_typed_failure(failure: &ConversationOperationFailure, json_output: bool) -> i32 {
    let exit = match failure.kind {
        ConversationOperationFailureKind::InvalidRequest
        | ConversationOperationFailureKind::UnsupportedCapability
        | ConversationOperationFailureKind::ProtocolViolation => 2,
        ConversationOperationFailureKind::AuthenticationRequired
        | ConversationOperationFailureKind::Unavailable => 3,
        ConversationOperationFailureKind::PermissionRejected
        | ConversationOperationFailureKind::Busy
        | ConversationOperationFailureKind::NotFound
        | ConversationOperationFailureKind::StaleGeneration
        | ConversationOperationFailureKind::ProviderSessionNotFound
        | ConversationOperationFailureKind::ProviderRejected => 4,
        ConversationOperationFailureKind::OutcomeUnknown => 5,
    };
    let rendered = if json_output {
        serde_json::json!({"kind":"error","error":failure}).to_string()
    } else {
        format!("Error: {:?}\n{:?}", failure.kind, failure.message)
    };
    let _written = if json_output {
        writeln!(std::io::stdout(), "{rendered}")
    } else {
        writeln!(std::io::stderr(), "{rendered}")
    };
    exit
}
