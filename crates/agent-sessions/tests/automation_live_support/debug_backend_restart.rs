//! Restart only the verified acceptance Host's retained native child through its existing CLI.
use super::proof_context::{ProofContext, ProofResult};
use codex_native_integration::NativeProtocolConnection;
use communication_protocol::ChannelDescription;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub async fn restart(proof: &mut ProofContext) -> ProofResult<()> {
    let marker: Value =
        serde_json::from_slice(&std::fs::read(proof.root.join("debug-host-context.json"))?)?;
    let original_directory = marker
        .get("runDirectory")
        .and_then(Value::as_str)
        .ok_or("Debug Host run-directory identity missing")?;
    if Path::new(original_directory).canonicalize()? != proof.root {
        return Err("Debug Host marker names a different root; no restart requested".into());
    }
    let pid = marker
        .get("hostPid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0)
        .ok_or("Owned Host PID missing")?;
    let inspected = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("ps")
            .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    let command = String::from_utf8(inspected.stdout)?;
    if !inspected.status.success()
        || !command.contains("automation-debug-host")
        || !command.contains(&format!("--run-directory {original_directory}"))
    {
        return Err("PID does not identify this acceptance Host; no restart requested".into());
    }
    let port = marker
        .get("port")
        .and_then(Value::as_u64)
        .filter(|port| *port > 0 && *port <= u64::from(u16::MAX) && *port != 8787)
        .ok_or("Debug provider port missing")?;
    let binary = Path::new(env!("CARGO_BIN_EXE_agent-sessions")).with_file_name("codex-router");
    if !binary.is_file() {
        return Err("Adjacent compiled debug router CLI missing".into());
    }
    let previous = proof.generation.clone();
    proof.record(
        "ownedBackendRestartRequested",
        json!({"hostPid":pid,"generation":previous}),
    )?;
    let output = tokio::time::timeout(
        Duration::from_secs(130),
        tokio::process::Command::new(binary)
            .args(["host", "restart", "--router-root"])
            .arg(&proof.root)
            .args(["--port", &port.to_string(), "--require-debug-isolation"])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    let succeeded = output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains("result: Succeeded");
    proof.record("ownedBackendRestartResult", json!({"exitCode":output.status.code(),"succeeded":succeeded,"stdout":String::from_utf8_lossy(&output.stdout)}))?;
    if !succeeded {
        return Err(
            "Owned app-server restart did not report success; inspect private proof events".into(),
        );
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    loop {
        interval.tick().await;
        let inventory = proof.client.list_endpoints().await?;
        let replacement = inventory
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.endpoint == proof.endpoint)
            .flat_map(|endpoint| &endpoint.channels)
            .find_map(|channel| match channel {
                ChannelDescription::NativeCodex {
                    generation: Some(generation),
                    schema_digest: Some(digest),
                    ..
                } if generation != &previous => Some((generation.clone(), digest.clone())),
                _ => None,
            });
        if let Some((generation, digest)) = replacement {
            if String::from(digest) != proof.schemas.schema_digest() {
                return Err(
                    "Native schema changed during the proof; no further input will be submitted"
                        .into(),
                );
            }
            proof.generation = generation;
            proof.native = NativeProtocolConnection::connect(
                &proof.root.join("native-socket/app-server.sock"),
            )
            .await?;
            proof.record(
                "ownedBackendReplacementReady",
                json!({"before":previous,"after":proof.generation}),
            )?;
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Replacement did not publish a new native generation".into());
        }
    }
}
