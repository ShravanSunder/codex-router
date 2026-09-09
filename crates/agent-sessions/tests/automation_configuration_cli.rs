use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_protocol::OperationId;
use serde_json::Value;
use std::os::unix::fs::DirBuilderExt;
#[tokio::test]
async fn cli_configures_and_inspects_future_attempt_budgets()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "automation-config-cli-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let runtime = CommunicationRuntime::start(CommunicationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("absent.sock"),
        native_schema: None,
    })
    .await?;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "automation",
            "configure",
            "--execution-timeout-seconds",
            "120",
            "--summary-timeout-seconds",
            "30",
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !output.status.success() {
        return Err(format!(
            "configuration CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args(["automation", "status", "--json", "--service-directory"])
        .arg(&root)
        .output()
        .await?;
    let value: Value = serde_json::from_slice(&status.stdout)?;
    if !status.status.success()
        || value
            .pointer("/result/configuration/executionTimeoutSeconds")
            .and_then(Value::as_u64)
            != Some(120)
        || value
            .pointer("/result/configuration/summaryTimeoutSeconds")
            .and_then(Value::as_u64)
            != Some(30)
    {
        return Err("CLI configured values not reflected by Host".into());
    }
    let configured: Value = serde_json::from_slice(&output.stdout)?;
    let operation_id = configured
        .get("operationId")
        .and_then(Value::as_str)
        .ok_or("configuration operation ID missing")?;
    let receipt = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "operation",
            "show",
            "--operation-id",
            operation_id,
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !receipt.status.success() {
        return Err("operation show CLI failed".into());
    }
    let receipt: Value = serde_json::from_slice(&receipt.stdout)?;
    if receipt
        .pointer("/result/state/kind")
        .and_then(Value::as_str)
        != Some("succeeded")
        || receipt
            .pointer("/result/state/outcome/result/executionTimeoutSeconds")
            .and_then(Value::as_u64)
            != Some(120)
    {
        return Err("operation CLI omitted the original configuration receipt".into());
    }
    runtime.shutdown().await?;
    for entry in std::fs::read_dir(&root)? {
        std::fs::remove_file(entry?.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
