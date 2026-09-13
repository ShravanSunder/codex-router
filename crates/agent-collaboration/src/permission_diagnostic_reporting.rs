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

pub(crate) fn report_permission_error(
    error: &ClientError,
    rendering: PermissionDiagnosticRendering<'_>,
    machine_output: bool,
) -> Option<i32> {
    let diagnostic = error.permission_diagnostic()?;
    Some(report_diagnostic(&diagnostic, rendering, machine_output))
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
