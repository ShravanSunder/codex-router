//! Built CLI acceptance against the owned debug service; no shell command interpolation.
use communication_protocol::SessionRef;
use serde_json::Value;
use std::{error::Error, path::Path, time::Duration};

pub struct CliSubmission<'a> {
    pub executable: &'a Path,
    pub directory: &'a Path,
    pub target: &'a SessionRef,
    pub sender: &'a SessionRef,
    pub text: &'a str,
}
pub async fn submit(request: CliSubmission<'_>) -> Result<String, Box<dyn Error>> {
    let mut command = tokio::process::Command::new(request.executable);
    command
        .kill_on_drop(true)
        .args(["message", "send", "--to"])
        .arg(serde_json::to_string(request.target)?)
        .arg("--from")
        .arg(serde_json::to_string(request.sender)?)
        .arg("--text")
        .arg(request.text)
        .arg("--service-directory")
        .arg(request.directory)
        .arg("--json");
    let output = tokio::time::timeout(Duration::from_secs(40), command.output()).await??;
    if output.stdout.len() > 65536 || output.stderr.len() > 65536 {
        return Err("CLI output exceeded proof limit".into());
    }
    let record: Value = serde_json::from_slice(&output.stdout)?;
    if !output.status.success() {
        eprintln!(
            "{}",
            serde_json::json!({"kind":"cliSubmissionFailed","exitCode":output.status.code(),"error":record.get("error")})
        );
        return Err("CLI message submission failed; no replay".into());
    }
    let result = record.get("result").ok_or("CLI result missing")?;
    if result.get("target") != Some(&serde_json::to_value(request.target)?)
        || result.pointer("/acceptance/kind").and_then(Value::as_str) != Some("nativeInputAccepted")
    {
        return Err("CLI receipt target or acceptance mismatch".into());
    }
    let turn = result
        .pointer("/acceptance/turnId")
        .and_then(Value::as_str)
        .ok_or("CLI turn ID missing")?;
    println!(
        "{}",
        serde_json::json!({"kind":"ownedCliAccepted","target":request.target,"turnId":turn,"acceptance":result.get("acceptance")})
    );
    Ok(turn.to_owned())
}
