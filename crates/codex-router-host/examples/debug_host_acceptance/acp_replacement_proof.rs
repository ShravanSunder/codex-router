//! Owned Host restart coordinated with independent ACP loss and explicit recovery.
use super::owned_thread_registry::OwnedThreadRegistry;
use codex_native_integration::NativeProtocolConnection;
use communication_client::ControlClient;
use communication_protocol::{ChannelDescription, CodexGeneration, EndpointAvailability};
use serde_json::{Value, json};
use std::{error::Error, os::unix::fs::OpenOptionsExt, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{ChildStdout, Command},
};

pub struct ReplacementProofRequest<'a> {
    pub directory: &'a Path,
    pub cwd: &'a Path,
    pub router_cli: &'a Path,
    pub sdk: &'a Path,
    pub native_socket: &'a Path,
}
pub async fn run_replacement_proof(
    native: &mut NativeProtocolConnection,
    owned: &mut OwnedThreadRegistry,
    request: ReplacementProofRequest<'_>,
) -> Result<(), Box<dyn Error>> {
    let target = owned.create(native, request.cwd).await?;
    let receipt = owned
        .submit_text(
            native,
            &target,
            "Do not use tools. Reply exactly ACP_REPLACEMENT_READY.",
        )
        .await?;
    if owned.observe_text(native, receipt).await?.trim() != "ACP_REPLACEMENT_READY" {
        return Err("ACP replacement preparation failed".into());
    }
    let before = current_generation(request.directory)
        .await?
        .ok_or("backend generation unavailable")?;
    println!(
        "{}",
        json!({"kind":"ownedReplacementThreadCreated","threadId":target,"model":"gpt-5.6-luna","generation":before})
    );
    let diagnostics = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(
            request
                .cwd
                .join("tmp")
                .join(format!("acp-replacement-{}.stderr", std::process::id())),
        )?;
    let mut child = Command::new("node")
        .kill_on_drop(true)
        .arg(
            request
                .cwd
                .join("scripts/proof-tools/acp-replacement-proof.mjs"),
        )
        .arg(request.sdk)
        .arg(request.directory.join("codex-acp.sock"))
        .arg(&target)
        .arg(request.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(diagnostics))
        .spawn()?;
    let mut input = child.stdin.take().ok_or("ACP client stdin missing")?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("ACP client stdout missing")?).lines();
    let ready = read_record(&mut lines, "restartRequested").await?;
    if ready.get("sessionId").and_then(Value::as_str) != Some(target.as_str()) {
        return Err("ACP restart request target changed".into());
    }
    // Only the already-owned debug Host's private operator socket is addressed.
    let root = request
        .directory
        .parent()
        .ok_or("debug runtime root missing")?;
    if root.file_name().and_then(|name| name.to_str()) != Some(".codex-router-debug") {
        return Err("replacement proof requires debug root".into());
    }
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        Command::new(request.router_cli)
            .args([
                "host",
                "restart",
                "--require-debug-isolation",
                "--router-root",
            ])
            .arg(root)
            .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if !output.status.success() {
        return Err("owned debug restart command failed".into());
    }
    let loss = read_record(&mut lines, "generationLost").await?;
    let after = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(generation) = current_generation(request.directory).await?
                && generation.service_epoch == before.service_epoch
                && u64::from(generation.generation) > u64::from(before.generation)
            {
                return Ok::<_, Box<dyn Error>>(generation);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    input.write_all(b"reconnect\n").await?;
    let created = read_record(&mut lines, "newSessionCreated").await?;
    let loaded = read_record(&mut lines, "reloadedSessionReady").await?;
    if loaded.get("sessionId").and_then(Value::as_str) != Some(target.as_str()) {
        return Err("ACP recovered another thread".into());
    }
    owned.require_owned(&target)?;
    let mut successor = NativeProtocolConnection::connect(request.native_socket).await?;
    let thread = successor.inspect_thread(&target).await?;
    if thread.get("model").and_then(Value::as_str) != Some("gpt-5.6-luna") {
        return Err("recovered ACP target model is not Luna; no prompt sent".into());
    }
    input.write_all(b"prompt\n").await?;
    let result = read_record(&mut lines, "independentAcpRecoveryPassed").await?;
    drop(input);
    if !tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await??
        .success()
    {
        return Err("independent ACP recovery process failed".into());
    }
    println!(
        "{}",
        json!({"kind":"ownedAcpReplacementPassed","threadId":target,"before":before,"after":after,"loss":loss,"newSession":created,"clientReceipt":result})
    );
    Ok(())
}
async fn read_record(
    lines: &mut Lines<BufReader<ChildStdout>>,
    expected: &str,
) -> Result<Value, Box<dyn Error>> {
    let line = tokio::time::timeout(Duration::from_secs(90), lines.next_line())
        .await??
        .ok_or_else(|| format!("ACP recovery client closed before {expected}"))?;
    if line.len() > 65536 {
        return Err("ACP proof receipt exceeds bound".into());
    }
    let value: Value = serde_json::from_str(&line)?;
    println!("{value}");
    if value.get("kind").and_then(Value::as_str) != Some(expected) {
        return Err("unexpected ACP recovery phase".into());
    }
    Ok(value)
}
async fn current_generation(directory: &Path) -> Result<Option<CodexGeneration>, Box<dyn Error>> {
    let mut client =
        ControlClient::connect(directory, "replacement-proof", env!("CARGO_PKG_VERSION")).await?;
    let inventory = client.list_endpoints().await?;
    client.close().await?;
    Ok(inventory
        .endpoints
        .into_iter()
        .filter(|endpoint| {
            String::from(endpoint.endpoint.endpoint_id.clone()) == "codex-local"
                && matches!(
                    endpoint.availability,
                    EndpointAvailability::Available { .. }
                )
        })
        .flat_map(|endpoint| endpoint.channels)
        .find_map(|channel| match channel {
            ChannelDescription::NativeCodex { generation, .. } => generation,
            _ => None,
        }))
}
