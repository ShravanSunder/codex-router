//! Real Host-owned local persistence and CLI processes, without Codex or model execution.
use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_client::ControlClient;
use communication_protocol::{InstructionCreateParams, InstructionText, OperationId};
use serde_json::{Value, json};
use std::os::unix::fs::DirBuilderExt;
#[tokio::test]
async fn cli_creates_and_inspects_disabled_schedule() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp")
        .join(format!("schedule-cli-{}", OperationId::generate().as_str()));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let runtime = CommunicationRuntime::start(CommunicationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("unavailable.sock"),
        native_schema: None,
    })
    .await?;
    let mut client = ControlClient::connect(&root, "schedule-cli-fixture", "1").await?;
    let instruction = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("Inspect job status".to_owned())?,
        })
        .await?;
    client.close().await?;
    let definition = root.join("schedule-definition.json");
    std::fs::write(
        &definition,
        serde_json::to_vec(
            &json!({"instructionId":instruction.instruction_id,"timing":{"kind":"interval","seconds":600},"enabled":false,"destination":{"kind":"unprepared"},"executionTimeoutSeconds":null}),
        )?,
    )?;
    let created = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .env_remove("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET")
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .args(["schedule", "create", "--definition-file"])
        .arg(&definition)
        .args(["--json", "--service-directory"])
        .arg(&root)
        .output()
        .await?;
    if !created.status.success() {
        return Err(format!(
            "schedule CLI create failed: {}",
            String::from_utf8_lossy(&created.stderr)
        )
        .into());
    }
    let created: Value = serde_json::from_slice(&created.stdout)?;
    let id = created
        .pointer("/result/scheduleId")
        .and_then(Value::as_str)
        .ok_or("schedule identity missing")?;
    let read = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .env_remove("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET")
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .args([
            "schedule",
            "show",
            "--schedule-id",
            id,
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !read.status.success() {
        return Err("schedule CLI inspection failed".into());
    }
    let read: Value = serde_json::from_slice(&read.stdout)?;
    if read.pointer("/result/definition/enabled") != Some(&json!(false))
        || read.pointer("/result/activeRunId") != Some(&Value::Null)
    {
        return Err("disabled schedule inspection invented active work".into());
    }
    runtime.shutdown().await?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected fixture cleanup entry".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
