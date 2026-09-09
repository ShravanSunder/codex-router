//! Explicit live-test admission and fresh-thread helpers; fixture tests never call this module.
#[path = "native_notification_diagnostics.rs"]
mod native_notification_diagnostics;
use codex_native_integration::{
    NativeOperation, NativePayloadSchemas, NativeProtocolConnection, NativeSchemaExport,
};
use communication_client::ControlClient;
use communication_protocol::{ChannelDescription, CodexGeneration, EndpointRef, SessionRef};
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
pub type ProofResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

pub struct ProofContext {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub service_directory: PathBuf,
    pub endpoint: EndpointRef,
    pub generation: CodexGeneration,
    pub client: ControlClient,
    pub native: NativeProtocolConnection,
    pub schemas: Arc<NativePayloadSchemas>,
}
impl ProofContext {
    pub async fn connect() -> ProofResult<Self> {
        let root = PathBuf::from(std::env::var_os("CODEX_AUTOMATION_PROOF_ROOT").ok_or(
            "Set CODEX_AUTOMATION_PROOF_ROOT to a fresh automation-debug-host run directory",
        )?)
        .canonicalize()?;
        if root.parent() != Some(std::path::Path::new("/tmp").canonicalize()?.as_path())
            || std::fs::metadata(&root)?.permissions().mode() & 0o077 != 0
        {
            return Err("Live proof requires a private direct child of /tmp".into());
        }
        let marker: Value =
            serde_json::from_slice(&std::fs::read(root.join("debug-host-context.json"))?)?;
        if marker.get("model").and_then(Value::as_str) != Some("gpt-5.6-luna")
            || marker.get("profile").and_then(Value::as_str) != Some("codex-router-debug")
            || marker
                .get("port")
                .and_then(Value::as_u64)
                .is_none_or(|port| port == 8787 || port == 0)
        {
            return Err("Host did not establish the isolated debug profile and Luna model".into());
        }
        let workspace = root.join("agent-workspace");
        if std::fs::read_dir(&workspace)?.next().is_some() {
            return Err("Live proof requires a previously unused workspace".into());
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("proof-started.json"))?
            .write_all(b"{\"model\":\"gpt-5.6-luna\"}\n")?;
        let service_directory = root.join("agent-communication");
        let mut client =
            ControlClient::connect(&service_directory, "automation-luna-acceptance", "1").await?;
        let inventory = client.list_endpoints().await?;
        let endpoint = inventory
            .endpoints
            .iter()
            .find(|endpoint| String::from(endpoint.endpoint.endpoint_id.clone()) == "codex-local")
            .ok_or("debug native endpoint missing")?;
        let (generation, digest) = endpoint
            .channels
            .iter()
            .find_map(|channel| match channel {
                ChannelDescription::NativeCodex {
                    generation: Some(generation),
                    schema_digest: Some(digest),
                    ..
                } => Some((generation.clone(), digest.clone())),
                _ => None,
            })
            .ok_or("debug native generation/schema unavailable")?;
        let endpoint = endpoint.endpoint.clone();
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME missing")?).join(".codex");
        let paths = codex_native_integration::CodexPaths::from_codex_home(home);
        let identity =
            codex_native_integration::executable_identity(&paths.managed_executable()).await?;
        let export =
            NativeSchemaExport::generate(&identity, &root.join("proof-native-schema")).await?;
        let schemas = NativePayloadSchemas::from_bundle(export.bundle())?;
        if schemas.schema_digest() != String::from(digest) {
            return Err("Running endpoint and proof executable expose different schemas".into());
        }
        let native =
            NativeProtocolConnection::connect(&root.join("native-socket/app-server.sock")).await?;
        Ok(Self {
            root,
            workspace,
            service_directory,
            endpoint,
            generation,
            client,
            native,
            schemas: Arc::new(schemas),
        })
    }
    pub async fn start_thread(&mut self, role: &str) -> ProofResult<SessionRef> {
        let reservation = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let proxy_port = reservation.local_addr()?.port();
        let control_socket = self.service_directory.join("control.sock").canonicalize()?;
        let configuration = json!({
            "permissions.automation-proof": {"extends":":workspace","network":{"enabled":true}},
            "features.hooks": false,
            "features.network_proxy": {
                "enabled":true,"proxy_url":format!("http://127.0.0.1:{proxy_port}"),
                "enable_socks5":false,"allow_upstream_proxy":false,"allow_local_binding":false,
                "credential_broker":false,"domains":{},"unix_sockets":{control_socket.to_string_lossy().as_ref():"allow"}
            }
        });
        drop(reservation);
        let response = self.native.request_validated(&self.schemas, NativeOperation::StartThread, json!({
            "model":"gpt-5.6-luna","allowProviderModelFallback":false,"cwd":self.workspace,
            "permissions":"automation-proof","approvalPolicy":"never","config":configuration,
            "developerInstructions":format!("You are {role} in an isolated local acceptance test. Follow the supplied test task. Use only the supplied test commands and this fresh workspace. Never inspect credentials, change configuration, access production services or existing user threads. Do not spawn other agents. A message receipt proves acceptance, not completion.")
        })).await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let rejection = self.native.take_last_rejection();
                self.record(
                    "nativeThreadRejected",
                    json!({"role":role,"rejection":rejection}),
                )?;
                return Err(error.into());
            }
        };
        if response.get("model").and_then(Value::as_str) != Some("gpt-5.6-luna")
            || response.get("modelProvider").and_then(Value::as_str) != Some("codex-router-debug")
        {
            return Err(
                "Native thread did not confirm Luna on the debug provider; no turn was submitted"
                    .into(),
            );
        }
        let target = SessionRef {
            endpoint: self.endpoint.clone(),
            session_id: response
                .pointer("/thread/id")
                .and_then(Value::as_str)
                .ok_or("new native thread identity missing")?
                .to_owned()
                .try_into()?,
        };
        self.record(
            "freshThread",
            json!({"role":role,"target":target,"model":"gpt-5.6-luna"}),
        )?;
        Ok(target)
    }
    pub async fn turns(&mut self, target: &SessionRef) -> ProofResult<Vec<Value>> {
        while let Some(message) = self.native.take_buffered_message() {
            if let Some(diagnostic) = native_notification_diagnostics::for_target(
                &message,
                &String::from(target.session_id.clone()),
            ) {
                self.record("nativeErrorNotification", diagnostic)?;
            }
            if message.get("id").is_some() && message.get("method").is_some() {
                self.record(
                    "unexpectedNativeRequest",
                    json!({"method":message.get("method")}),
                )?;
                return Err(
                    "Native requested an unexpected callback during approval-never proof".into(),
                );
            }
        }
        let response = self
            .native
            .request_validated(
                &self.schemas,
                NativeOperation::ReadThread,
                json!({"threadId":String::from(target.session_id.clone()),"includeTurns":true}),
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let rejection = self.native.take_last_rejection();
                // Upstream thread_read_history_load_error explicitly rejects new empty threads.
                // Match both the code and exact fresh target before treating it as waiting.
                let expected = format!(
                    "thread {} is not materialized yet; includeTurns is unavailable before first user message",
                    String::from(target.session_id.clone())
                );
                if matches!(
                    error,
                    codex_native_integration::NativeConnectionError::Rejected { code: -32600 }
                ) && rejection
                    .as_ref()
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    == Some(expected.as_str())
                {
                    self.record(
                        "unmaterializedHistory",
                        json!({"target":target,"code":-32600}),
                    )?;
                    return Ok(Vec::new());
                }
                // Live proof established a short first-input metadata gap on 0.153.4:
                // pagination rejects before the row exists, then the identical read succeeds.
                // The caller's bounded observation must still obtain actual history and receipts.
                if matches!(
                    error,
                    codex_native_integration::NativeConnectionError::Rejected { code: -32601 }
                ) && rejection
                    .as_ref()
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    == Some("list_turns is not supported yet")
                {
                    self.record(
                        "historyNotReady",
                        json!({"target":target,"rejection":rejection}),
                    )?;
                    return Ok(Vec::new());
                }
                self.record(
                    "nativeHistoryRejected",
                    json!({"target":target,"rejection":rejection}),
                )?;
                return Err(error.into());
            }
        };
        if response.pointer("/thread/id").and_then(Value::as_str)
            != Some(String::from(target.session_id.clone()).as_str())
        {
            return Err("History response selected a different thread".into());
        }
        response
            .pointer("/thread/turns")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| "native turn history missing".into())
    }
    pub async fn wait_for_text(
        &mut self,
        target: &SessionRef,
        text: &str,
    ) -> ProofResult<Vec<Value>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            let turns = self.turns(target).await?;
            if turns.iter().any(|turn| agent_text(turn).contains(text)) {
                return Ok(turns);
            }
            if tokio::time::Instant::now() >= deadline {
                self.record(
                    "waitExpired",
                    json!({"target":target,"expectedMarker":text,"observedTurns":turns}),
                )?;
                return Err(format!(
                    "Luna did not emit expected marker {text}; inspect the private proof events"
                )
                .into());
            }
        }
    }
    pub async fn wait_for_arranged_wake(
        &mut self,
        origin: &SessionRef,
        turn_id: &str,
        marker: &str,
    ) -> ProofResult<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            let turns = self.turns(origin).await?;
            let wakes = self
                .client
                .list_wakeups(communication_protocol::AutomationPageRequest {
                    cursor: None,
                    limit: 100.try_into()?,
                })
                .await?;
            if let Some(wake) =
                wakes
                    .records
                    .iter()
                    .find(|wake| match &wake.definition.message.content {
                        communication_protocol::MessageContent::Agent { text, .. } => {
                            text.as_str().starts_with(marker)
                        }
                        _ => false,
                    })
            {
                self.record(
                    "wakeArrangedByAgent",
                    json!({"wakeupId":wake.definition.wakeup_id}),
                )?;
                return Ok(());
            }
            if let Some(turn) = turns.iter().find(|turn| {
                turn.get("id").and_then(Value::as_str) == Some(turn_id)
                    && matches!(
                        turn.get("status").and_then(Value::as_str),
                        Some("completed" | "failed" | "interrupted")
                    )
            }) {
                self.record(
                    "senderStoppedBeforeWake",
                    json!({"target":origin,"turn":turn}),
                )?;
                return Err(
                    "Sender stopped without arranging the wake; inspect its private tool output"
                        .into(),
                );
            }
            if tokio::time::Instant::now() >= deadline {
                self.record(
                    "senderWaitExpired",
                    json!({"target":origin,"observedTurns":turns}),
                )?;
                return Err("Sender did not arrange a wake before the proof deadline".into());
            }
        }
    }
    pub fn record(&self, event: &str, details: Value) -> ProofResult<()> {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(self.root.join("proof-events.jsonl"))?;
        writeln!(file, "{}", json!({"event":event,"details":details}))?;
        println!("Luna acceptance: {event}");
        Ok(())
    }
}
pub fn agent_text(turn: &Value) -> String {
    turn.get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("agentMessage"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub async fn python_executable() -> ProofResult<PathBuf> {
    let output = tokio::process::Command::new("python3")
        .args([
            "-c",
            "import sys; assert sys.version_info >= (3, 12); print(sys.executable)",
        ])
        .output()
        .await?;
    if !output.status.success() {
        return Err("Python 3.12 or newer is required for the permission probe".into());
    }
    Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()).canonicalize()?)
}
