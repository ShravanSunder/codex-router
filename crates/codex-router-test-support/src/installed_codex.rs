//! Installed Codex smoke harness.

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::net::TcpListener;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Output;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use codex_router_cli::profile::CodexRouterProfile;
use codex_router_cli::profile::CodexRouterProfileWriter;
use codex_router_cli::token::LocalRouterTokenService;
use codex_router_cli::token::Shell;
use codex_router_cli::token::export_token_assignment;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_proxy::server::LoopbackBindAddress;
use codex_router_proxy::server::LoopbackRouterRuntime;
use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
use codex_router_proxy::upstream::UpstreamEndpoint;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::upstream_access_token_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use codex_router_state::sqlite::SqliteStateStore;
use serde_json::Value;
use tungstenite::Message;
use tungstenite::accept_hdr;
use tungstenite::client::IntoClientRequest;
use tungstenite::connect;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;

const SMOKE_EXPECTED_TEXT: &str = "codex-router smoke ok";
const CODEX_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_ACCEPT_TIMEOUT: Duration = Duration::from_secs(35);

/// Redacted report produced by the installed Codex smoke harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledCodexSmokeReport {
    transcript_path: PathBuf,
}

impl InstalledCodexSmokeReport {
    /// Returns the redacted transcript artifact path.
    #[must_use]
    pub fn transcript_path(&self) -> &PathBuf {
        &self.transcript_path
    }
}

/// Runs the installed Codex mock smoke.
pub fn run_installed_codex_mock_smoke() -> Result<InstalledCodexSmokeReport, String> {
    let smoke_root = SmokeTempRoot::new("installed-codex")?;
    let codex_home = smoke_root.path().join("codex-home");
    let workdir = smoke_root.path().join("workdir");
    let process_home = smoke_root.path().join("home");
    let xdg_config_home = smoke_root.path().join("xdg-config");
    let xdg_state_home = smoke_root.path().join("xdg-state");
    let xdg_cache_home = smoke_root.path().join("xdg-cache");
    let router_root = smoke_root.path().join("router");
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    fs::create_dir_all(&codex_home).map_err(|error| {
        format!(
            "failed to create temp Codex home {}: {error}",
            codex_home.display()
        )
    })?;
    fs::create_dir_all(&workdir).map_err(|error| {
        format!(
            "failed to create temp workdir {}: {error}",
            workdir.display()
        )
    })?;
    for temp_home_path in [
        &process_home,
        &xdg_config_home,
        &xdg_state_home,
        &xdg_cache_home,
    ] {
        fs::create_dir_all(temp_home_path).map_err(|error| {
            format!(
                "failed to create temp process home path {}: {error}",
                temp_home_path.display()
            )
        })?;
    }
    fs::create_dir_all(&router_root).map_err(|error| {
        format!(
            "failed to create temp router root {}: {error}",
            router_root.display()
        )
    })?;

    let codex_version = command_output_text(Command::new("codex").arg("--version"))?;
    let upstream = MockWebSocketUpstream::start()?;
    let (local_token_assignment, local_token) =
        seed_router_state(&state_path, &secret_root, upstream_account_token())?;
    let router_port = reserve_loopback_port()?;
    let profile_writer = CodexRouterProfileWriter::new(&codex_home);
    let profile = CodexRouterProfile::new(router_port);
    let profile_preview = profile_writer
        .dry_run(&profile)
        .map_err(|error| format!("failed to preview generated Codex profile: {error}"))?;
    let profile_path = profile_writer
        .write(&profile, true, Some(profile_preview.preview_token()))
        .map_err(|error| format!("failed to write generated Codex profile: {error}"))?;
    let router_thread = start_router_once(
        router_port,
        state_path,
        secret_root,
        local_token.clone(),
        format!("http://{}/v1", upstream.address()),
    )?;

    let http_sse_last_message_path = smoke_root.path().join("http-sse-last-message.txt");
    let http_sse_codex_output = run_codex_exec(
        CodexTransportMode::HttpSse,
        &codex_home,
        &workdir,
        &http_sse_last_message_path,
        &local_token_assignment,
        CodexChildEnvironment::new(
            &process_home,
            &xdg_config_home,
            &xdg_state_home,
            &xdg_cache_home,
        ),
    )?;
    assert_codex_visible_output(
        "HTTP/SSE",
        &http_sse_codex_output,
        &http_sse_last_message_path,
    )?;
    let websocket_last_message_path = smoke_root.path().join("websocket-last-message.txt");
    let websocket_codex_output = run_codex_exec(
        CodexTransportMode::WebSocket,
        &codex_home,
        &workdir,
        &websocket_last_message_path,
        &local_token_assignment,
        CodexChildEnvironment::new(
            &process_home,
            &xdg_config_home,
            &xdg_state_home,
            &xdg_cache_home,
        ),
    )?;
    assert_codex_visible_output(
        "WebSocket",
        &websocket_codex_output,
        &websocket_last_message_path,
    )?;
    drain_optional_router_connections(router_port, 16);
    join_result(router_thread, "router runtime")?;
    let upstream_result = upstream.join()?;
    assert_smoke_contract(
        &http_sse_codex_output.status,
        &websocket_codex_output.status,
        &upstream_result,
        &local_token,
    )?;
    let transcript_path = write_redacted_transcript(RedactedTranscriptInput {
        codex_version: codex_version.trim(),
        profile_path: &profile_path,
        http_sse_codex_status: &http_sse_codex_output.status,
        http_sse_codex_stdout: &String::from_utf8_lossy(&http_sse_codex_output.stdout),
        http_sse_codex_stderr: &String::from_utf8_lossy(&http_sse_codex_output.stderr),
        http_sse_last_message_path: &http_sse_last_message_path,
        websocket_codex_status: &websocket_codex_output.status,
        websocket_codex_stdout: &String::from_utf8_lossy(&websocket_codex_output.stdout),
        websocket_codex_stderr: &String::from_utf8_lossy(&websocket_codex_output.stderr),
        websocket_last_message_path: &websocket_last_message_path,
        upstream: &upstream_result,
    })?;

    Ok(InstalledCodexSmokeReport { transcript_path })
}

/// Runs a hostile local no-token smoke and verifies upstream remains untouched.
pub fn run_hostile_no_token_smoke() -> Result<(), String> {
    let smoke_root = SmokeTempRoot::new("hostile-no-token")?;
    let router_root = smoke_root.path().join("router");
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    fs::create_dir_all(&router_root).map_err(|error| {
        format!(
            "failed to create hostile smoke router root {}: {error}",
            router_root.display()
        )
    })?;

    let upstream = MockNoConnectionUpstream::start(Duration::from_secs(3))?;
    let (_local_token_assignment, local_token) =
        seed_router_state(&state_path, &secret_root, upstream_account_token())?;
    let router_port = reserve_loopback_port()?;
    let router_thread = start_router_once(
        router_port,
        state_path,
        secret_root,
        local_token,
        format!("http://{}/v1", upstream.address()),
    )?;

    send_hostile_no_token_websocket(router_port)?;
    drain_optional_router_connections(router_port, 15);
    join_result(router_thread, "hostile no-token router runtime")?;
    let upstream_connection_count = upstream.join()?;
    if upstream_connection_count != 0 {
        return Err(format!(
            "hostile no-token smoke reached upstream {upstream_connection_count} time(s)"
        ));
    }

    Ok(())
}

fn seed_router_state(
    state_path: &Path,
    secret_root: &Path,
    upstream_token: &str,
) -> Result<(String, String), String> {
    let state = SqliteStateStore::open(state_path)
        .map_err(|error| format!("failed to open smoke SQLite state: {error}"))?;
    let secrets = FileSecretStore::open(secret_root)
        .map_err(|error| format!("failed to open smoke secret store: {error}"))?;
    let token_service = LocalRouterTokenService::new(secrets.clone());
    let local_token = token_service
        .rotate()
        .map_err(|error| format!("failed to rotate smoke local token: {error}"))?;
    let local_token_assignment = export_token_assignment(
        "CODEX_ROUTER_TOKEN",
        local_token.token().expose_secret(),
        Shell::Posix,
    );
    let exported_token = parse_posix_token_assignment(&local_token_assignment)?;
    let account_id = account_id("acct_installed_smoke")?;
    let account = AccountRecord::new(
        account_id.clone(),
        "installed-smoke",
        AccountStatus::Enabled,
    );
    AccountStateRepository::upsert_account(&state, &account)
        .map_err(|error| format!("failed to seed smoke account: {error}"))?;
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 100);
    QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot)
        .map_err(|error| format!("failed to seed smoke quota snapshot: {error}"))?;
    let upstream_token_key = upstream_access_token_key(&account_id)
        .map_err(|error| format!("failed to build upstream token key: {error}"))?;
    secrets
        .write_secret(
            &upstream_token_key,
            &SecretString::new(upstream_token.to_owned()),
        )
        .map_err(|error| format!("failed to write smoke upstream token: {error}"))?;

    Ok((local_token_assignment, exported_token))
}

fn start_router_once(
    router_port: u16,
    state_path: PathBuf,
    secret_root: PathBuf,
    local_token: String,
    upstream_base_url: String,
) -> Result<thread::JoinHandle<Result<(), String>>, String> {
    let (ready_sender, ready_receiver) = mpsc::channel();
    let handle = thread::Builder::new()
        .name("codex-router-installed-smoke-router".to_owned())
        .spawn(move || {
            let bind_address = LoopbackBindAddress::new("127.0.0.1", router_port)
                .map_err(|error| format!("failed to build router bind address: {error}"))?;
            let upstream_endpoint = UpstreamEndpoint::new(upstream_base_url)
                .map_err(|error| format!("failed to build upstream endpoint: {error}"))?;
            let local_token = codex_router_core::local_auth::LocalRouterTokenRecord::new(
                SecretString::new(local_token),
                codex_router_core::ids::TokenGeneration::new(1),
            );
            let runtime = LoopbackRouterRuntime::start(
                LoopbackRouterRuntimeConfig::new(
                    bind_address,
                    upstream_endpoint,
                    state_path,
                    secret_root,
                    local_token,
                )
                .with_quota_clock(1_030, 60)
                .with_max_websocket_upstream_messages(4),
            )
            .map_err(|error| format!("failed to start router runtime: {error}"))?;
            if let Err(error) = ready_sender.send(()) {
                return Err(format!("failed to signal router readiness: {error}"));
            }
            runtime
                .serve_protocol_connections(16)
                .map(|_| ())
                .map_err(|error| format!("router runtime failed: {error}"))
        })
        .map_err(|error| format!("failed to spawn router runtime thread: {error}"))?;

    ready_receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("router did not become ready on port {router_port}: {error}"))?;
    Ok(handle)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodexTransportMode {
    HttpSse,
    WebSocket,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodexChildEnvironment {
    home: PathBuf,
    xdg_config_home: PathBuf,
    xdg_state_home: PathBuf,
    xdg_cache_home: PathBuf,
    path: Option<OsString>,
}

impl CodexChildEnvironment {
    fn new(
        home: &Path,
        xdg_config_home: &Path,
        xdg_state_home: &Path,
        xdg_cache_home: &Path,
    ) -> Self {
        Self {
            home: home.to_path_buf(),
            xdg_config_home: xdg_config_home.to_path_buf(),
            xdg_state_home: xdg_state_home.to_path_buf(),
            xdg_cache_home: xdg_cache_home.to_path_buf(),
            path: std::env::var_os("PATH"),
        }
    }
}

fn run_codex_exec(
    transport_mode: CodexTransportMode,
    codex_home: &Path,
    workdir: &Path,
    last_message_path: &Path,
    local_token_assignment: &str,
    child_environment: CodexChildEnvironment,
) -> Result<Output, String> {
    let local_token = parse_posix_token_assignment(local_token_assignment)?;
    let CodexChildEnvironment {
        home,
        xdg_config_home,
        xdg_state_home,
        xdg_cache_home,
        path,
    } = child_environment;
    let mut command = Command::new("codex");
    command
        .arg("--profile")
        .arg("codex-router")
        .arg("exec")
        .arg("--cd")
        .arg(workdir)
        .arg("--skip-git-repo-check")
        .arg("--sandbox")
        .arg("read-only")
        .arg("-c")
        .arg("approval_policy=\"never\"")
        .arg("--ephemeral")
        .arg("--output-last-message")
        .arg(last_message_path);
    if transport_mode == CodexTransportMode::HttpSse {
        command
            .arg("-c")
            .arg("model_providers.codex-router.supports_websockets=false");
    }
    command
        .arg("Reply with exactly: codex-router smoke ok")
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg_config_home)
        .env("XDG_STATE_HOME", xdg_state_home)
        .env("XDG_CACHE_HOME", xdg_cache_home)
        .env("CODEX_HOME", codex_home)
        .env("CODEX_ROUTER_TOKEN", local_token)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }

    run_with_timeout(command, CODEX_COMMAND_TIMEOUT)
}

fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<Output, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn installed codex: {error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                return child
                    .wait_with_output()
                    .map_err(|error| format!("failed to collect installed codex output: {error}"));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let output = child.wait_with_output().map_err(|error| {
                        format!("failed to collect timed-out installed codex output: {error}")
                    })?;
                    let stdout_byte_count = output.stdout.len();
                    let stderr_byte_count = output.stderr.len();
                    return Err(format!(
                        "installed codex timed out after {}s; captured stdout/stderr suppressed to avoid leaking secrets (stdout_bytes={stdout_byte_count}, stderr_bytes={stderr_byte_count})",
                        timeout.as_secs(),
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("failed to poll installed codex: {error}")),
        }
    }
}

fn assert_codex_visible_output(
    label: &str,
    output: &Output,
    last_message_path: &Path,
) -> Result<(), String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(SMOKE_EXPECTED_TEXT) {
        return Err(format!(
            "{label} smoke stdout did not contain expected response text"
        ));
    }
    let last_message = fs::read_to_string(last_message_path).map_err(|error| {
        format!(
            "{label} smoke failed to read last-message file {}: {error}",
            last_message_path.display()
        )
    })?;
    if !last_message.contains(SMOKE_EXPECTED_TEXT) {
        return Err(format!(
            "{label} smoke last-message file did not contain expected response text"
        ));
    }
    Ok(())
}

fn assert_smoke_contract(
    http_sse_codex_status: &ExitStatus,
    websocket_codex_status: &ExitStatus,
    upstream: &MockWebSocketTranscript,
    local_token: &str,
) -> Result<(), String> {
    if !http_sse_codex_status.success() {
        return Err(format!(
            "installed codex HTTP/SSE smoke exited with status {http_sse_codex_status}"
        ));
    }
    if !websocket_codex_status.success() {
        return Err(format!(
            "installed codex WebSocket smoke exited with status {websocket_codex_status}"
        ));
    }
    let authorization = upstream
        .header("authorization")
        .ok_or_else(|| "mock upstream did not receive Authorization header".to_owned())?;
    if authorization != format!("Bearer {}", upstream_account_token()) {
        return Err("mock upstream did not receive the selected upstream account token".to_owned());
    }
    if upstream.header("x-codex-router-token").is_some() {
        return Err("mock upstream websocket received local router token header".to_owned());
    }
    if upstream.first_frame.contains(local_token) {
        return Err("mock upstream websocket frame leaked local router token".to_owned());
    }
    let http_sse = upstream
        .http_sse
        .as_ref()
        .ok_or_else(|| "mock upstream did not capture HTTP/SSE /v1/responses traffic".to_owned())?;
    if !http_sse.request_line.starts_with("POST /v1/responses ") {
        return Err(format!(
            "HTTP/SSE request was not POST /v1/responses: {}",
            http_sse.request_line
        ));
    }
    if http_sse.header("authorization") != Some(format!("Bearer {}", upstream_account_token())) {
        return Err(
            "HTTP/SSE request did not receive the selected upstream account token".to_owned(),
        );
    }
    if http_sse.header("x-codex-router-token").is_some() {
        return Err("HTTP/SSE request leaked local router token header upstream".to_owned());
    }
    if http_sse.body.contains(local_token) {
        return Err("HTTP/SSE request body leaked local router token upstream".to_owned());
    }
    if !http_sse.body.contains("\"stream\":true") {
        return Err("HTTP/SSE request did not ask for a streaming response".to_owned());
    }
    let first_frame = upstream
        .first_frame_json()
        .ok_or_else(|| "mock upstream did not capture a JSON first frame".to_owned())?;
    if first_frame.get("type").and_then(Value::as_str) != Some("response.create") {
        return Err(format!(
            "first websocket frame was not response.create: {first_frame}"
        ));
    }

    Ok(())
}

struct RedactedTranscriptInput<'a> {
    codex_version: &'a str,
    profile_path: &'a Path,
    http_sse_codex_status: &'a ExitStatus,
    http_sse_codex_stdout: &'a str,
    http_sse_codex_stderr: &'a str,
    http_sse_last_message_path: &'a Path,
    websocket_codex_status: &'a ExitStatus,
    websocket_codex_stdout: &'a str,
    websocket_codex_stderr: &'a str,
    websocket_last_message_path: &'a Path,
    upstream: &'a MockWebSocketTranscript,
}

fn write_redacted_transcript(input: RedactedTranscriptInput<'_>) -> Result<PathBuf, String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let artifact_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "failed to resolve workspace root for smoke artifact".to_owned())?
        .join("tmp")
        .join("smoke");
    fs::create_dir_all(&artifact_dir).map_err(|error| {
        format!(
            "failed to create smoke artifact dir {}: {error}",
            artifact_dir.display()
        )
    })?;
    let transcript_path = artifact_dir.join(format!(
        "installed-codex-mock-{}-{}.json",
        std::process::id(),
        timestamp_millis()
    ));
    let first_frame = input.upstream.first_frame_json().unwrap_or(Value::Null);
    let redacted = serde_json::json!({
        "codex_version": input.codex_version,
        "profile_path": input.profile_path.display().to_string(),
        "http_sse_codex_status": input.http_sse_codex_status.to_string(),
        "http_sse_codex_stdout_contains_smoke_text": input.http_sse_codex_stdout.contains("codex-router smoke ok"),
        "http_sse_codex_stderr_line_count": input.http_sse_codex_stderr.lines().count(),
        "http_sse_last_message_path": input.http_sse_last_message_path.display().to_string(),
        "websocket_codex_status": input.websocket_codex_status.to_string(),
        "websocket_codex_stdout_contains_smoke_text": input.websocket_codex_stdout.contains("codex-router smoke ok"),
        "websocket_codex_stderr_line_count": input.websocket_codex_stderr.lines().count(),
        "websocket_last_message_path": input.websocket_last_message_path.display().to_string(),
        "router_completed": true,
        "upstream": {
            "handshake_count": 1,
            "http_probe_count": input.upstream.http_probe_count,
            "http_sse_request_line": input.upstream.http_sse.as_ref().map(|request| request.request_line.as_str()),
            "http_sse_authorization_header": input.upstream.http_sse.as_ref().and_then(|request| request.header("authorization")).map(|_| "<redacted-present>"),
            "http_sse_local_router_header_present": input.upstream.http_sse.as_ref().and_then(|request| request.header("x-codex-router-token")).is_some(),
            "http_sse_stream_requested": input.upstream.http_sse.as_ref().map(|request| request.body.contains("\"stream\":true")),
            "http_sse_local_router_token_in_body": false,
            "authorization_header": input.upstream.header("authorization").map(|_| "<redacted-present>"),
            "local_router_header_present": input.upstream.header("x-codex-router-token").is_some(),
            "websocket_local_router_token_in_first_frame": false,
            "first_frame_type": first_frame.get("type").and_then(Value::as_str),
            "first_frame_model": first_frame.get("model").and_then(Value::as_str),
            "first_frame_stream": first_frame.get("stream").and_then(Value::as_bool),
        }
    });
    let payload = serde_json::to_string_pretty(&redacted)
        .map_err(|error| format!("failed to render redacted smoke transcript: {error}"))?;
    fs::write(&transcript_path, payload)
        .map_err(|error| format!("failed to write smoke transcript: {error}"))?;

    Ok(transcript_path)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MockWebSocketTranscript {
    headers: Vec<(String, String)>,
    first_frame: String,
    http_probe_count: usize,
    http_sse: Option<MockHttpSseTranscript>,
}

impl MockWebSocketTranscript {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    fn first_frame_json(&self) -> Option<Value> {
        serde_json::from_str(&self.first_frame).ok()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MockHttpSseTranscript {
    request_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl MockHttpSseTranscript {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }
}

struct MockWebSocketUpstream {
    address: String,
    transcript: Arc<Mutex<Option<MockWebSocketTranscript>>>,
    handle: thread::JoinHandle<Result<(), String>>,
}

struct MockNoConnectionUpstream {
    address: String,
    handle: thread::JoinHandle<Result<usize, String>>,
}

impl MockNoConnectionUpstream {
    fn start(timeout: Duration) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("failed to bind no-connection upstream: {error}"))?;
        listener.set_nonblocking(true).map_err(|error| {
            format!("failed to configure no-connection upstream nonblocking: {error}")
        })?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("failed to read no-connection upstream address: {error}"))?
            .to_string();
        let handle = thread::Builder::new()
            .name("codex-router-hostile-no-token-upstream".to_owned())
            .spawn(move || run_no_connection_upstream(listener, timeout))
            .map_err(|error| format!("failed to spawn no-connection upstream thread: {error}"))?;

        Ok(Self { address, handle })
    }

    fn address(&self) -> &str {
        &self.address
    }

    fn join(self) -> Result<usize, String> {
        join_result(self.handle, "no-connection upstream")
    }
}

impl MockWebSocketUpstream {
    fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("failed to bind mock websocket upstream: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("failed to configure mock upstream nonblocking: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("failed to read mock upstream address: {error}"))?
            .to_string();
        let transcript = Arc::new(Mutex::new(None));
        let thread_transcript = Arc::clone(&transcript);
        let handle = thread::Builder::new()
            .name("codex-router-installed-smoke-upstream".to_owned())
            .spawn(move || run_mock_upstream(listener, thread_transcript))
            .map_err(|error| format!("failed to spawn mock upstream thread: {error}"))?;

        Ok(Self {
            address,
            transcript,
            handle,
        })
    }

    fn address(&self) -> &str {
        &self.address
    }

    fn join(self) -> Result<MockWebSocketTranscript, String> {
        join_result(self.handle, "mock websocket upstream")?;
        let mut transcript = self
            .transcript
            .lock()
            .map_err(|_| "mock upstream transcript mutex poisoned".to_owned())?;
        transcript
            .take()
            .ok_or_else(|| "mock upstream recorded no websocket transcript".to_owned())
    }
}

fn run_mock_upstream(
    listener: TcpListener,
    transcript: Arc<Mutex<Option<MockWebSocketTranscript>>>,
) -> Result<(), String> {
    let mut http_probe_count = 0_usize;
    let mut http_sse = None;
    loop {
        let deadline = Instant::now() + UPSTREAM_ACCEPT_TIMEOUT;
        let stream = accept_with_deadline(&listener, deadline)?;
        if !looks_like_websocket_upgrade(&stream)? {
            match respond_to_http_request(stream)? {
                MockHttpRequestResult::Probe => http_probe_count += 1,
                MockHttpRequestResult::Responses(transcript) => {
                    http_sse = Some(transcript);
                }
            }
            continue;
        }
        run_mock_websocket(stream, transcript, http_probe_count, http_sse.take())?;
        return Ok(());
    }
}

fn run_no_connection_upstream(listener: TcpListener, timeout: Duration) -> Result<usize, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let _ = stream.shutdown(Shutdown::Both);
                return Ok(1);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Ok(0);
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("no-connection upstream accept failed: {error}")),
        }
    }
}

fn accept_with_deadline(
    listener: &TcpListener,
    deadline: Instant,
) -> Result<std::net::TcpStream, String> {
    loop {
        match listener.accept() {
            Ok((stream, _peer)) => {
                stream.set_nonblocking(false).map_err(|error| {
                    format!("failed to restore accepted stream blocking mode: {error}")
                })?;
                return Ok(stream);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("mock upstream timed out waiting for websocket".to_owned());
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("mock upstream accept failed: {error}")),
        }
    }
}

fn looks_like_websocket_upgrade(stream: &std::net::TcpStream) -> Result<bool, String> {
    let mut buffer = [0_u8; 1024];
    let byte_count = stream
        .peek(&mut buffer)
        .map_err(|error| format!("mock upstream failed to peek request: {error}"))?;
    let request = String::from_utf8_lossy(&buffer[..byte_count]);
    Ok(request.to_ascii_lowercase().contains("upgrade: websocket"))
}

enum MockHttpRequestResult {
    Probe,
    Responses(MockHttpSseTranscript),
}

fn respond_to_http_request(
    mut stream: std::net::TcpStream,
) -> Result<MockHttpRequestResult, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("mock upstream failed to set HTTP probe timeout: {error}"))?;
    let request = read_http_request(&mut stream)?;
    if request.request_line.starts_with("POST /v1/responses ") {
        let body = smoke_sse_body();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .map_err(|error| format!("mock upstream failed to write HTTP/SSE response: {error}"))?;
        return Ok(MockHttpRequestResult::Responses(request));
    }
    let body = r#"{"object":"list","data":[]}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| format!("mock upstream failed to write HTTP probe response: {error}"))?;
    Ok(MockHttpRequestResult::Probe)
}

fn read_http_request(stream: &mut std::net::TcpStream) -> Result<MockHttpSseTranscript, String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let byte_count = stream
            .read(&mut buffer)
            .map_err(|error| format!("mock upstream failed to read HTTP request: {error}"))?;
        if byte_count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..byte_count]);
        if let Some(header_end) = find_header_end(&bytes) {
            let header_text = String::from_utf8_lossy(&bytes[..header_end]).to_string();
            let content_length = parse_content_length(&header_text);
            let body_start = header_end + 4;
            if bytes.len() >= body_start + content_length {
                let body = String::from_utf8_lossy(&bytes[body_start..body_start + content_length])
                    .to_string();
                let (request_line, headers) = parse_http_head(&header_text)?;
                return Ok(MockHttpSseTranscript {
                    request_line,
                    headers,
                    body,
                });
            }
        }
    }
    Err("mock upstream received incomplete HTTP request".to_owned())
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header_text: &str) -> usize {
    header_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

fn parse_http_head(header_text: &str) -> Result<(String, Vec<(String, String)>), String> {
    let mut lines = header_text.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| "HTTP request was missing request line".to_owned())?
        .to_owned();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    Ok((request_line, headers))
}

fn smoke_sse_body() -> String {
    smoke_http_sse_events()
        .into_iter()
        .map(|event| {
            let event_type = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("response.unknown");
            format!("event: {event_type}\ndata: {event}\n\n")
        })
        .collect::<String>()
}

fn smoke_http_sse_events() -> Vec<Value> {
    let response_id = "resp-smoke-http-sse";
    let message_id = "msg-smoke-http-sse";
    let text = "codex-router smoke ok";
    vec![
        serde_json::json!({
            "type": "response.created",
            "response": {"id": response_id, "status": "in_progress", "output": []}
        }),
        serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"id": message_id, "type": "message", "role": "assistant", "status": "in_progress", "content": []}
        }),
        serde_json::json!({
            "type": "response.content_part.added",
            "item_id": message_id,
            "output_index": 0,
            "content_index": 0,
            "part": {"type": "output_text", "text": ""}
        }),
        serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": message_id,
            "output_index": 0,
            "content_index": 0,
            "delta": text
        }),
        serde_json::json!({
            "type": "response.output_text.done",
            "item_id": message_id,
            "output_index": 0,
            "content_index": 0,
            "text": text
        }),
        serde_json::json!({
            "type": "response.content_part.done",
            "item_id": message_id,
            "output_index": 0,
            "content_index": 0,
            "part": {"type": "output_text", "text": text}
        }),
        serde_json::json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": text}]
            }
        }),
        serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": response_id,
                "status": "completed",
                "output": [{
                    "id": message_id,
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": text}]
                }],
                "usage": {
                    "input_tokens": 0,
                    "input_tokens_details": null,
                    "output_tokens": 0,
                    "output_tokens_details": null,
                    "total_tokens": 0
                }
            }
        }),
    ]
}

#[allow(clippy::result_large_err)]
fn run_mock_websocket(
    stream: std::net::TcpStream,
    transcript: Arc<Mutex<Option<MockWebSocketTranscript>>>,
    http_probe_count: usize,
    http_sse: Option<MockHttpSseTranscript>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| format!("mock upstream failed to set websocket read timeout: {error}"))?;
    let captured_headers = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let callback_headers = Arc::clone(&captured_headers);
    let mut websocket = accept_hdr(stream, move |request: &Request, response: Response| {
        let headers = request
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
            })
            .collect::<Vec<_>>();
        if let Ok(mut captured) = callback_headers.lock() {
            *captured = headers;
        }
        Ok(response)
    })
    .map_err(|error| format!("mock upstream websocket handshake failed: {error}"))?;
    let mut first_frame = None;
    for request_index in 0..4 {
        let frame = match websocket.read() {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Binary(bytes)) => String::from_utf8(bytes.to_vec())
                .map_err(|error| format!("mock upstream frame was not UTF-8: {error}"))?,
            Ok(Message::Close(_)) => break,
            Ok(_other) => continue,
            Err(tungstenite::Error::Io(error))
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_error) if first_frame.is_some() => break,
            Err(error) => return Err(format!("mock upstream failed to read frame: {error}")),
        };
        if first_frame.is_none() {
            first_frame = Some(frame.clone());
        }
        for event in smoke_response_events(request_index) {
            websocket
                .send(Message::Text(event.into()))
                .map_err(|error| format!("mock upstream failed to send response event: {error}"))?;
        }
    }
    let first_frame = first_frame
        .ok_or_else(|| "mock upstream did not receive any websocket request frame".to_owned())?;
    let headers = captured_headers
        .lock()
        .map_err(|_| "mock upstream header mutex poisoned".to_owned())?
        .clone();
    let mut transcript = transcript
        .lock()
        .map_err(|_| "mock upstream transcript mutex poisoned".to_owned())?;
    *transcript = Some(MockWebSocketTranscript {
        headers,
        first_frame,
        http_probe_count,
        http_sse,
    });

    Ok(())
}

fn smoke_response_events(request_index: usize) -> Vec<String> {
    let response_id = format!("resp-smoke-{request_index}");
    let message_id = format!("msg-smoke-{request_index}");
    vec![
        serde_json::json!({
            "type": "response.created",
            "response": {"id": response_id}
        })
        .to_string(),
        serde_json::json!({
            "type": "response.output_item.done",
            "item": {
                "type": "message",
                "role": "assistant",
                "id": message_id,
                "content": [{"type": "output_text", "text": "codex-router smoke ok"}]
            }
        })
        .to_string(),
        serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": response_id,
                "usage": {
                    "input_tokens": 0,
                    "input_tokens_details": null,
                    "output_tokens": 0,
                    "output_tokens_details": null,
                    "total_tokens": 0
                }
            }
        })
        .to_string(),
    ]
}

fn command_output_text(command: &mut Command) -> Result<String, String> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("failed to run command: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "command exited with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn join_result<T>(handle: thread::JoinHandle<Result<T, String>>, label: &str) -> Result<T, String> {
    match handle.join() {
        Ok(result) => result,
        Err(error) => Err(format!("{label} thread panicked: {error:?}")),
    }
}

fn parse_posix_token_assignment(assignment: &str) -> Result<String, String> {
    let prefix = "export CODEX_ROUTER_TOKEN='";
    let suffix = "'\n";
    if !assignment.starts_with(prefix) || !assignment.ends_with(suffix) {
        return Err("token export assignment did not use expected POSIX shape".to_owned());
    }
    let token = &assignment[prefix.len()..assignment.len() - suffix.len()];
    if token.contains("'\\''") {
        return Err("smoke token unexpectedly required shell unescaping".to_owned());
    }

    Ok(token.to_owned())
}

fn reserve_loopback_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("failed to reserve loopback port: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("failed to read reserved loopback port: {error}"))?
        .port();
    drop(listener);
    Ok(port)
}

fn drain_optional_router_connections(router_port: u16, attempts: usize) {
    for _ in 0..attempts {
        if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", router_port)) {
            let _ = stream.write_all(
                b"POST /v1/responses HTTP/1.1\r\nhost: 127.0.0.1\r\ncontent-length: 2\r\n\r\n{}",
            );
            let _ = stream.shutdown(Shutdown::Write);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
            let mut buffer = [0_u8; 256];
            let _ = stream.read(&mut buffer);
        }
    }
}

fn send_hostile_no_token_websocket(router_port: u16) -> Result<(), String> {
    let request = format!("ws://127.0.0.1:{router_port}/v1/responses")
        .into_client_request()
        .map_err(|error| format!("failed to build hostile local websocket request: {error}"))?;
    let (mut websocket, _response) = match connect(request) {
        Ok(connection) => connection,
        Err(_error) => return Ok(()),
    };
    websocket
        .send(Message::text(
            r#"{"type":"response.create","hostile_no_token":true}"#,
        ))
        .map_err(|error| format!("hostile local websocket send failed: {error}"))?;
    match websocket.read() {
        Ok(Message::Close(_)) => Ok(()),
        Err(_error) => Ok(()),
        Ok(message) => Err(format!(
            "hostile local websocket unexpectedly received non-close message: {message}"
        )),
    }
}

fn account_id(value: &str) -> Result<AccountId, String> {
    AccountId::new(value.to_owned()).map_err(|_| format!("invalid smoke account id: {value}"))
}

fn upstream_account_token() -> &'static str {
    "installed-smoke-upstream-token"
}

fn timestamp_millis() -> u128 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_millis(),
        Err(_) => 0,
    }
}

struct SmokeTempRoot {
    path: PathBuf,
}

impl SmokeTempRoot {
    fn new(name: &str) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "codex-router-{name}-{}-{}",
            std::process::id(),
            timestamp_millis()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| {
                format!(
                    "failed to remove stale temp root {}: {error}",
                    path.display()
                )
            })?;
        }
        fs::create_dir_all(&path)
            .map_err(|error| format!("failed to create temp root {}: {error}", path.display()))?;

        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SmokeTempRoot {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::process::Output;

    use super::MockHttpSseTranscript;
    use super::MockWebSocketTranscript;
    use super::SMOKE_EXPECTED_TEXT;
    use super::SmokeTempRoot;
    use super::assert_codex_visible_output;
    use super::assert_smoke_contract;
    use super::run_hostile_no_token_smoke;
    use super::run_installed_codex_mock_smoke;
    use super::run_with_timeout;
    use super::upstream_account_token;

    fn success_status() -> ExitStatus {
        ExitStatus::from_raw(0)
    }

    fn valid_transcript(
        local_token_in_http_body: bool,
        local_token_in_first_frame: bool,
    ) -> MockWebSocketTranscript {
        let local_token = "local-token-canary";
        MockWebSocketTranscript {
            headers: vec![(
                "authorization".to_owned(),
                format!("Bearer {}", upstream_account_token()),
            )],
            first_frame: if local_token_in_first_frame {
                format!(r#"{{"type":"response.create","token":"{local_token}"}}"#)
            } else {
                r#"{"type":"response.create"}"#.to_owned()
            },
            http_probe_count: 0,
            http_sse: Some(MockHttpSseTranscript {
                request_line: "POST /v1/responses HTTP/1.1".to_owned(),
                headers: vec![(
                    "authorization".to_owned(),
                    format!("Bearer {}", upstream_account_token()),
                )],
                body: if local_token_in_http_body {
                    format!(r#"{{"stream":true,"token":"{local_token}"}}"#)
                } else {
                    r#"{"stream":true}"#.to_owned()
                },
            }),
        }
    }

    #[test]
    fn smoke_contract_rejects_local_token_in_upstream_http_body() {
        let error = match assert_smoke_contract(
            &success_status(),
            &success_status(),
            &valid_transcript(true, false),
            "local-token-canary",
        ) {
            Ok(()) => panic!("HTTP/SSE body local-token leak must fail smoke contract"),
            Err(error) => error,
        };

        assert!(error.contains("HTTP/SSE request body leaked local router token"));
    }

    #[test]
    fn smoke_contract_rejects_local_token_in_upstream_websocket_frame() {
        let error = match assert_smoke_contract(
            &success_status(),
            &success_status(),
            &valid_transcript(false, true),
            "local-token-canary",
        ) {
            Ok(()) => panic!("WebSocket frame local-token leak must fail smoke contract"),
            Err(error) => error,
        };

        assert!(error.contains("websocket frame leaked local router token"));
    }

    #[test]
    fn smoke_visible_output_requires_last_message_text() {
        let test_root = match SmokeTempRoot::new("visible-output") {
            Ok(test_root) => test_root,
            Err(error) => panic!("failed to create temp root: {error}"),
        };
        let last_message_path = test_root.path().join("last-message.txt");
        if let Err(error) = fs::write(&last_message_path, "wrong text") {
            panic!("failed to write last-message fixture: {error}");
        }
        let output = Output {
            status: success_status(),
            stdout: SMOKE_EXPECTED_TEXT.as_bytes().to_vec(),
            stderr: Vec::new(),
        };

        let error = match assert_codex_visible_output("HTTP/SSE", &output, &last_message_path) {
            Ok(()) => panic!("wrong last-message text must fail visible output check"),
            Err(error) => error,
        };

        assert!(error.contains("last-message file did not contain expected response text"));
    }

    #[test]
    fn timed_out_codex_output_suppresses_captured_stdout_stderr() {
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg("printf local-secret-canary; sleep 2")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let error = match run_with_timeout(command, std::time::Duration::from_millis(10)) {
            Ok(_) => panic!("sleeping command must time out"),
            Err(error) => error,
        };

        assert!(!error.contains("local-secret-canary"));
        assert!(error.contains("captured stdout/stderr suppressed"));
    }

    #[test]
    #[ignore = "run through tests/smoke/installed_codex_mock.sh"]
    fn installed_codex_mock_smoke_exercises_generated_profile_token_and_websocket() {
        let report = match run_installed_codex_mock_smoke() {
            Ok(report) => report,
            Err(error) => panic!("installed Codex smoke failed: {error}"),
        };

        assert!(report.transcript_path().exists());
    }

    #[test]
    #[ignore = "run through tests/smoke/installed_codex_mock.sh"]
    fn installed_codex_hostile_no_token_smoke_keeps_upstream_empty() {
        if let Err(error) = run_hostile_no_token_smoke() {
            panic!("hostile no-token smoke failed: {error}");
        }
    }
}
