//! Reusable recipient and transport fixtures for the opt-in delivery matrix.
use super::proof_context::{ProofContext, ProofResult};
use collaboration_client::protocol::{EndpointId, SessionId, SessionRef};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _, symlink},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt as _, BufReader};

const PEER_TOKEN: &str = "0123456789abcdef0123456789abcdef";

enum ProviderFixtureMode {
    CodexAndPeerRecipients,
    AcpTarget,
}

pub(super) fn prepare_provider_fixture() -> ProofResult<()> {
    prepare_fixture(ProviderFixtureMode::CodexAndPeerRecipients)
}

pub(super) fn prepare_acp_target_fixture() -> ProofResult<()> {
    prepare_fixture(ProviderFixtureMode::AcpTarget)
}

fn prepare_fixture(mode: ProviderFixtureMode) -> ProofResult<()> {
    let root = PathBuf::from(
        std::env::var_os("CODEX_AUTOMATION_PROOF_ROOT")
            .ok_or("Set CODEX_AUTOMATION_PROOF_ROOT to a new direct child of /tmp")?,
    );
    if root.parent() != Some(Path::new("/tmp")) || root.exists() {
        return Err("Matrix root must be a new direct child of /tmp".into());
    }
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    for directory in [
        root.join("home"),
        root.join("home/.claude"),
        root.join("home/.claude/sessions"),
        root.join("codex-home"),
        root.join("codex-home/packages"),
        root.join("codex-home/packages/standalone"),
        root.join("codex-home/packages/standalone/current"),
        root.join("native-socket"),
        root.join("agent-workspace"),
    ] {
        std::fs::DirBuilder::new().mode(0o700).create(directory)?;
    }
    let owner_home =
        PathBuf::from(std::env::var_os("HOME").ok_or("owner HOME missing")?).canonicalize()?;
    if owner_home.starts_with(root.canonicalize()?) {
        return Err("Owner HOME cannot be inside the matrix root".into());
    }
    let managed_executable = owner_home.join(".codex/packages/standalone/current/codex");
    if !managed_executable.is_file() {
        return Err("Managed Codex executable missing from normal home".into());
    }
    symlink(
        managed_executable,
        root.join("codex-home/packages/standalone/current/codex"),
    )?;
    let profile = "model = \"gpt-5.6-luna\"\nmodel_reasoning_effort = \"high\"\nmodel_provider = \"codex-router-debug\"\n\n[model_providers.codex-router-debug]\nname = \"isolated matrix router\"\nbase_url = \"http://127.0.0.1:43127/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\nsupports_websockets = true\n";
    write_private_file(
        &root.join("codex-home/codex-router-debug.config.toml"),
        profile.as_bytes(),
    )?;
    write_private_file(
        &root.join("debug-host-context.json"),
        serde_json::to_string(&json!({
            "kind":"isolatedDeliveryMatrix",
            "profile":"codex-router-debug",
            "model":"gpt-5.6-luna",
            "port":43127,
            "ownerHome":owner_home,
        }))?
        .as_bytes(),
    )?;
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("crate parent missing")?
        .join("codex-router-host/src/external_provider_runtime/acp_scripted_fixture.py")
        .canonicalize()?;
    let mut steps = vec![
        json!({"action":"expect_request","requestName":"initialize","method":"initialize","params":{"protocolVersion":1}}),
        json!({"action":"respond","requestName":"initialize","result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"delivery-matrix-fixture","version":"1"}}}),
    ];
    for (index, session_id) in ["matrix-provider-codex", "matrix-provider-claude"]
        .into_iter()
        .enumerate()
    {
        let request_id = 91 + index;
        let create_name = format!("create-{index}");
        let prompt_name = format!("prompt-{index}");
        steps.extend([
            json!({"action":"expect_request","requestName":create_name,"method":"session/new","params":{}}),
            json!({"action":"respond","requestName":create_name,"result":{"sessionId":session_id}}),
            json!({"action":"expect_request","requestName":prompt_name,"method":"session/prompt","params":{"sessionId":session_id}}),
            json!({"action":"send","message":{"jsonrpc":"2.0","id":request_id,"method":"session/request_permission","params":{"sessionId":session_id,"toolCall":{"toolCallId":format!("matrix-permission-{index}"),"title":"Approve matrix command","kind":"execute"},"options":[{"optionId":"allow-once","name":"Allow once","kind":"allow_once"},{"optionId":"deny-once","name":"Deny once","kind":"reject_once"}]}}}),
            json!({"action":"expect_message","message":{"jsonrpc":"2.0","id":request_id,"result":{"outcome":{"outcome":"selected"}}}}),
            json!({"action":"respond","requestName":prompt_name,"result":{"stopReason":"end_turn"}}),
        ]);
    }
    let config = if matches!(mode, ProviderFixtureMode::AcpTarget) {
        let receipt_path = root.join("provider-prompt-receipts.jsonl");
        let gate_path = root.join("busy-prompt-gate.sock");
        let mut target_steps = vec![
            json!({"action":"expect_request","requestName":"initialize","method":"initialize","params":{"protocolVersion":1}}),
            json!({"action":"respond","requestName":"initialize","result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"matrix-recipient","version":"1"}}}),
            json!({"action":"expect_request","requestName":"create-target","method":"session/new","params":{}}),
            json!({"action":"respond","requestName":"create-target","result":{"sessionId":"matrix-acp-target"}}),
        ];
        // Board subscriptions emit one neutral notice, with no lifecycle heartbeat.
        const HELD_PROMPT_INDEX: usize = 6;
        for index in 0..super::ACP_TARGET_EXPECTED_PROMPTS {
            let request_name = format!("target-prompt-{index}");
            target_steps.push(json!({"action":"expect_request","requestName":request_name,"method":"session/prompt","params":{"sessionId":"matrix-acp-target"},"recordPath":receipt_path}));
            if index == HELD_PROMPT_INDEX {
                target_steps
                    .push(json!({"action":"wait_for_socket_signal","socketPath":gate_path}));
            }
            target_steps.push(json!({"action":"respond","requestName":request_name,"result":{"stopReason":"end_turn"}}));
        }
        let requester_steps = vec![
            json!({"action":"expect_request","requestName":"initialize","method":"initialize","params":{"protocolVersion":1}}),
            json!({"action":"respond","requestName":"initialize","result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"matrix-approval-requester","version":"1"}}}),
            json!({"action":"expect_request","requestName":"create-requester","method":"session/new","params":{}}),
            json!({"action":"respond","requestName":"create-requester","result":{"sessionId":"matrix-approval-requester"}}),
            json!({"action":"expect_request","requestName":"approval-prompt","method":"session/prompt","params":{"sessionId":"matrix-approval-requester"}}),
            json!({"action":"send","message":{"jsonrpc":"2.0","id":191,"method":"session/request_permission","params":{"sessionId":"matrix-approval-requester","toolCall":{"toolCallId":"matrix-target-approval","title":"Approve matrix command","kind":"execute"},"options":[{"optionId":"allow-once","name":"Allow once","kind":"allow_once"},{"optionId":"deny-once","name":"Deny once","kind":"reject_once"}]}}}),
            json!({"action":"expect_message","message":{"jsonrpc":"2.0","id":191,"result":{"outcome":{"outcome":"selected"}}}}),
            json!({"action":"respond","requestName":"approval-prompt","result":{"stopReason":"end_turn"}}),
        ];
        json!({"version":1,"providers":{
            "claude":{"enabled":true,"executable":"/usr/bin/env","arguments":["python3","-u",&script,serde_json::to_string(&requester_steps)?]},
            "cursor":{"enabled":true,"executable":"/usr/bin/env","arguments":["python3","-u",&script,serde_json::to_string(&target_steps)?]}
        }})
    } else {
        json!({"version":1,"providers":{
            "claude":{"enabled":false,"executable":null,"arguments":[]},
            "cursor":{"enabled":true,"executable":"/usr/bin/env","arguments":["python3","-u",script,serde_json::to_string(&steps)?]}
        }})
    };
    write_private_file(
        &root.join("providers.json"),
        format!("{}\n", serde_json::to_string_pretty(&config)?).as_bytes(),
    )?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> ProofResult<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

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
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl ConfigHashGuard {
    pub(super) fn capture() -> ProofResult<Self> {
        let root = PathBuf::from(
            std::env::var_os("CODEX_AUTOMATION_PROOF_ROOT")
                .ok_or("CODEX_AUTOMATION_PROOF_ROOT missing")?,
        );
        let marker: Value =
            serde_json::from_slice(&std::fs::read(root.join("debug-host-context.json"))?)?;
        let owner_home = match marker.get("kind").and_then(Value::as_str) {
            Some("isolatedDeliveryMatrix") => PathBuf::from(
                marker
                    .get("ownerHome")
                    .and_then(Value::as_str)
                    .ok_or("Isolated matrix marker omitted owner home")?,
            ),
            Some("debugHostPrepared") => PathBuf::from(
                std::env::var_os("HOME")
                    .ok_or("Normal HOME missing for the documented debug Host context")?,
            ),
            _ => return Err(
                "Config hash capture requires a supported isolated matrix or debug Host context"
                    .into(),
            ),
        };
        let files = [
            owner_home.join(".codex/config.toml"),
            owner_home.join(".claude/settings.json"),
        ]
        .into_iter()
        .map(|path| {
            let expected = match std::fs::read(&path) {
                Ok(bytes) => Some(Sha256::digest(bytes).to_vec()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            Ok((path, expected))
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;
        Ok(Self { files })
    }
    pub(super) fn verify(&self) -> ProofResult<()> {
        for (path, expected) in &self.files {
            let observed = match std::fs::read(path) {
                Ok(bytes) => Some(Sha256::digest(bytes).to_vec()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if &observed != expected {
                return Err(
                    "Owner Codex or Claude settings changed during delivery matrix; stop and report without restoration".into(),
                );
            }
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
        let registry = proof.root.join("home/.claude/sessions");
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
                        let auth: Value = serde_json::from_str(
                            &lines.next_line().await?.ok_or("peer auth missing")?,
                        )?;
                        if auth["type"] != "auth" || auth["token"] != PEER_TOKEN {
                            return Err("peer fixture received invalid authentication".into());
                        }
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
        self.expect_text_with_timeout(marker, timeout).await?;
        Ok(())
    }
    pub(super) async fn expect_text_with_timeout(
        &mut self,
        marker: &str,
        timeout: Duration,
    ) -> ProofResult<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let received = tokio::time::timeout_at(deadline, self.received.recv())
                .await?
                .ok_or("peer fixture stopped before message")?;
            if super::user_text_contains_marker(&received, marker) {
                return Ok(received);
            }
        }
    }
    pub(super) async fn shutdown(self) -> ProofResult<()> {
        self.stop.cancel();
        self.task.await??;
        Ok(())
    }
}
