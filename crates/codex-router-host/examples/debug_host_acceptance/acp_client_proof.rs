//! Real ACP SDK and conversation CLI acceptance against owned Luna threads.
use super::{MessageProof, OwnedThreadRegistry};
use codex_native_integration::NativeProtocolConnection;
use serde_json::json;
use std::{error::Error, path::Path, time::Duration};
use tokio::process::Command;

pub(super) struct ClientProofRequest<'a> {
    pub messages: &'a MessageProof,
    pub second: &'a str,
    pub cwd: &'a Path,
    pub directory: &'a Path,
}

pub(super) async fn run_client_proof(
    owned: &OwnedThreadRegistry,
    peer: &mut NativeProtocolConnection,
    request: ClientProofRequest<'_>,
) -> Result<(), Box<dyn Error>> {
    let ClientProofRequest {
        messages,
        second,
        cwd,
        directory,
    } = request;
    if let MessageProof::Acp(sdk) = messages {
        // Materialize only our Luna-pinned thread so ACP load has persisted history.
        let receipt = owned
            .submit_text(
                peer,
                second,
                "Do not use tools. Reply exactly READY_FOR_ACP.",
            )
            .await?;
        if owned.observe_text(peer, receipt).await?.trim() != "READY_FOR_ACP" {
            return Err("ACP preparation failed".into());
        }
        let thread = peer.inspect_thread(second).await?;
        if thread.get("model").and_then(serde_json::Value::as_str) != Some("gpt-5.6-luna") {
            return Err("ACP target model is not Luna".into());
        }
        let script = cwd.join("scripts/proof-tools/acp-client-proof.mjs");
        let output = tokio::time::timeout(
            Duration::from_secs(130),
            Command::new("node")
                .kill_on_drop(true)
                .arg(script)
                .arg(sdk)
                .arg(directory.join("codex-acp.sock"))
                .arg(second)
                .arg(cwd)
                .output(),
        )
        .await??;
        if !output.status.success() {
            eprintln!(
                "Independent ACP client failed with exit {:?}",
                output.status.code()
            );
            return Err("ACP client proof failed; no replay".into());
        }
        let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        if result.get("kind").and_then(serde_json::Value::as_str)
            != Some("independentAcpPromptPassed")
        {
            return Err("ACP proof receipt missing".into());
        }
        println!("{result}");
    }
    if let MessageProof::AcpCli(executable) = messages {
        let receipt = owned
            .submit_text(
                peer,
                second,
                "Do not use tools. Reply exactly READY_FOR_ACP.",
            )
            .await?;
        if owned.observe_text(peer, receipt).await?.trim() != "READY_FOR_ACP" {
            return Err("ACP CLI preparation failed".into());
        }
        let thread = peer.inspect_thread(second).await?;
        if thread.get("model").and_then(serde_json::Value::as_str) != Some("gpt-5.6-luna") {
            return Err("ACP CLI target model is not Luna".into());
        }
        let output = tokio::time::timeout(
            Duration::from_secs(130),
            Command::new(executable)
                .kill_on_drop(true)
                .args([
                    "conversation",
                    "prompt",
                    "--endpoint",
                    "codex-local",
                    "--session",
                ])
                .arg(second)
                .arg("--cwd")
                .arg(cwd)
                .arg("--service-directory")
                .arg(directory)
                .args([
                    "--text",
                    "Do not use tools or modify files. Reply exactly ACP_CLI_LUNA_OK.",
                    "--timeout-seconds",
                    "90",
                    "--json",
                ])
                .output(),
        )
        .await??;
        if !output.status.success() {
            return Err("ACP conversation CLI failed; no replay".into());
        }
        let mut answer = String::new();
        let mut ready = false;
        let mut complete = false;
        for line in std::str::from_utf8(&output.stdout)?.lines() {
            let value: serde_json::Value = serde_json::from_str(line)?;
            match value.get("kind").and_then(serde_json::Value::as_str) {
                Some("sessionReady") => ready = true,
                Some("sessionUpdate")
                    if ready
                        && value
                            .pointer("/update/sessionUpdate")
                            .and_then(serde_json::Value::as_str)
                            == Some("agent_message_chunk") =>
                {
                    answer.push_str(
                        value
                            .pointer("/update/content/text")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default(),
                    );
                }
                Some("promptResult") => {
                    complete = value
                        .pointer("/result/stopReason")
                        .and_then(serde_json::Value::as_str)
                        == Some("end_turn")
                }
                _ => {}
            }
        }
        // Load-time history is emitted before prompt output and can include preparation text.
        if !ready || !complete || !answer.trim_end().ends_with("ACP_CLI_LUNA_OK") {
            return Err("ACP conversation CLI result mismatch".into());
        }
        println!(
            "{}",
            json!({"kind":"ownedAcpCliPassed","threadId":second,"proof":"Rust conversation client and CLI through published ACP carrier with Luna"})
        );
    }
    Ok(())
}
