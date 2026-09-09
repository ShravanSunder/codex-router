use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_protocol::OperationId;
use std::os::unix::fs::DirBuilderExt;

#[tokio::test]
async fn cli_creates_and_reads_wakeup_through_host() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange: a real owned Control service and CLI subprocess; no native process/model.
    let root = std::path::PathBuf::from(format!(
        "/tmp/wake-cli-{}",
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
    // Act: the documented CLI operation must reach Host-created persistent state.
    let target=serde_json::json!({"endpoint":{"serviceId":runtime.service_id(),"endpointId":"codex-local"},"sessionId":"fixture-only-new-thread"}).to_string();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "wake",
            "send",
            "--to",
            &target,
            "--human-user",
            "--every",
            "10m",
            "--for",
            "2h",
            "--text",
            "Check repository",
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !output.status.success() {
        return Err(format!(
            "CLI create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let record: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let id = record
        .pointer("/result/definition/wakeupId")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing created instruction identity")?;
    let read = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "wake",
            "show",
            "--wakeup-id",
            id,
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    // Assert: machine-readable read returns the exact text, with no native backend involved.
    if !read.status.success() {
        return Err("CLI show failed".into());
    }
    let record: serde_json::Value = serde_json::from_slice(&read.stdout)?;
    if record
        .pointer("/result/definition/message/content/text")
        .and_then(serde_json::Value::as_str)
        != Some("Check repository")
    {
        return Err("CLI wake text was not persisted".into());
    }
    if record.pointer("/result/firstFire") != Some(&serde_json::Value::Null) {
        return Err("CLI creation invented a firing".into());
    }
    for (action, state) in [
        ("pause", "paused"),
        ("resume", "active"),
        ("cancel", "cancelled"),
    ] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
            .args([
                "wake",
                action,
                "--wakeup-id",
                id,
                "--json",
                "--service-directory",
            ])
            .arg(&root)
            .output()
            .await?;
        if !output.status.success() {
            return Err(format!(
                "wake {action} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let response: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        if response
            .pointer("/result/wakeup/state")
            .and_then(serde_json::Value::as_str)
            != Some(state)
        {
            return Err(format!("wake {action} returned wrong state").into());
        }
    }
    let fire = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "wake",
            "send",
            "--to",
            &target,
            "--human-user",
            "--text",
            "Check after timer",
            "--after",
            "1s",
            "--wait-until-first-fire",
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !fire.status.success() {
        return Err(format!(
            "first-fire CLI failed: {}",
            String::from_utf8_lossy(&fire.stderr)
        )
        .into());
    }
    let last = String::from_utf8(fire.stdout)?
        .lines()
        .last()
        .ok_or("missing firing output")?
        .to_owned();
    let fired: serde_json::Value = serde_json::from_str(&last)?;
    if fired
        .pointer("/result/kind")
        .and_then(serde_json::Value::as_str)
        != Some("wakeFired")
    {
        return Err("wait returned before firing receipt".into());
    }
    let listing = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "wake",
            "list",
            "--limit",
            "1",
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !listing.status.success() {
        return Err(format!(
            "wake list failed: {}",
            String::from_utf8_lossy(&listing.stderr)
        )
        .into());
    }
    let listing: serde_json::Value = serde_json::from_slice(&listing.stdout)?;
    if listing
        .pointer("/result/records")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        != Some(1)
        || listing
            .pointer("/result/nextCursor")
            .is_none_or(serde_json::Value::is_null)
    {
        return Err("CLI bounded listing missing page or cursor".into());
    }
    let fired_id = fired
        .pointer("/result/wakeupId")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing fired wake identity")?;
    let deliveries = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "delivery",
            "list",
            "--wakeup-id",
            fired_id,
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !deliveries.status.success() {
        return Err("delivery list CLI failed".into());
    }
    let deliveries: serde_json::Value = serde_json::from_slice(&deliveries.stdout)?;
    let delivery_id = deliveries
        .pointer("/result/records/0/deliveryId")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing fired delivery")?;
    for action in ["show", "attempts", "reconcile"] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
            .args([
                "delivery",
                action,
                "--delivery-id",
                delivery_id,
                "--json",
                "--service-directory",
            ])
            .arg(&root)
            .output()
            .await?;
        if !output.status.success() {
            return Err(format!("delivery {action} CLI failed").into());
        }
        let output: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        if action == "attempts"
            && output
                .pointer("/result/coverage/earlierAttempts")
                .and_then(serde_json::Value::as_str)
                != Some("mayBeUnavailable")
        {
            return Err("delivery attempts CLI omitted retention coverage".into());
        }
    }
    runtime.shutdown().await?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected cleanup entry".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
