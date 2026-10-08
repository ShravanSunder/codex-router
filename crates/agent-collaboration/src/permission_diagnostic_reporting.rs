use collaboration_client::ClientError;
use collaboration_client::protocol::OperationId;
use serde::Serialize;
use serde_json::json;
use std::io::{self, Write};

#[derive(Clone, Copy)]
pub(crate) enum PermissionDiagnosticRendering<'a> {
    Command,
    Operation(Option<&'a OperationId>),
}

/// Reports a client error the caller acts on before retrying, if it is one: missing socket
/// permission, or the API at its request limit (the request was not run; retry it).
pub(crate) fn report_actionable_client_error(
    error: &ClientError,
    rendering: PermissionDiagnosticRendering<'_>,
    machine_output: bool,
) -> Option<i32> {
    if let ClientError::Overloaded { message } = error {
        return Some(report_overload(message, rendering, machine_output));
    }
    let diagnostic = error.permission_diagnostic()?;
    Some(report_diagnostic(&diagnostic, rendering, machine_output))
}

fn report_overload(
    message: &str,
    rendering: PermissionDiagnosticRendering<'_>,
    machine_output: bool,
) -> i32 {
    let failure = json!({
        "kind": "overloaded",
        "stage": "admission",
        "effect": "none",
        "message": message,
        "nextAction": "retryLater",
    });
    if machine_output {
        let record = match rendering {
            PermissionDiagnosticRendering::Command => json!({"kind": "error", "error": failure}),
            PermissionDiagnosticRendering::Operation(operation_id) => json!({
                "kind": "error",
                "operationId": operation_id,
                "error": failure,
            }),
        };
        let _written = writeln!(io::stdout().lock(), "{record}");
    } else {
        let _written = writeln!(
            io::stderr().lock(),
            "Error: overloaded\nStage: admission\n{message}\nNext action: retryLater"
        );
    }
    3
}

pub(crate) fn report_diagnostic(
    diagnostic: &collaboration_client::protocol::PermissionDiagnostic,
    rendering: PermissionDiagnosticRendering<'_>,
    machine_output: bool,
) -> i32 {
    if machine_output {
        let record = match rendering {
            PermissionDiagnosticRendering::Command => {
                json!({"kind": "error", "error": diagnostic})
            }
            PermissionDiagnosticRendering::Operation(operation_id) => json!({
                "kind": "error",
                "operationId": operation_id,
                "error": diagnostic,
            }),
        };
        let _written = writeln!(io::stdout().lock(), "{record}");
    } else {
        let _written = writeln!(io::stderr().lock(), "{}", human_diagnostic_text(diagnostic));
    }
    3
}

pub(crate) fn human_diagnostic_text(
    diagnostic: &collaboration_client::protocol::PermissionDiagnostic,
) -> String {
    let kind = wire_name(&diagnostic.kind);
    let stage = wire_name(&diagnostic.stage);
    let next_action = wire_name(&diagnostic.next_action);
    format!(
        "Error: {kind}\nStage: {stage}\n{}\nNext action: {next_action}",
        diagnostic.message
    )
}

fn wire_name<TValue: Serialize>(value: &TValue) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unavailable".to_owned())
}
