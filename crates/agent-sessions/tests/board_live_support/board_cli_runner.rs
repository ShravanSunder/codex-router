use crate::proof_context::ProofResult;
use serde_json::Value;
use std::{ffi::OsString, path::Path};

pub(super) async fn run_board_cli(
    service_directory: &Path,
    arguments: impl IntoIterator<Item = impl Into<OsString>>,
) -> ProofResult<Value> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"));
    command.arg("board");
    command.args(arguments.into_iter().map(Into::into));
    command
        .arg("--service-directory")
        .arg(service_directory)
        .arg("--json")
        .kill_on_drop(true);
    let output = command.output().await?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    if !output.status.success() {
        return Err(format!(
            "board CLI failed with status {:?}: {}",
            output.status.code(),
            response
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("structured error omitted message")
        )
        .into());
    }
    Ok(response)
}

pub(super) async fn run_rejected_board_cli(
    service_directory: &Path,
    expected_kind: &str,
    arguments: impl IntoIterator<Item = impl Into<OsString>>,
) -> ProofResult<Value> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"));
    command.arg("board");
    command.args(arguments.into_iter().map(Into::into));
    command
        .arg("--service-directory")
        .arg(service_directory)
        .arg("--json")
        .kill_on_drop(true);
    let output = command.output().await?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    if output.status.code() != Some(4)
        || response.pointer("/error/kind").and_then(Value::as_str) != Some(expected_kind)
    {
        return Err(format!(
            "board CLI did not return expected {expected_kind} rejection: status {:?}, response {response}",
            output.status.code()
        )
        .into());
    }
    Ok(response)
}

pub(super) fn result<'a>(response: &'a Value, pointer: &str) -> ProofResult<&'a Value> {
    response
        .pointer(&format!("/result{pointer}"))
        .ok_or_else(|| format!("board CLI result omitted {pointer}").into())
}

pub(super) fn require_record_message_ids(
    response: &Value,
    expected_message_ids: &[&str],
) -> ProofResult<()> {
    let records = result(response, "/page/records")?
        .as_array()
        .ok_or("board CLI records were not an array")?;
    for expected_message_id in expected_message_ids {
        if !records.iter().any(|record| {
            record.get("messageId").and_then(Value::as_str) == Some(*expected_message_id)
        }) {
            return Err(format!("board CLI records omitted message {expected_message_id}").into());
        }
    }
    Ok(())
}
