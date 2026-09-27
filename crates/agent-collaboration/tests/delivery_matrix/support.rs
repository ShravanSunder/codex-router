//! Reusable recipient and transport fixtures for the opt-in delivery matrix.
use super::proof_context::{ProofContext, ProofResult};
use collaboration_client::protocol::{EndpointId, SessionId, SessionRef};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{os::unix::fs::PermissionsExt as _, path::PathBuf, time::Duration};
use tokio::io::{AsyncBufReadExt as _, BufReader};

const PEER_TOKEN: &str = "0123456789abcdef0123456789abcdef";

pub(super) async fn mcp_send(
    proof: &ProofContext,
    sender: &SessionRef,
    target: &SessionRef,
    text: &str,
) -> ProofResult<()> {
    let manifest: Value = serde_json::from_slice(&std::fs::read(
        proof.service_directory.join("service.json"),
    )?)?;
    let url = manifest["mcp"]["url"].as_str().ok_or("MCP URL missing")?;
    if !url.starts_with("http://127.0.0.1:") || url.contains(":8788/") {
        return Err("MCP proof URL was not isolated loopback".into());
    }
    let client = reqwest::Client::new();
    let initialize = client.post(url).header("accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"delivery-matrix","version":"1"}}}))
        .send().await?;
    let session = initialize
        .headers()
        .get("mcp-session-id")
        .ok_or("MCP session ID missing")?
        .to_str()?
        .to_owned();
    let _initialized = client
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
        .send()
        .await?;
    let catalog = client
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .send()
        .await?;
    let catalog = mcp_json(catalog).await?;
    let output_schema = catalog["result"]["tools"]
        .as_array()
        .ok_or("MCP tools missing")?
        .iter()
        .find(|tool| tool["name"] == "message_send")
        .and_then(|tool| tool.get("outputSchema"))
        .ok_or("message_send schema missing")?;
    let response = client.post(url).header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session).header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"message_send","arguments":{"target":target,"message":{"kind":"agent","sender":sender,"text":text},"delivery":"auto"}}}))
        .send().await?;
    let response = mcp_json(response).await?;
    if response["result"]["isError"] == true {
        return Err("MCP message_send returned an error".into());
    }
    let structured = response["result"]["structuredContent"].clone();
    jsonschema::validator_for(output_schema)?
        .validate(&structured)
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn mcp_json(response: reqwest::Response) -> ProofResult<Value> {
    let text = response.text().await?;
    if text.starts_with("data:") || text.starts_with("event:") {
        let body = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(serde_json::from_str(&body)?)
    } else {
        Ok(serde_json::from_str(&text)?)
    }
}

pub(super) struct ConfigHashGuard {
    path: PathBuf,
    expected: Vec<u8>,
}

impl ConfigHashGuard {
    pub(super) fn capture() -> ProofResult<Self> {
        let path = PathBuf::from(std::env::var_os("HOME").ok_or("HOME unavailable")?)
            .join(".codex/config.toml");
        let expected = Sha256::digest(std::fs::read(&path)?).to_vec();
        Ok(Self { path, expected })
    }
    pub(super) fn verify(&self) -> ProofResult<()> {
        let observed = Sha256::digest(std::fs::read(&self.path)?);
        if observed.as_slice() != self.expected.as_slice() {
            return Err("Owner Codex config hash changed during delivery matrix; stop and report without restoration".into());
        }
        Ok(())
    }
}

pub(super) struct PeerFixture {
    pub(super) target: SessionRef,
    received: tokio::sync::mpsc::Receiver<String>,
    stop: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<ProofResult<()>>,
}

impl PeerFixture {
    pub(super) async fn start(proof: &ProofContext) -> ProofResult<Self> {
        let registry = proof.root.join("claude-peer-fixture");
        if !registry.is_dir() || std::fs::metadata(&registry)?.permissions().mode() & 0o077 != 0 {
            return Err("Debug Host did not enable a private fixture peer registry".into());
        }
        let socket = registry.join("peer.sock");
        let listener = tokio::net::UnixListener::bind(&socket)?;
        let pid = std::process::id();
        std::fs::write(
            registry.join(format!("{pid}.json")),
            json!({
                "pid":pid,"sessionId":"fixture-session","peerProtocol":1,
                "messagingSocketPath":socket,"status":"busy"
            })
            .to_string(),
        )?;
        let digest = Sha256::digest(socket.to_string_lossy().as_bytes());
        let key = registry.join(format!("{pid}.{digest:x}.key"));
        std::fs::write(&key, json!({"peerToken":PEER_TOKEN}).to_string())?;
        std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600))?;
        let (transmit, received) = tokio::sync::mpsc::channel(16);
        let stop = tokio_util::sync::CancellationToken::new();
        let task_stop = stop.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = task_stop.cancelled() => return Ok(()),
                    accepted = listener.accept() => {
                        let (stream, _) = accepted?;
                        let mut lines = BufReader::new(stream).lines();
                        let _auth = lines.next_line().await?.ok_or("peer auth missing")?;
                        let user: Value = serde_json::from_str(&lines.next_line().await?.ok_or("peer user frame missing")?)?;
                        let text = user["message"]["content"].as_str().ok_or("peer text missing")?.to_owned();
                        transmit.send(text).await.map_err(|_| "peer receiver closed")?;
                    }
                }
            }
        });
        let mut target = SessionRef {
            endpoint: proof.endpoint.clone(),
            session_id: SessionId::try_from("fixture-session".to_owned())?,
        };
        target.endpoint.endpoint_id = EndpointId::try_from("claude-local".to_owned())?;
        target.session_id = SessionId::try_from("fixture-session".to_owned())?;
        Ok(Self {
            target,
            received,
            stop,
            task,
        })
    }
    pub(super) async fn expect_marker(&mut self, marker: &str) -> ProofResult<()> {
        self.expect_marker_with_timeout(marker, Duration::from_secs(30))
            .await
    }
    pub(super) async fn expect_marker_with_timeout(
        &mut self,
        marker: &str,
        timeout: Duration,
    ) -> ProofResult<()> {
        let received = tokio::time::timeout(timeout, self.received.recv())
            .await?
            .ok_or("peer fixture stopped before message")?;
        if !received.contains(marker) {
            return Err("peer frame omitted matrix marker".into());
        }
        Ok(())
    }
    pub(super) async fn shutdown(self) -> ProofResult<()> {
        self.stop.cancel();
        self.task.await??;
        Ok(())
    }
}
