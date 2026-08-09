//! Installed Codex smoke harness.

mod retry;

pub use retry::run_all_weekly_exhausted_terminal;
pub use retry::run_capacity_retry_limit_terminal;
pub use retry::run_model_capacity_reconnect;
pub use retry::run_three_account_short_quota_reconnect;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::BufRead;
use std::io::BufReader;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::net::TcpListener;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Output;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use codex_router_cli::CliContext;
use codex_router_cli::profile::CodexRouterProfileWriter;
use codex_router_cli::run_with_io;
use codex_router_cli::token::LocalRouterTokenService;
use codex_router_cli::token::Shell;
use codex_router_cli::token::export_token_assignment;
use codex_router_codex::CodexRouterProfile;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::account_tokens::upstream_access_token_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::sqlite::SqliteStateStore;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tungstenite::Message;
use tungstenite::WebSocket;
use tungstenite::accept_hdr;
use tungstenite::client::IntoClientRequest;
use tungstenite::connect;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;
use tungstenite::stream::MaybeTlsStream;

const SMOKE_EXPECTED_TEXT: &str = "codex-router smoke ok";
const SMOKE_PROMPT: &str = "Reply with exactly: codex-router smoke ok";
const SMOKE_TARGET_MODEL: &str = "gpt-5.4-mini";
const SMOKE_TARGET_MODEL_OVERRIDE: &str = "model=\"gpt-5.4-mini\"";
const CODEX_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_ACCEPT_TIMEOUT: Duration = Duration::from_secs(35);
const DEFAULT_SOAK_DURATION: Duration = Duration::from_secs(300);
const SOAK_COMMAND_TIMEOUT_SLACK: Duration = Duration::from_secs(90);
const SOAK_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const SOAK_PROOF_MARGIN: Duration = Duration::from_secs(1);
const QUICK_CONCURRENT_HOLD_DURATION: Duration = Duration::from_secs(2);
const ROUTER_REGISTRY_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);
const RETAIN_SMOKE_ROOT_ENV: &str = "CODEX_ROUTER_RETAIN_SMOKE_ROOT";
type PressureHandles = Arc<Mutex<Vec<thread::JoinHandle<Result<(), String>>>>>;
const INSTALLED_SMOKE_RUNTIME_ROOT_MODE_ENV: &str =
    "CODEX_ROUTER_INSTALLED_SMOKE_RUNTIME_ROOT_MODE";
const INSTALLED_SMOKE_ROUTER_ROOT_ENV: &str = "CODEX_ROUTER_INSTALLED_SMOKE_ROUTER_ROOT";
const INSTALLED_SMOKE_CODEX_HOME_ENV: &str = "CODEX_ROUTER_INSTALLED_SMOKE_CODEX_HOME";
const INSTALLED_SMOKE_PROCESS_HOME_ENV: &str = "CODEX_ROUTER_INSTALLED_SMOKE_PROCESS_HOME";
const S8_RUN_ID_ENV: &str = "CODEX_ROUTER_S8_RUN_ID";
const QUOTA_RECONNECT_SQLITE_PRESSURE_HOLD: Duration = Duration::from_secs(15);
const QUOTA_RECONNECT_SQLITE_PRESSURE_READY_TIMEOUT: Duration = Duration::from_secs(5);
const QUOTA_RECONNECT_ROUTER_MAX_CONNECTIONS: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstalledCodexSmokeMode {
    HttpSse,
    WebSocket,
    Combined,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConcurrentWebSocketHarnessConfig {
    artifact_mode: &'static str,
    upstream: ConcurrentUpstreamConfig,
    codex_command_timeout: Duration,
    router_max_connections: usize,
    capture_registry_report: bool,
    quota_reconnect: bool,
}

impl ConcurrentWebSocketHarnessConfig {
    const fn quick() -> Self {
        Self {
            artifact_mode: "three-websocket",
            upstream: ConcurrentUpstreamConfig::quick(3),
            codex_command_timeout: Duration::from_secs(60),
            router_max_connections: 3,
            capture_registry_report: true,
            quota_reconnect: false,
        }
    }

    fn soak() -> Self {
        let hold_duration = soak_duration_from_env();
        Self {
            artifact_mode: "three-websocket-soak",
            upstream: ConcurrentUpstreamConfig::soak(3, hold_duration),
            codex_command_timeout: hold_duration.saturating_add(SOAK_COMMAND_TIMEOUT_SLACK),
            router_max_connections: 3,
            capture_registry_report: true,
            quota_reconnect: false,
        }
    }

    fn s8_overlap_quota() -> Self {
        let hold_duration = soak_duration_from_env();
        Self {
            artifact_mode: "s8-overlap-quota",
            upstream: ConcurrentUpstreamConfig::s8_overlap_quota(3, hold_duration),
            codex_command_timeout: hold_duration.saturating_add(SOAK_COMMAND_TIMEOUT_SLACK),
            router_max_connections: 5,
            capture_registry_report: true,
            quota_reconnect: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConcurrentUpstreamConfig {
    expected_sessions: usize,
    expected_upstream_sessions: usize,
    hold_duration: Duration,
    heartbeat_interval: Duration,
}

impl ConcurrentUpstreamConfig {
    const fn quick(expected_sessions: usize) -> Self {
        Self {
            expected_sessions,
            expected_upstream_sessions: expected_sessions,
            hold_duration: QUICK_CONCURRENT_HOLD_DURATION,
            heartbeat_interval: Duration::from_millis(250),
        }
    }

    fn soak(expected_sessions: usize, hold_duration: Duration) -> Self {
        let heartbeat_interval = hold_duration
            .checked_div(4)
            .filter(|duration| !duration.is_zero())
            .map_or(SOAK_HEARTBEAT_INTERVAL, |duration| {
                duration.min(SOAK_HEARTBEAT_INTERVAL)
            });
        Self {
            expected_sessions,
            expected_upstream_sessions: expected_sessions,
            hold_duration,
            heartbeat_interval,
        }
    }

    fn s8_overlap_quota(expected_sessions: usize, hold_duration: Duration) -> Self {
        let heartbeat_interval = hold_duration
            .checked_div(4)
            .filter(|duration| !duration.is_zero())
            .map_or(SOAK_HEARTBEAT_INTERVAL, |duration| {
                duration.min(SOAK_HEARTBEAT_INTERVAL)
            });
        Self {
            expected_sessions,
            expected_upstream_sessions: expected_sessions.saturating_add(2),
            hold_duration,
            heartbeat_interval,
        }
    }
}

fn soak_duration_from_env() -> Duration {
    std::env::var("CODEX_ROUTER_SOAK_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_SOAK_DURATION)
}

impl InstalledCodexSmokeMode {
    const fn requires_http_sse(self) -> bool {
        matches!(self, Self::HttpSse | Self::Combined)
    }

    const fn requires_websocket(self) -> bool {
        matches!(self, Self::WebSocket | Self::Combined)
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::HttpSse => "http-sse",
            Self::WebSocket => "websocket",
            Self::Combined => "combined",
        }
    }
}

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

#[derive(Debug)]
struct CodexChildRun {
    pid: u32,
    output: Output,
}

struct CodexExecRequest<'a> {
    transport_mode: CodexTransportMode,
    codex_home: &'a Path,
    workdir: &'a Path,
    last_message_path: &'a Path,
    child_environment: CodexChildEnvironment,
    timeout: Duration,
    prompt: &'a str,
    client_index: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InstalledCodexRuntimeRoots {
    mode: String,
    router_root: PathBuf,
    state_path: PathBuf,
    secret_root: PathBuf,
    codex_home: Option<PathBuf>,
    process_home: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct UpstreamClientSessionObservation {
    client_index: usize,
    upstream_session_id: u64,
}

/// Runs the installed Codex mock smoke.
pub fn run_installed_codex_mock_smoke() -> Result<InstalledCodexSmokeReport, String> {
    run_installed_codex_mock_smoke_with_mode(InstalledCodexSmokeMode::Combined)
}

/// Runs the installed Codex HTTP/SSE mock smoke.
pub fn run_installed_codex_http_sse_mock_smoke() -> Result<InstalledCodexSmokeReport, String> {
    run_installed_codex_mock_smoke_with_mode(InstalledCodexSmokeMode::HttpSse)
}

/// Runs the installed Codex WebSocket mock smoke.
pub fn run_installed_codex_websocket_mock_smoke() -> Result<InstalledCodexSmokeReport, String> {
    run_installed_codex_mock_smoke_with_mode(InstalledCodexSmokeMode::WebSocket)
}

/// Runs three installed Codex WebSocket clients through one router child process.
pub fn run_installed_codex_three_websocket_mock_e2e() -> Result<InstalledCodexSmokeReport, String> {
    run_installed_codex_three_websocket_mock_e2e_inner(ConcurrentWebSocketHarnessConfig::quick())
}

/// Runs three installed Codex WebSocket clients through one router for a sustained soak.
pub fn run_installed_codex_three_websocket_mock_soak() -> Result<InstalledCodexSmokeReport, String>
{
    run_installed_codex_three_websocket_mock_e2e_inner(ConcurrentWebSocketHarnessConfig::soak())
}

/// Runs three installed Codex WebSocket clients while one reconnects after quota exhaustion.
pub fn run_installed_codex_s8_overlap_quota_websocket_mock_smoke()
-> Result<InstalledCodexSmokeReport, String> {
    run_installed_codex_three_websocket_mock_e2e_inner(
        ConcurrentWebSocketHarnessConfig::s8_overlap_quota(),
    )
}

/// Runs one installed Codex WebSocket client through provider quota exhaustion,
/// then proves Codex reconnects and completes on the next router account.
pub fn run_installed_codex_quota_reconnect_websocket_mock_smoke()
-> Result<InstalledCodexSmokeReport, String> {
    let smoke_root = SmokeTempRoot::new("installed-codex-quota-reconnect")?;
    let runtime_roots = installed_codex_runtime_roots(&smoke_root)?;
    let codex_home = runtime_roots
        .codex_home
        .clone()
        .unwrap_or_else(|| smoke_root.path().join("codex-home"));
    let workdir = smoke_root.path().join("workdir");
    let process_home = runtime_roots
        .process_home
        .clone()
        .unwrap_or_else(|| smoke_root.path().join("home"));
    let xdg_config_home = smoke_root.path().join("xdg-config");
    let xdg_state_home = smoke_root.path().join("xdg-state");
    let xdg_cache_home = smoke_root.path().join("xdg-cache");
    for path in [
        &codex_home,
        &workdir,
        &process_home,
        &xdg_config_home,
        &xdg_state_home,
        &xdg_cache_home,
        &runtime_roots.router_root,
    ] {
        fs::create_dir_all(path)
            .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    }

    let codex_version = command_output_text(Command::new("codex").arg("--version"))?;
    seed_quota_reconnect_router_state(&runtime_roots.state_path, &runtime_roots.secret_root)?;
    let sqlite_pressure = (runtime_roots.mode == "copied-dev-state")
        .then(|| QuotaReconnectSqlitePressureConfig::new(runtime_roots.state_path.clone()));
    let upstream = MockQuotaReconnectWebSocketUpstream::start(sqlite_pressure)?;
    let router_port = reserve_loopback_port()?;
    let audit_path = smoke_root
        .path()
        .join("quota-reconnect-audit")
        .join("events.jsonl");
    let registry_report_path = runtime_roots
        .router_root
        .join("quota-reconnect-websocket-registry-report.json");
    if let Some(audit_dir) = audit_path.parent() {
        fs::create_dir_all(audit_dir).map_err(|error| {
            format!(
                "failed to create quota reconnect audit dir {}: {error}",
                audit_dir.display()
            )
        })?;
    }
    let profile_writer = CodexRouterProfileWriter::new(&codex_home);
    let profile = CodexRouterProfile::new(router_port);
    let profile_path = profile_writer
        .write(&profile, true)
        .map_err(|error| format!("failed to write quota reconnect Codex profile: {error}"))?;
    let router_process = start_router_process_with_options(RouterProcessStartOptions {
        now_unix_seconds: Some(1_030),
        router_port,
        state_path: runtime_roots.state_path.clone(),
        secret_root: runtime_roots.secret_root.clone(),
        local_token: None,
        upstream_base_url: format!("http://{}/v1", upstream.address()),
        audit_path: audit_path.clone(),
        max_connections: QUOTA_RECONNECT_ROUTER_MAX_CONNECTIONS,
        websocket_registry_report_file: Some(registry_report_path.clone()),
    })?;

    let last_message_path = smoke_root.path().join("websocket-last-message.txt");
    let codex_output = run_codex_exec_with_timeout(
        CodexTransportMode::WebSocket,
        &codex_home,
        &workdir,
        &last_message_path,
        CodexChildEnvironment::new(
            &process_home,
            &xdg_config_home,
            &xdg_state_home,
            &xdg_cache_home,
        ),
        CODEX_COMMAND_TIMEOUT,
    )?;
    assert_codex_visible_output(
        "WebSocket quota reconnect",
        &codex_output,
        &last_message_path,
    )?;
    assert_codex_quota_reconnect_output_is_safe(&codex_output, &last_message_path)?;
    let router_process =
        router_process.wait("quota reconnect router process", Duration::from_secs(10))?;
    let registry_report = RouterWebSocketRegistryReport::from_file(&registry_report_path)?;
    let router_audit = RouterAuditObservation::from_file(&audit_path)?;
    router_audit.require_mode(InstalledCodexSmokeMode::WebSocket)?;
    let upstream_result = upstream.join()?;
    assert_quota_reconnect_contract(&upstream_result)?;
    let transcript_path =
        write_redacted_quota_reconnect_transcript(&QuotaReconnectTranscriptInput {
            codex_version: codex_version.trim(),
            profile_path: &profile_path,
            codex_status: &codex_output.status,
            codex_stdout: &String::from_utf8_lossy(&codex_output.stdout),
            codex_stderr: &String::from_utf8_lossy(&codex_output.stderr),
            last_message_path: &last_message_path,
            upstream: &upstream_result,
            router_process: &router_process,
            router_audit: &router_audit,
            registry_report: &registry_report,
            runtime_roots: &runtime_roots,
        })?;

    Ok(InstalledCodexSmokeReport { transcript_path })
}

fn run_installed_codex_mock_smoke_with_mode(
    mode: InstalledCodexSmokeMode,
) -> Result<InstalledCodexSmokeReport, String> {
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
    let upstream = MockWebSocketUpstream::start(mode)?;
    let seed = seed_router_state(&state_path, &secret_root)?;
    let router_port = reserve_loopback_port()?;
    let audit_path = router_root.join("audit").join("events.jsonl");
    let profile_writer = CodexRouterProfileWriter::new(&codex_home);
    let profile = CodexRouterProfile::new(router_port);
    let profile_path = profile_writer
        .write(&profile, true)
        .map_err(|error| format!("failed to write generated Codex profile: {error}"))?;
    let router_process = start_router_process(
        router_port,
        state_path,
        secret_root,
        None,
        format!("http://{}/v1", upstream.address()),
        audit_path.clone(),
    )?;

    let http_sse_last_message_path = smoke_root.path().join("http-sse-last-message.txt");
    let http_sse_codex_output = if mode.requires_http_sse() {
        let output = run_codex_exec(
            CodexTransportMode::HttpSse,
            &codex_home,
            &workdir,
            &http_sse_last_message_path,
            &seed.local_token_assignment,
            CodexChildEnvironment::new(
                &process_home,
                &xdg_config_home,
                &xdg_state_home,
                &xdg_cache_home,
            ),
        )?;
        if let Err(error) =
            assert_codex_visible_output("HTTP/SSE", &output, &http_sse_last_message_path)
        {
            let router_stop = router_process
                .wait("router process after HTTP/SSE failure", Duration::ZERO)
                .map(|observation| observation.cleanup_result)
                .unwrap_or_else(|stop_error| {
                    stop_error.lines().take(12).collect::<Vec<_>>().join(" | ")
                });
            let upstream_summary = upstream
                .join()
                .map(|transcript| http_sse_transcript_summary(&transcript))
                .unwrap_or_else(|join_error| format!("upstream-join-error:{join_error}"));
            return Err(format!(
                "{error}; router_stop={router_stop}; upstream_summary={upstream_summary}"
            ));
        }
        Some(output)
    } else {
        None
    };
    let websocket_last_message_path = smoke_root.path().join("websocket-last-message.txt");
    let websocket_codex_output = if mode.requires_websocket() {
        let output = run_codex_exec(
            CodexTransportMode::WebSocket,
            &codex_home,
            &workdir,
            &websocket_last_message_path,
            &seed.local_token_assignment,
            CodexChildEnvironment::new(
                &process_home,
                &xdg_config_home,
                &xdg_state_home,
                &xdg_cache_home,
            ),
        )?;
        assert_codex_visible_output("WebSocket", &output, &websocket_last_message_path)?;
        Some(output)
    } else {
        None
    };
    let router_process = router_process.stop("router process")?;
    let router_audit = RouterAuditObservation::from_file(&audit_path)?;
    router_audit.require_mode(mode)?;
    let upstream_result = upstream.join().map_err(|error| {
        format!(
            "{error}; websocket_codex_status={}; websocket_stdout={}; websocket_stderr={}",
            output_status_text(websocket_codex_output.as_ref()),
            redacted_optional_command_text(
                websocket_codex_output.as_ref().map(|output| &output.stdout),
                &seed
            ),
            redacted_optional_command_text(
                websocket_codex_output.as_ref().map(|output| &output.stderr),
                &seed
            )
        )
    })?;
    assert_smoke_contract(SmokeContractAssertion {
        mode,
        http_sse_codex_status: http_sse_codex_output.as_ref().map(|output| &output.status),
        websocket_codex_status: websocket_codex_output.as_ref().map(|output| &output.status),
        upstream: &upstream_result,
        local_token: &seed.local_token,
        expected_account_label: &seed.expected_account_label,
        expected_upstream_token: &seed.expected_upstream_token,
        routable_upstream_tokens: &seed.routable_upstream_tokens,
        quota_status: &seed.quota_status,
    })?;
    let transcript_path = write_redacted_transcript(RedactedTranscriptInput {
        mode,
        codex_version: codex_version.trim(),
        profile_path: &profile_path,
        expected_upstream_token: &seed.expected_upstream_token,
        http_sse_codex_status: http_sse_codex_output.as_ref().map(|output| &output.status),
        http_sse_codex_stdout: http_sse_codex_output
            .as_ref()
            .map(|output| String::from_utf8_lossy(&output.stdout)),
        http_sse_codex_stderr: http_sse_codex_output
            .as_ref()
            .map(|output| String::from_utf8_lossy(&output.stderr)),
        http_sse_last_message_path: mode
            .requires_http_sse()
            .then_some(http_sse_last_message_path.as_path()),
        websocket_codex_status: websocket_codex_output.as_ref().map(|output| &output.status),
        websocket_codex_stdout: websocket_codex_output
            .as_ref()
            .map(|output| String::from_utf8_lossy(&output.stdout)),
        websocket_codex_stderr: websocket_codex_output
            .as_ref()
            .map(|output| String::from_utf8_lossy(&output.stderr)),
        websocket_last_message_path: mode
            .requires_websocket()
            .then_some(websocket_last_message_path.as_path()),
        upstream: &upstream_result,
        quota_status: &seed.quota_status,
        expected_account_label: &seed.expected_account_label,
        router_process: &router_process,
        router_audit: &router_audit,
    })?;

    Ok(InstalledCodexSmokeReport { transcript_path })
}

fn run_installed_codex_three_websocket_mock_e2e_inner(
    config: ConcurrentWebSocketHarnessConfig,
) -> Result<InstalledCodexSmokeReport, String> {
    let smoke_root = SmokeTempRoot::new("installed-codex-three-websocket")?;
    let runtime_roots = installed_codex_runtime_roots_for_three_websocket(&smoke_root)?;
    fs::create_dir_all(&runtime_roots.router_root).map_err(|error| {
        format!(
            "failed to create temp router root {}: {error}",
            runtime_roots.router_root.display()
        )
    })?;

    let codex_version = command_output_text(Command::new("codex").arg("--version"))?;
    let sqlite_pressure = (config.quota_reconnect && runtime_roots.mode == "copied-dev-state")
        .then(|| QuotaReconnectSqlitePressureConfig::new(runtime_roots.state_path.clone()));
    let upstream = MockConcurrentWebSocketUpstream::start(config.upstream, sqlite_pressure)?;
    let seed = if config.quota_reconnect {
        seed_s8_overlap_quota_router_state(&runtime_roots.state_path, &runtime_roots.secret_root)?
    } else {
        seed_router_state(&runtime_roots.state_path, &runtime_roots.secret_root)?
    };
    let router_port = reserve_loopback_port()?;
    let audit_path = smoke_root
        .path()
        .join("three-websocket-audit")
        .join("events.jsonl");
    if let Some(audit_dir) = audit_path.parent() {
        fs::create_dir_all(audit_dir).map_err(|error| {
            format!(
                "failed to create three-websocket audit dir {}: {error}",
                audit_dir.display()
            )
        })?;
    }
    let registry_report_path = config.capture_registry_report.then(|| {
        runtime_roots
            .router_root
            .join("websocket-registry-report.json")
    });
    let router_process = start_router_process_with_options(RouterProcessStartOptions {
        now_unix_seconds: Some(1_030),
        router_port,
        state_path: runtime_roots.state_path.clone(),
        secret_root: runtime_roots.secret_root.clone(),
        local_token: None,
        upstream_base_url: format!("http://{}/v1", upstream.address()),
        audit_path,
        max_connections: config.router_max_connections,
        websocket_registry_report_file: registry_report_path.clone(),
    })?;

    let start_barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for client_index in 0..3 {
        let client_root = smoke_root.path().join(format!("client-{client_index}"));
        let codex_home = runtime_roots
            .codex_home
            .clone()
            .unwrap_or_else(|| client_root.join("codex-home"));
        let workdir = client_root.join("workdir");
        let process_home = runtime_roots
            .process_home
            .clone()
            .unwrap_or_else(|| client_root.join("home"));
        let xdg_config_home = client_root.join("xdg-config");
        let xdg_state_home = client_root.join("xdg-state");
        let xdg_cache_home = client_root.join("xdg-cache");
        for path in [
            &codex_home,
            &workdir,
            &process_home,
            &xdg_config_home,
            &xdg_state_home,
            &xdg_cache_home,
        ] {
            fs::create_dir_all(path)
                .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
        }
        let profile_writer = CodexRouterProfileWriter::new(&codex_home);
        let profile = CodexRouterProfile::new(router_port);
        profile_writer.write(&profile, true).map_err(|error| {
            format!("failed to write generated Codex profile for client {client_index}: {error}")
        })?;
        let last_message_path = client_root.join("websocket-last-message.txt");
        let child_environment = CodexChildEnvironment::new(
            &process_home,
            &xdg_config_home,
            &xdg_state_home,
            &xdg_cache_home,
        );
        let barrier = Arc::clone(&start_barrier);
        handles.push(
            thread::Builder::new()
                .name(format!("codex-router-three-client-{client_index}"))
                .spawn(move || {
                    barrier.wait();
                    let prompt = format!(
                        "{SMOKE_PROMPT}\n\nHarness marker: codex-router-client-{client_index}"
                    );
                    let output = run_codex_exec_with_timeout_observed(CodexExecRequest {
                        transport_mode: CodexTransportMode::WebSocket,
                        codex_home: &codex_home,
                        workdir: &workdir,
                        last_message_path: &last_message_path,
                        child_environment,
                        timeout: config.codex_command_timeout,
                        prompt: &prompt,
                        client_index: Some(client_index),
                    })?;
                    assert_codex_visible_output(
                        &format!("WebSocket client {client_index}"),
                        &output.output,
                        &last_message_path,
                    )?;
                    Ok::<CodexChildRun, String>(output)
                })
                .map_err(|error| {
                    format!("failed to spawn installed Codex client {client_index}: {error}")
                })?,
        );
    }

    let quota_probe_handle = if config.quota_reconnect {
        let overlap_state = Arc::clone(&upstream.state);
        let local_token = seed.local_token.clone();
        Some(
            thread::Builder::new()
                .name("codex-router-s8-overlap-quota-probe".to_owned())
                .spawn(move || {
                    wait_for_concurrent_session_barrier(&overlap_state)?;
                    run_s8_overlap_quota_local_probe(router_port, &local_token)
                })
                .map_err(|error| {
                    format!("failed to spawn S8 overlap quota local probe: {error}")
                })?,
        )
    } else {
        None
    };

    let mut outputs = Vec::new();
    let mut output_errors = Vec::new();
    for (client_index, handle) in handles.into_iter().enumerate() {
        match join_result(handle, &format!("installed Codex client {client_index}")) {
            Ok(output) => outputs.push(output),
            Err(error) => output_errors.push(format!("client{client_index}:{error}")),
        }
    }
    if let Some(handle) = quota_probe_handle
        && let Err(error) = join_result(handle, "S8 overlap quota local probe")
    {
        output_errors.push(format!("quota_probe:{error}"));
    }
    if !output_errors.is_empty() {
        return Err(format!(
            "installed Codex client failures: {}",
            output_errors.join(" | ")
        ));
    }
    let upstream_result = upstream.join()?;
    let socket_cleanup = observe_router_socket_cleanup(router_process.observation.pid)?;
    let router_process =
        router_process.wait("three-client router process", ROUTER_REGISTRY_DRAIN_TIMEOUT)?;
    let registry_report = registry_report_path
        .as_deref()
        .map(RouterWebSocketRegistryReport::from_file)
        .transpose()?;
    assert_concurrent_websocket_contract(config, &upstream_result, registry_report.as_ref())?;
    socket_cleanup.assert_no_leaked_sessions()?;
    let transcript_path =
        write_redacted_three_websocket_transcript(&ThreeWebSocketTranscriptInput {
            mode: config.artifact_mode,
            codex_version: &codex_version,
            router_process: &router_process,
            registry_report: registry_report.as_ref(),
            upstream: &upstream_result,
            socket_cleanup: &socket_cleanup,
            outputs: &outputs,
            seed: &seed,
            runtime_roots: &runtime_roots,
        })?;

    Ok(InstalledCodexSmokeReport { transcript_path })
}

fn installed_codex_runtime_roots_for_three_websocket(
    smoke_root: &SmokeTempRoot,
) -> Result<InstalledCodexRuntimeRoots, String> {
    installed_codex_runtime_roots(smoke_root)
}

fn installed_codex_runtime_roots(
    smoke_root: &SmokeTempRoot,
) -> Result<InstalledCodexRuntimeRoots, String> {
    let mode = std::env::var(INSTALLED_SMOKE_RUNTIME_ROOT_MODE_ENV)
        .unwrap_or_else(|_| "isolated-temp".to_owned());
    match mode.as_str() {
        "isolated-temp" => {
            let router_root = smoke_root.path().join("router");
            Ok(InstalledCodexRuntimeRoots {
                mode,
                state_path: router_root.join("state.sqlite"),
                secret_root: router_root.join("secrets"),
                router_root,
                codex_home: None,
                process_home: None,
            })
        }
        "copied-dev-state" => {
            let router_root = required_env_path(INSTALLED_SMOKE_ROUTER_ROOT_ENV)?;
            let codex_home = required_env_path(INSTALLED_SMOKE_CODEX_HOME_ENV)?;
            let process_home = required_env_path(INSTALLED_SMOKE_PROCESS_HOME_ENV)?;
            validate_copied_dev_state_roots(&router_root, &codex_home, &process_home)?;
            Ok(InstalledCodexRuntimeRoots {
                mode,
                state_path: router_root.join("state.sqlite"),
                secret_root: router_root.join("secrets"),
                router_root,
                codex_home: Some(codex_home),
                process_home: Some(process_home),
            })
        }
        other => Err(format!(
            "{INSTALLED_SMOKE_RUNTIME_ROOT_MODE_ENV} must be isolated-temp or copied-dev-state, got {other}"
        )),
    }
}

fn required_env_path(name: &str) -> Result<PathBuf, String> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| format!("{name} is required for copied-dev-state installed Codex smoke"))
}

fn validate_copied_dev_state_roots(
    router_root: &Path,
    codex_home: &Path,
    process_home: &Path,
) -> Result<(), String> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "test-support crate should live under workspace crates".to_owned())?;
    let dev_state_root = resolve_copied_dev_state_policy_path(
        "repo tmp/dev-state",
        &workspace_root.join("tmp/dev-state"),
    )?;
    for (label, path) in [
        ("router root", router_root),
        ("Codex home", codex_home),
        ("process HOME", process_home),
        ("router state.sqlite", &router_root.join("state.sqlite")),
        ("Codex state_5.sqlite", &codex_home.join("state_5.sqlite")),
        ("router secrets", &router_root.join("secrets")),
    ] {
        let resolved = resolve_copied_dev_state_policy_path(label, path)?;
        if !resolved.starts_with(&dev_state_root) {
            return Err(format!(
                "copied-dev-state {label} must be under repo-local tmp/dev-state; got {}",
                resolved.display()
            ));
        }
    }
    Ok(())
}

fn resolve_copied_dev_state_policy_path(label: &str, path: &Path) -> Result<PathBuf, String> {
    if path_is_symlink(path)? {
        return Err(format!(
            "copied-dev-state {label} must not be a symlink; got {}",
            path.display()
        ));
    }
    if path.exists() {
        return fs::canonicalize(path).map_err(|error| {
            format!(
                "failed to resolve copied-dev-state {label} {}: {error}",
                path.display()
            )
        });
    }
    resolve_future_path_without_following_new_leaf(path)
}

fn path_is_symlink(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "failed to inspect copied-dev-state path {}: {error}",
            path.display()
        )),
    }
}

fn resolve_future_path_without_following_new_leaf(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("failed to resolve current directory: {error}"))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    let mut missing_components = Vec::new();
    let mut existing_ancestor = normalized.as_path();
    while !existing_ancestor.exists() {
        let Some(file_name) = existing_ancestor.file_name() else {
            break;
        };
        missing_components.push(file_name.to_os_string());
        let Some(parent) = existing_ancestor.parent() else {
            break;
        };
        existing_ancestor = parent;
    }
    let mut resolved = fs::canonicalize(existing_ancestor).map_err(|error| {
        format!(
            "failed to resolve copied-dev-state ancestor {}: {error}",
            existing_ancestor.display()
        )
    })?;
    for component in missing_components.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn assert_concurrent_websocket_contract(
    config: ConcurrentWebSocketHarnessConfig,
    upstream: &ConcurrentWebSocketTranscript,
    registry_report: Option<&RouterWebSocketRegistryReport>,
) -> Result<(), String> {
    if upstream.expected_sessions != config.upstream.expected_sessions {
        return Err(format!(
            "concurrent upstream expected_sessions={} did not match config {}",
            upstream.expected_sessions, config.upstream.expected_sessions
        ));
    }
    if upstream.completed_sessions != config.upstream.expected_upstream_sessions {
        return Err(format!(
            "concurrent upstream completed {} sessions, expected {}",
            upstream.completed_sessions, config.upstream.expected_upstream_sessions
        ));
    }
    if upstream.final_active_sessions != 0 {
        return Err(format!(
            "concurrent upstream final active sessions was {}, expected 0",
            upstream.final_active_sessions
        ));
    }
    if upstream.active_high_water < config.upstream.expected_sessions {
        return Err(format!(
            "concurrent upstream high-water was {}, expected at least {}",
            upstream.active_high_water, config.upstream.expected_sessions
        ));
    }
    if upstream.target_model_session_count != config.upstream.expected_upstream_sessions {
        return Err(format!(
            "concurrent upstream observed {} target-model sessions, expected {} with {SMOKE_TARGET_MODEL}",
            upstream.target_model_session_count, config.upstream.expected_upstream_sessions
        ));
    }
    if !upstream.unexpected_response_create_models.is_empty() {
        return Err(format!(
            "concurrent upstream observed unexpected response.create models: {:?}",
            upstream.unexpected_response_create_models
        ));
    }
    if config.upstream.hold_duration > Duration::ZERO {
        if upstream.real_overlap_duration_ms < config.upstream.hold_duration.as_millis() {
            return Err(format!(
                "soak real overlap duration was {}ms, expected at least {}ms",
                upstream.real_overlap_duration_ms,
                config.upstream.hold_duration.as_millis()
            ));
        }
        if !config.quota_reconnect
            && upstream
                .in_overlap_session_event_counts
                .iter()
                .any(|event_count| *event_count < 3)
        {
            return Err(format!(
                "soak in-overlap session event counts were {:?}, expected at least 3 each",
                upstream.in_overlap_session_event_counts
            ));
        }
        if upstream.in_overlap_session_event_counts.len() < config.upstream.expected_sessions {
            return Err(format!(
                "soak in-overlap session event counts {:?} had fewer entries than expected sessions {}",
                upstream.in_overlap_session_event_counts, config.upstream.expected_sessions
            ));
        }
        if upstream.upstream_session_ids.len() < config.upstream.expected_sessions {
            return Err(format!(
                "soak upstream session ids {:?} had fewer entries than expected sessions {}",
                upstream.upstream_session_ids, config.upstream.expected_sessions
            ));
        }
        if upstream.normal_close_sessions < config.upstream.expected_upstream_sessions
            || upstream.abnormal_close_sessions != 0
        {
            return Err(format!(
                "soak close outcomes were {:?}; normal={} abnormal={} expected all {} normal",
                upstream.session_close_outcomes,
                upstream.normal_close_sessions,
                upstream.abnormal_close_sessions,
                config.upstream.expected_upstream_sessions
            ));
        }
        if !config.quota_reconnect && !upstream.multi_step_interleave_completed {
            return Err("soak did not complete a multi-step WebSocket interleave".to_owned());
        }
        if !config.quota_reconnect && upstream.multi_step_followup_frame_count == 0 {
            return Err(
                "soak did not observe a follow-up local frame before completion".to_owned(),
            );
        }
        if !config.quota_reconnect
            && upstream.multi_step_followup_active_session_count < config.upstream.expected_sessions
        {
            return Err(format!(
                "multi-step follow-up saw {} active sessions, expected at least {}",
                upstream.multi_step_followup_active_session_count,
                config.upstream.expected_sessions
            ));
        }
        if !config.quota_reconnect && !upstream.multi_step_completed_before_overlap_end {
            return Err(
                "multi-step WebSocket interleave did not complete before true 3-way overlap ended"
                    .to_owned(),
            );
        }
    }
    if config.quota_reconnect {
        if !upstream.quota_error_sent || !upstream.completion_sent {
            return Err(format!(
                "S8 overlap quota did not send both quota error and completion: {upstream:?}"
            ));
        }
        if upstream.quota_error_connection_label.as_deref() != Some(QUOTA_RECONNECT_PRIMARY.label) {
            return Err(format!(
                "S8 overlap quota first real request used {:?}, expected {}",
                upstream.quota_error_connection_label, QUOTA_RECONNECT_PRIMARY.label
            ));
        }
        if upstream.completion_connection_label.as_deref() != Some(QUOTA_RECONNECT_FALLBACK.label) {
            return Err(format!(
                "S8 overlap quota completion used {:?}, expected {}",
                upstream.completion_connection_label, QUOTA_RECONNECT_FALLBACK.label
            ));
        }
        let sqlite_pressure = upstream
            .sqlite_pressure
            .as_ref()
            .ok_or_else(|| "S8 overlap quota did not record copied SQLite pressure".to_owned())?;
        if !sqlite_pressure.acquired_before_quota_error
            || !sqlite_pressure.released_after_completion
        {
            return Err(format!(
                "S8 overlap quota SQLite pressure did not cover quota reconnect: {sqlite_pressure:?}"
            ));
        }
    }
    if config.capture_registry_report {
        let registry_report =
            registry_report.ok_or_else(|| "router registry report was not captured".to_owned())?;
        if registry_report.handled_connections != Some(config.router_max_connections) {
            return Err(format!(
                "router registry handled_connections={:?}, expected final CLI report with {}",
                registry_report.handled_connections, config.router_max_connections
            ));
        }
        if registry_report.active_sessions != 0 {
            return Err(format!(
                "router registry active_sessions={} after soak; expected 0",
                registry_report.active_sessions
            ));
        }
        if registry_report.high_water_sessions < config.upstream.expected_sessions {
            return Err(format!(
                "router registry high_water_sessions={} did not prove all {} sessions overlapped",
                registry_report.high_water_sessions, config.upstream.expected_sessions
            ));
        }
        if registry_report.registered_sessions < config.upstream.expected_sessions {
            return Err(format!(
                "router registry registered_sessions={} was less than expected sessions {}",
                registry_report.registered_sessions, config.upstream.expected_sessions
            ));
        }
        if registry_report.closed_sessions < config.upstream.expected_sessions {
            return Err(format!(
                "router registry closed_sessions={} was less than expected sessions {}",
                registry_report.closed_sessions, config.upstream.expected_sessions
            ));
        }
        if registry_report.completed_response_sessions < config.upstream.expected_sessions {
            return Err(format!(
                "router registry completed_response_sessions={} was less than expected sessions {}",
                registry_report.completed_response_sessions, config.upstream.expected_sessions
            ));
        }
        if registry_report
            .final_session_forwarded_upstream_message_counts
            .len()
            < config.upstream.expected_sessions
        {
            return Err(format!(
                "router registry final-session forwarded counts {:?} had fewer entries than expected sessions {}",
                registry_report.final_session_forwarded_upstream_message_counts,
                config.upstream.expected_sessions
            ));
        }
        let mut sorted_forwarded_counts = registry_report
            .final_session_forwarded_upstream_message_counts
            .clone();
        sorted_forwarded_counts.sort_unstable_by(|left, right| right.cmp(left));
        if sorted_forwarded_counts
            .iter()
            .take(config.upstream.expected_sessions)
            .any(|count| *count < 3)
        {
            return Err(format!(
                "router registry final-session forwarded counts {:?} did not prove three unique sessions with at least three local writes",
                registry_report.final_session_forwarded_upstream_message_counts
            ));
        }
        if registry_report.forwarded_upstream_messages
            < config.upstream.expected_sessions.saturating_mul(3)
        {
            return Err(format!(
                "router registry forwarded_upstream_messages={} was less than expected minimum {}",
                registry_report.forwarded_upstream_messages,
                config.upstream.expected_sessions.saturating_mul(3)
            ));
        }
    }

    Ok(())
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
    let seed = seed_router_state(&state_path, &secret_root)?;
    let router_port = reserve_loopback_port()?;
    let audit_path = router_root.join("audit").join("events.jsonl");
    let router_process = start_router_process(
        router_port,
        state_path,
        secret_root,
        Some(seed.local_token),
        format!("http://{}/v1", upstream.address()),
        audit_path,
    )?;

    send_hostile_no_token_websocket(router_port)?;
    let _router_process = router_process.stop("hostile no-token router process")?;
    let upstream_connection_count = upstream.join()?;
    if upstream_connection_count != 0 {
        return Err(format!(
            "hostile no-token smoke reached upstream {upstream_connection_count} time(s)"
        ));
    }

    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SmokeQuotaStatus {
    table: String,
    plain: String,
    json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SmokeSeed {
    local_token_assignment: String,
    local_token: String,
    expected_account_label: String,
    expected_account_tag: String,
    expected_upstream_token: String,
    routable_upstream_tokens: Vec<String>,
    quota_status: SmokeQuotaStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SmokeAccountFixture {
    account_id: &'static str,
    label: &'static str,
    upstream_token: &'static str,
    short_remaining: u32,
    short_reset: u64,
    weekly_remaining: u32,
    weekly_reset: u64,
    weekly_status: SelectorQuotaWindowStatus,
}

const SMOKE_ACCOUNT_FIXTURES: &[SmokeAccountFixture] = &[
    SmokeAccountFixture {
        account_id: "acct_askluna",
        label: "askluna",
        upstream_token: "installed-smoke-askluna-token",
        short_remaining: 100,
        short_reset: 17_900,
        weekly_remaining: 0,
        weekly_reset: 130_600,
        weekly_status: SelectorQuotaWindowStatus::Ineligible,
    },
    SmokeAccountFixture {
        account_id: "acct_matches",
        label: "matches",
        upstream_token: "installed-smoke-matches-token",
        short_remaining: 91,
        short_reset: 16_000,
        weekly_remaining: 54,
        weekly_reset: 525_000,
        weekly_status: SelectorQuotaWindowStatus::Eligible,
    },
    SmokeAccountFixture {
        account_id: "acct_ssdev",
        label: "ssdev",
        upstream_token: "installed-smoke-ssdev-token",
        short_remaining: 100,
        short_reset: 15_000,
        weekly_remaining: 16,
        weekly_reset: 120_000,
        weekly_status: SelectorQuotaWindowStatus::Eligible,
    },
];

const QUOTA_RECONNECT_PRIMARY: SmokeAccountFixture = SmokeAccountFixture {
    account_id: "acct_quota_primary",
    label: "quota-primary",
    upstream_token: "installed-quota-primary-token",
    short_remaining: 100,
    short_reset: 17_900,
    weekly_remaining: 100,
    weekly_reset: 525_000,
    weekly_status: SelectorQuotaWindowStatus::Eligible,
};

const QUOTA_RECONNECT_FALLBACK: SmokeAccountFixture = SmokeAccountFixture {
    account_id: "acct_quota_fallback",
    label: "quota-fallback",
    upstream_token: "installed-quota-fallback-token",
    short_remaining: 80,
    short_reset: 17_900,
    weekly_remaining: 80,
    weekly_reset: 525_000,
    weekly_status: SelectorQuotaWindowStatus::Eligible,
};

const SMOKE_SELECTOR_STALE_AFTER_SECONDS: u64 = 300;

fn seed_router_state(state_path: &Path, secret_root: &Path) -> Result<SmokeSeed, String> {
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

    disable_accounts_outside_fixtures(&state, SMOKE_ACCOUNT_FIXTURES, "three-websocket")?;
    reset_fixture_route_band_state(state_path, SMOKE_ACCOUNT_FIXTURES, "three-websocket")?;
    for fixture in SMOKE_ACCOUNT_FIXTURES {
        seed_smoke_account(&state, &secrets, *fixture)?;
    }

    let quota_status = capture_quota_status(state_path)?;
    let selected_account = selected_account_from_status_json(&quota_status.json)?;
    let selected_fixture = SMOKE_ACCOUNT_FIXTURES
        .iter()
        .find(|fixture| fixture.label == selected_account.safe_label)
        .ok_or_else(|| {
            format!(
                "quota status selected unknown smoke account: {}",
                selected_account.safe_label
            )
        })?;

    Ok(SmokeSeed {
        local_token_assignment,
        local_token: exported_token,
        expected_account_label: selected_fixture.label.to_owned(),
        expected_account_tag: selected_account.account_hash,
        expected_upstream_token: selected_fixture.upstream_token.to_owned(),
        routable_upstream_tokens: SMOKE_ACCOUNT_FIXTURES
            .iter()
            .filter(|fixture| fixture.weekly_status == SelectorQuotaWindowStatus::Eligible)
            .map(|fixture| fixture.upstream_token.to_owned())
            .collect(),
        quota_status,
    })
}

fn seed_quota_reconnect_router_state(state_path: &Path, secret_root: &Path) -> Result<(), String> {
    let state = SqliteStateStore::open(state_path)
        .map_err(|error| format!("failed to open quota reconnect SQLite state: {error}"))?;
    let secrets = FileSecretStore::open(secret_root)
        .map_err(|error| format!("failed to open quota reconnect secret store: {error}"))?;
    disable_accounts_outside_fixtures(
        &state,
        &[QUOTA_RECONNECT_PRIMARY, QUOTA_RECONNECT_FALLBACK],
        "quota reconnect",
    )?;
    reset_fixture_route_band_state(
        state_path,
        &[QUOTA_RECONNECT_PRIMARY, QUOTA_RECONNECT_FALLBACK],
        "quota reconnect",
    )?;
    seed_smoke_account(&state, &secrets, QUOTA_RECONNECT_PRIMARY)?;
    seed_smoke_account(&state, &secrets, QUOTA_RECONNECT_FALLBACK)?;
    Ok(())
}

fn seed_s8_overlap_quota_router_state(
    state_path: &Path,
    secret_root: &Path,
) -> Result<SmokeSeed, String> {
    seed_quota_reconnect_router_state(state_path, secret_root)?;
    let secrets = FileSecretStore::open(secret_root)
        .map_err(|error| format!("failed to open S8 overlap quota secret store: {error}"))?;
    let token_service = LocalRouterTokenService::new(secrets);
    let local_token = token_service
        .rotate()
        .map_err(|error| format!("failed to rotate S8 overlap quota local token: {error}"))?;
    let local_token_assignment = export_token_assignment(
        "CODEX_ROUTER_TOKEN",
        local_token.token().expose_secret(),
        Shell::Posix,
    );
    let exported_token = parse_posix_token_assignment(&local_token_assignment)?;
    let quota_status = capture_quota_status(state_path)?;
    let selected_account = selected_account_from_status_json(&quota_status.json)?;

    Ok(SmokeSeed {
        local_token_assignment,
        local_token: exported_token,
        expected_account_label: selected_account.safe_label,
        expected_account_tag: selected_account.account_hash,
        expected_upstream_token: QUOTA_RECONNECT_PRIMARY.upstream_token.to_owned(),
        routable_upstream_tokens: vec![
            QUOTA_RECONNECT_PRIMARY.upstream_token.to_owned(),
            QUOTA_RECONNECT_FALLBACK.upstream_token.to_owned(),
        ],
        quota_status,
    })
}

fn reset_fixture_route_band_state(
    state_path: &Path,
    fixtures: &[SmokeAccountFixture],
    scenario_label: &str,
) -> Result<(), String> {
    let mut command = Command::new("python3");
    command
        .arg("-c")
        .arg(
            r#"
import sqlite3
import sys

database_path = sys.argv[1]
account_ids = sys.argv[2:]
connection = sqlite3.connect(database_path, timeout=0)
try:
    for account_id in account_ids:
        connection.execute(
            "DELETE FROM route_band_account_states WHERE account_id = ? AND route_band = 'responses'",
            (account_id,),
        )
    connection.commit()
finally:
    connection.close()
"#,
        )
        .arg(state_path);
    for account_fixture in fixtures {
        command.arg(account_fixture.account_id);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run {scenario_label} state reset helper: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to reset {scenario_label} route-band state in {}: status={} stderr={}",
            state_path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn disable_accounts_outside_fixtures(
    state: &SqliteStateStore,
    allowed_fixtures: &[SmokeAccountFixture],
    scenario_label: &str,
) -> Result<(), String> {
    for account in AccountStateRepository::list_accounts(state)
        .map_err(|error| format!("failed to list accounts for {scenario_label} fixture: {error}"))?
    {
        if allowed_fixtures
            .iter()
            .any(|fixture| fixture.account_id == account.account_id().as_str())
        {
            continue;
        }
        let mut disabled_account = AccountRecord::new(
            account.account_id().clone(),
            account.label().to_owned(),
            AccountStatus::Disabled,
        );
        if let Some(active_generation) = account.active_credential_generation() {
            disabled_account =
                disabled_account.with_active_credential_generation(active_generation);
        }
        AccountStateRepository::upsert_account(state, &disabled_account).map_err(|error| {
            format!(
                "failed to disable non-{scenario_label} account {}: {error}",
                account.label()
            )
        })?;
    }
    Ok(())
}

fn seed_smoke_account(
    state: &SqliteStateStore,
    secrets: &FileSecretStore,
    fixture: SmokeAccountFixture,
) -> Result<(), String> {
    let account_id = account_id(fixture.account_id)?;
    let observed_unix_seconds = timestamp_seconds();
    let short_reset_unix_seconds = observed_unix_seconds.saturating_add(fixture.short_reset);
    let weekly_reset_unix_seconds = observed_unix_seconds.saturating_add(fixture.weekly_reset);
    let stale_after_unix_seconds =
        observed_unix_seconds.saturating_add(SMOKE_SELECTOR_STALE_AFTER_SECONDS);
    let account = AccountRecord::new(account_id.clone(), fixture.label, AccountStatus::Enabled)
        .with_active_credential_generation(1);
    AccountStateRepository::upsert_account(state, &account)
        .map_err(|error| format!("failed to seed smoke account {}: {error}", fixture.label))?;
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band("responses", fixture.short_remaining)
            .with_reset_unix_seconds(short_reset_unix_seconds);
    QuotaSnapshotRepository::upsert_snapshot(state, &snapshot).map_err(|error| {
        format!(
            "failed to seed smoke quota snapshot for {}: {error}",
            fixture.label
        )
    })?;
    let short_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(fixture.short_remaining)
    .with_reset_unix_seconds(short_reset_unix_seconds)
    .with_effective(true)
    .with_observed_unix_seconds(observed_unix_seconds);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        604_800,
        fixture.weekly_status,
    )
    .with_remaining_headroom(fixture.weekly_remaining)
    .with_reset_unix_seconds(weekly_reset_unix_seconds)
    .with_observed_unix_seconds(observed_unix_seconds);
    SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        state,
        &account_id,
        "responses",
        &[short_window, weekly_window],
        observed_unix_seconds,
        stale_after_unix_seconds,
    )
    .map_err(|error| {
        format!(
            "failed to seed fresh smoke selector windows for {}: {error}",
            fixture.label
        )
    })?;
    let credential_key = account_credential_bundle_key(&account_id, 1)
        .map_err(|error| format!("failed to build account credential key: {error}"))?;
    let credential_bundle = AccountCredentialBundle::imported_codex_auth(
        fixture.upstream_token,
        Some(format!("{}-refresh", fixture.upstream_token)),
    )
    .to_secret_string()
    .map_err(|error| format!("failed to serialize smoke credential bundle: {error}"))?;
    secrets
        .write_secret(&credential_key, &credential_bundle)
        .map_err(|error| format!("failed to write smoke credential bundle: {error}"))?;
    let legacy_token_key = upstream_access_token_key(&account_id)
        .map_err(|error| format!("failed to build upstream token key: {error}"))?;
    secrets
        .write_secret(
            &legacy_token_key,
            &SecretString::new(fixture.upstream_token.to_owned()),
        )
        .map_err(|error| format!("failed to write smoke upstream token: {error}"))?;

    Ok(())
}

fn capture_quota_status(state_path: &Path) -> Result<SmokeQuotaStatus, String> {
    let router_root = state_path
        .parent()
        .ok_or_else(|| "state path had no router root parent".to_owned())?;
    Ok(SmokeQuotaStatus {
        table: run_quota_status(router_root, "table")?,
        plain: run_quota_status(router_root, "plain")?,
        json: run_quota_status(router_root, "json")?,
    })
}

fn run_quota_status(router_root: &Path, format: &str) -> Result<String, String> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    run_with_io(
        vec![
            OsString::from("codex-router"),
            OsString::from("quota"),
            OsString::from("status"),
            OsString::from("--router-root"),
            router_root.as_os_str().to_owned(),
            OsString::from("--no-refresh"),
            OsString::from("--format"),
            OsString::from(format),
            OsString::from("--now-unix-seconds"),
            OsString::from("1030"),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    )
    .map_err(|error| {
        format!(
            "quota status {format} failed: {error}; stderr={}",
            String::from_utf8_lossy(&stderr)
        )
    })?;
    if !stderr.is_empty() {
        return Err(format!(
            "quota status {format} wrote stderr: {}",
            String::from_utf8_lossy(&stderr)
        ));
    }
    String::from_utf8(stdout)
        .map_err(|error| format!("quota status {format} was not UTF-8: {error}"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectedQuotaStatusAccount {
    safe_label: String,
    account_hash: String,
}

fn selected_account_from_status_json(payload: &str) -> Result<SelectedQuotaStatusAccount, String> {
    let value: Value = serde_json::from_str(payload)
        .map_err(|error| format!("quota status json was invalid: {error}"))?;
    value
        .get("accounts")
        .and_then(Value::as_array)
        .and_then(|accounts| {
            accounts.iter().find_map(|account| {
                if !account
                    .get("preferred_next")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return None;
                }
                Some(SelectedQuotaStatusAccount {
                    safe_label: account
                        .get("safe_account_label")
                        .and_then(Value::as_str)?
                        .to_owned(),
                    account_hash: account
                        .get("account_hash")
                        .and_then(Value::as_str)?
                        .to_owned(),
                })
            })
        })
        .ok_or_else(|| "quota status json did not include preferred account label".to_owned())
}

fn smoke_account_label_from_upstream_token(token: &str) -> Option<&'static str> {
    SMOKE_ACCOUNT_FIXTURES
        .iter()
        .find(|fixture| fixture.upstream_token == token)
        .map(|fixture| fixture.label)
}

fn start_router_process(
    router_port: u16,
    state_path: PathBuf,
    secret_root: PathBuf,
    local_token: Option<String>,
    upstream_base_url: String,
    audit_path: PathBuf,
) -> Result<RouterProcessGuard, String> {
    start_router_process_with_options(RouterProcessStartOptions {
        now_unix_seconds: Some(1_030),
        router_port,
        state_path,
        secret_root,
        local_token,
        upstream_base_url,
        audit_path,
        max_connections: 64,
        websocket_registry_report_file: None,
    })
}

struct RouterProcessStartOptions {
    now_unix_seconds: Option<u64>,
    router_port: u16,
    state_path: PathBuf,
    secret_root: PathBuf,
    local_token: Option<String>,
    upstream_base_url: String,
    audit_path: PathBuf,
    max_connections: usize,
    websocket_registry_report_file: Option<PathBuf>,
}

fn start_router_process_with_options(
    options: RouterProcessStartOptions,
) -> Result<RouterProcessGuard, String> {
    let binary_path = codex_router_binary_path()?;
    let mut argv = vec![
        "serve".to_owned(),
        "--port".to_owned(),
        options.router_port.to_string(),
        "--listen-host".to_owned(),
        "127.0.0.1".to_owned(),
        "--state-db".to_owned(),
        options.state_path.display().to_string(),
        "--secret-root".to_owned(),
        options.secret_root.display().to_string(),
        "--upstream-base-url".to_owned(),
        options.upstream_base_url,
        "--max-snapshot-age-seconds".to_owned(),
        "60".to_owned(),
        "--disable-background-quota-refresh".to_owned(),
        "--max-connections".to_owned(),
        options.max_connections.to_string(),
        "--audit-file".to_owned(),
        options.audit_path.display().to_string(),
    ];
    if let Some(now_unix_seconds) = options.now_unix_seconds {
        argv.extend([
            "--now-unix-seconds".to_owned(),
            now_unix_seconds.to_string(),
        ]);
    }
    if let Some(report_file) = options.websocket_registry_report_file {
        argv.extend([
            "--websocket-registry-report-file".to_owned(),
            report_file.display().to_string(),
        ]);
    }
    if options.local_token.is_some() {
        argv.push("--require-local-token".to_owned());
    }
    let mut command = Command::new(&binary_path);
    command.args(&argv);
    if cfg!(debug_assertions) {
        command
            .env("CODEX_ROUTER_TEST_CAPACITY_RETRY_DELAY_SECONDS", "2")
            .env("CODEX_ROUTER_TEST_SHORT_QUOTA_WAIT_JITTER_SECONDS", "2");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        format!(
            "failed to spawn codex-router serve child {}: {error}",
            binary_path.display()
        )
    })?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "router child stdout was not piped".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "router child stderr was not piped".to_owned())?;
    let (line_sender, line_receiver) = mpsc::channel();
    let stdout_handle = spawn_router_output_reader("router stdout", stdout, Some(line_sender))?;
    let stderr_handle = spawn_router_output_reader("router stderr", stderr, None)?;
    let readiness_line =
        wait_for_router_readiness(&mut child, &line_receiver, options.router_port)?;
    let listener = readiness_line
        .trim()
        .strip_prefix("listening: ")
        .unwrap_or_else(|| readiness_line.trim())
        .to_owned();

    Ok(RouterProcessGuard {
        child: Some(child),
        stdout_handle: Some(stdout_handle),
        stderr_handle: Some(stderr_handle),
        observation: RouterProcessObservation {
            binary_path,
            pid,
            argv,
            listener,
            readiness_line,
            cleanup_result: "not-cleaned".to_owned(),
        },
    })
}

fn codex_router_binary_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_codex-router") {
        return Ok(PathBuf::from(path));
    }
    let workspace_root = workspace_root()?;
    let binary_name = if cfg!(windows) {
        "codex-router.exe"
    } else {
        "codex-router"
    };
    Ok(workspace_root
        .join("target")
        .join("debug")
        .join(binary_name))
}

fn workspace_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "failed to resolve workspace root".to_owned())
}

fn spawn_router_output_reader<R>(
    name: &'static str,
    stream: R,
    line_sender: Option<mpsc::Sender<String>>,
) -> Result<thread::JoinHandle<Vec<String>>, String>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(format!("codex-router-installed-smoke-{name}"))
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => return lines,
                    Ok(_) => {
                        let line = line.trim_end_matches(['\r', '\n']).to_owned();
                        if let Some(sender) = &line_sender {
                            let _ = sender.send(line.clone());
                        }
                        lines.push(line);
                    }
                    Err(error) => {
                        lines.push(format!("<{name} read error: {error}>"));
                        return lines;
                    }
                }
            }
        })
        .map_err(|error| format!("failed to spawn {name} reader: {error}"))
}

fn wait_for_router_readiness(
    child: &mut Child,
    line_receiver: &mpsc::Receiver<String>,
    router_port: u16,
) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "router child exited before readiness on port {router_port}: {status}"
            ));
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "router child did not print readiness for port {router_port} before timeout"
            ));
        }
        let remaining = deadline.saturating_duration_since(now);
        let wait = remaining.min(Duration::from_millis(50));
        match line_receiver.recv_timeout(wait) {
            Ok(line) if line.starts_with("listening: ") => return Ok(line),
            Ok(_line) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(format!(
                    "router child stdout closed before readiness on port {router_port}"
                ));
            }
        }
    }
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
    _local_token_assignment: &str,
    child_environment: CodexChildEnvironment,
) -> Result<Output, String> {
    run_codex_exec_with_timeout(
        transport_mode,
        codex_home,
        workdir,
        last_message_path,
        child_environment,
        CODEX_COMMAND_TIMEOUT,
    )
}

fn run_codex_exec_with_timeout(
    transport_mode: CodexTransportMode,
    codex_home: &Path,
    workdir: &Path,
    last_message_path: &Path,
    child_environment: CodexChildEnvironment,
    timeout: Duration,
) -> Result<Output, String> {
    run_codex_exec_with_timeout_observed(CodexExecRequest {
        transport_mode,
        codex_home,
        workdir,
        last_message_path,
        child_environment,
        timeout,
        prompt: SMOKE_PROMPT,
        client_index: None,
    })
    .map(|run| run.output)
}

fn run_codex_exec_with_timeout_observed(
    request: CodexExecRequest<'_>,
) -> Result<CodexChildRun, String> {
    let CodexExecRequest {
        transport_mode,
        codex_home,
        workdir,
        last_message_path,
        child_environment,
        timeout,
        prompt,
        client_index,
    } = request;
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
        .arg("-c")
        .arg(SMOKE_TARGET_MODEL_OVERRIDE)
        .arg("--ephemeral")
        .arg("--output-last-message")
        .arg(last_message_path);
    if transport_mode == CodexTransportMode::HttpSse {
        command
            .arg("-c")
            .arg("model_providers.codex-router.supports_websockets=false");
    }
    command
        .arg(prompt)
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg_config_home)
        .env("XDG_STATE_HOME", xdg_state_home)
        .env("XDG_CACHE_HOME", xdg_cache_home)
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }

    run_with_timeout_observed(command, timeout).map_err(|error| {
        format!(
            "{error}; {}",
            codex_child_timeout_diagnostics(client_index, last_message_path)
        )
    })
}

#[cfg(test)]
fn run_with_timeout(command: Command, timeout: Duration) -> Result<Output, String> {
    run_with_timeout_observed(command, timeout).map(|run| run.output)
}

fn run_with_timeout_observed(
    mut command: Command,
    timeout: Duration,
) -> Result<CodexChildRun, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn installed codex: {error}"))?;
    let pid = child.id();
    let stdout_reader = child
        .stdout
        .take()
        .map(|stdout| spawn_child_output_reader("stdout", stdout))
        .transpose()?;
    let stderr_reader = child
        .stderr
        .take()
        .map(|stderr| spawn_child_output_reader("stderr", stderr))
        .transpose()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = collect_child_output(status, stdout_reader, stderr_reader)?;
                return Ok(CodexChildRun { pid, output });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let status = child.wait().map_err(|error| {
                        format!("failed to wait for timed-out installed codex: {error}")
                    })?;
                    let output = collect_child_output(status, stdout_reader, stderr_reader)?;
                    let stdout_byte_count = output.stdout.len();
                    let stderr_byte_count = output.stderr.len();
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(format!(
                        "installed codex timed out after {}s; captured stdout/stderr suppressed to avoid leaking secrets (stdout_bytes={stdout_byte_count}, stderr_bytes={stderr_byte_count}); stdout_preview={}; stderr_preview={}; stdout_markers={}; stderr_markers={}",
                        timeout.as_secs(),
                        redacted_process_output_preview(&stdout),
                        redacted_process_output_preview(&stderr),
                        process_output_markers(&stdout),
                        process_output_markers(&stderr),
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("failed to poll installed codex: {error}")),
        }
    }
}

fn spawn_child_output_reader<R>(
    stream_name: &'static str,
    mut stream: R,
) -> Result<thread::JoinHandle<Result<Vec<u8>, String>>, String>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(format!("codex-router-installed-codex-{stream_name}-reader"))
        .spawn(move || {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).map_err(|error| {
                format!("failed to read installed codex {stream_name}: {error}")
            })?;
            Ok(bytes)
        })
        .map_err(|error| format!("failed to spawn installed codex {stream_name} reader: {error}"))
}

fn collect_child_output(
    status: ExitStatus,
    stdout_reader: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
    stderr_reader: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
) -> Result<Output, String> {
    let stdout = join_child_output_reader(stdout_reader, "stdout")?;
    let stderr = join_child_output_reader(stderr_reader, "stderr")?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn join_child_output_reader(
    reader: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
    stream_name: &str,
) -> Result<Vec<u8>, String> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| format!("installed codex {stream_name} reader panicked"))?,
        None => Ok(Vec::new()),
    }
}

fn assert_codex_visible_output(
    label: &str,
    output: &Output,
    last_message_path: &Path,
) -> Result<(), String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(SMOKE_EXPECTED_TEXT) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{label} smoke stdout did not contain expected response text; status={}; stdout_preview={}; stderr_preview={}; stdout_markers={}; stderr_markers={}",
            output.status,
            redacted_process_output_preview(&stdout),
            redacted_process_output_preview(&stderr),
            process_output_markers(&stdout),
            process_output_markers(&stderr),
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

fn redacted_process_output_preview(output: &str) -> String {
    if output.trim().is_empty() {
        "<empty>".to_owned()
    } else {
        "<suppressed>".to_owned()
    }
}

fn process_output_markers(output: &str) -> String {
    let markers = stderr_transport_error_markers(output);
    if markers.is_empty() {
        "<none>".to_owned()
    } else {
        markers.join(",")
    }
}

fn codex_child_timeout_diagnostics(
    client_index: Option<usize>,
    last_message_path: &Path,
) -> String {
    let client_index = client_index
        .map(|index| index.to_string())
        .unwrap_or_else(|| "n/a".to_owned());
    match fs::read(last_message_path) {
        Ok(bytes) => {
            let contains_expected = String::from_utf8_lossy(&bytes).contains(SMOKE_EXPECTED_TEXT);
            format!(
                "child_diagnostics=client_index:{client_index},last_message_exists:true,last_message_bytes:{},last_message_contains_expected:{contains_expected}",
                bytes.len()
            )
        }
        Err(error) if error.kind() == ErrorKind::NotFound => format!(
            "child_diagnostics=client_index:{client_index},last_message_exists:false,last_message_bytes:0,last_message_contains_expected:false"
        ),
        Err(error) => format!(
            "child_diagnostics=client_index:{client_index},last_message_exists:unknown,last_message_read_error:{},last_message_contains_expected:false",
            error.kind()
        ),
    }
}

struct SmokeContractAssertion<'a> {
    mode: InstalledCodexSmokeMode,
    http_sse_codex_status: Option<&'a ExitStatus>,
    websocket_codex_status: Option<&'a ExitStatus>,
    upstream: &'a MockWebSocketTranscript,
    local_token: &'a str,
    expected_account_label: &'a str,
    expected_upstream_token: &'a str,
    routable_upstream_tokens: &'a [String],
    quota_status: &'a SmokeQuotaStatus,
}

fn assert_smoke_contract(assertion: SmokeContractAssertion<'_>) -> Result<(), String> {
    if let Some(status) = assertion.http_sse_codex_status
        && !status.success()
    {
        return Err(format!(
            "installed codex HTTP/SSE smoke exited with status {status}"
        ));
    }
    if let Some(status) = assertion.websocket_codex_status
        && !status.success()
    {
        return Err(format!(
            "installed codex WebSocket smoke exited with status {status}"
        ));
    }
    let http_sse_authorization = if assertion.mode.requires_http_sse() {
        Some(assert_http_sse_contract(&assertion)?)
    } else {
        None
    };
    let websocket_authorization = if assertion.mode.requires_websocket() {
        Some(assert_websocket_contract(&assertion)?)
    } else {
        None
    };
    if assertion.mode == InstalledCodexSmokeMode::Combined
        && http_sse_authorization != websocket_authorization
    {
        return Err(format!(
            "WebSocket did not reuse the held HTTP/SSE account inside cooldown; expected_account_hint={}; http_sse_authorization={}; websocket_authorization={}",
            assertion.expected_account_label,
            http_sse_authorization.unwrap_or_else(|| "<missing>".to_owned()),
            websocket_authorization.unwrap_or_else(|| "<missing>".to_owned())
        ));
    }
    if !assertion
        .quota_status
        .table
        .contains(assertion.expected_account_label)
    {
        return Err("quota status table did not include selected account label".to_owned());
    }
    if !assertion
        .quota_status
        .plain
        .contains(assertion.expected_account_label)
    {
        return Err("quota status plain output did not include selected account label".to_owned());
    }
    if !assertion
        .quota_status
        .json
        .contains(assertion.expected_account_label)
    {
        return Err("quota status json did not include selected account label".to_owned());
    }
    if !assertion.quota_status.plain.contains("\tnext") {
        return Err("quota status plain output did not mark a next account".to_owned());
    }
    for forbidden in [
        assertion.local_token,
        "X-Codex-Router-Token",
        "authorization",
        "bottleneck",
        "pp",
    ] {
        if assertion.quota_status.table.contains(forbidden)
            || assertion.quota_status.plain.contains(forbidden)
        {
            return Err(format!(
                "human quota status leaked forbidden text: {forbidden}"
            ));
        }
    }

    Ok(())
}

fn assert_http_sse_contract(assertion: &SmokeContractAssertion<'_>) -> Result<String, String> {
    let http_sse =
        assertion.upstream.http_sse.as_ref().ok_or_else(|| {
            "mock upstream did not capture HTTP/SSE /v1/responses traffic".to_owned()
        })?;
    if !http_sse.request_line.starts_with("POST /v1/responses ") {
        return Err(format!(
            "HTTP/SSE request was not POST /v1/responses: {}",
            http_sse.request_line
        ));
    }
    if http_sse.header("x-codex-router-token").is_some() {
        return Err("HTTP/SSE request leaked local router token header upstream".to_owned());
    }
    if http_sse.body.contains(assertion.local_token) {
        return Err("HTTP/SSE request body leaked local router token upstream".to_owned());
    }
    if !http_sse_request_asks_streaming(http_sse) {
        return Err(format!(
            "HTTP/SSE request did not ask for a streaming response; body_shape={}; transfer_encoding={}; content_length={}",
            http_sse_body_shape_summary(&http_sse.body),
            http_sse
                .header("transfer-encoding")
                .as_deref()
                .unwrap_or("<none>"),
            http_sse
                .header("content-length")
                .as_deref()
                .unwrap_or("<none>")
        ));
    }
    let authorization = http_sse
        .header("authorization")
        .ok_or_else(|| "HTTP/SSE request did not receive an Authorization header".to_owned())?;
    let token = authorization
        .strip_prefix("Bearer ")
        .ok_or_else(|| "HTTP/SSE Authorization header was not bearer".to_owned())?;
    if !assertion
        .routable_upstream_tokens
        .iter()
        .any(|routable_token| routable_token == token)
    {
        return Err("HTTP/SSE token was not one of the routable account tokens".to_owned());
    }
    if token != assertion.expected_upstream_token {
        return Err(format!(
            "HTTP/SSE selected a different upstream account than expected; expected_label={}; actual_label={}",
            assertion.expected_account_label,
            smoke_account_label_from_upstream_token(token).unwrap_or("<unknown>")
        ));
    }

    Ok(authorization)
}

fn http_sse_request_asks_streaming(request: &MockHttpSseTranscript) -> bool {
    request.request_line.contains("stream=true")
        || request
            .header("accept")
            .as_deref()
            .is_some_and(|accept| accept.contains("text/event-stream"))
        || http_sse_body_requests_streaming(&request.body)
}

fn http_sse_body_requests_streaming(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(Value::as_bool))
        == Some(true)
}

fn http_sse_body_shape_summary(body: &str) -> String {
    let value = match serde_json::from_str::<Value>(body) {
        Ok(value) => value,
        Err(error) => {
            let hex_prefix = body
                .as_bytes()
                .iter()
                .take(16)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join("");
            return format!(
                "invalid-json-len-{}-hex-{hex_prefix}-error-{:?}",
                body.len(),
                error.classify()
            );
        }
    };
    let Some(object) = value.as_object() else {
        return value.as_array().map_or("non-object".to_owned(), |array| {
            format!("array-len-{}", array.len())
        });
    };
    let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();
    format!("keys={}", keys.join(","))
}

fn assert_websocket_contract(assertion: &SmokeContractAssertion<'_>) -> Result<String, String> {
    let authorization = assertion
        .upstream
        .header("authorization")
        .ok_or_else(|| "mock upstream did not receive Authorization header".to_owned())?;
    let websocket_token = authorization
        .strip_prefix("Bearer ")
        .ok_or_else(|| "mock upstream Authorization header was not bearer".to_owned())?;
    if !assertion
        .routable_upstream_tokens
        .iter()
        .any(|token| token == websocket_token)
    {
        return Err(
            "mock upstream WebSocket token was not one of the routable account tokens".to_owned(),
        );
    }
    if websocket_token != assertion.expected_upstream_token {
        return Err(format!(
            "mock upstream WebSocket selected a different account than expected; expected_label={}; actual_label={}",
            assertion.expected_account_label,
            smoke_account_label_from_upstream_token(websocket_token).unwrap_or("<unknown>")
        ));
    }
    if assertion.upstream.header("x-codex-router-token").is_some() {
        return Err("mock upstream websocket received local router token header".to_owned());
    }
    if assertion
        .upstream
        .request_frames
        .iter()
        .any(|frame| frame.contains(assertion.local_token))
    {
        return Err("mock upstream websocket frame leaked local router token".to_owned());
    }
    if assertion.upstream.websocket_request_frame_count == 0 {
        return Err("mock upstream did not receive a WebSocket request frame".to_owned());
    }
    if !assertion
        .upstream
        .request_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .any(|value| is_non_prewarm_response_create_frame(&value))
    {
        return Err(
            "mock upstream did not receive a non-prewarm WebSocket response request".to_owned(),
        );
    }

    Ok(authorization)
}

fn bearer_token_from_authorization_header(authorization: Option<&str>) -> Option<&str> {
    authorization?.strip_prefix("Bearer ")
}

fn bearer_token_from_headers(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case("authorization"))
        .and_then(|(_, value)| bearer_token_from_authorization_header(Some(value)))
}

fn authorization_header_matches_expected(
    authorization: Option<String>,
    expected_token: &str,
) -> Option<bool> {
    let authorization = authorization?;
    Some(bearer_token_from_authorization_header(Some(&authorization))? == expected_token)
}

fn upstream_label_from_authorization_header(authorization: Option<String>) -> Option<&'static str> {
    let authorization = authorization?;
    smoke_account_label_from_upstream_token(bearer_token_from_authorization_header(Some(
        &authorization,
    ))?)
}

fn quota_reconnect_label_from_upstream_token(token: &str) -> Option<&'static str> {
    [
        (
            QUOTA_RECONNECT_PRIMARY.upstream_token,
            QUOTA_RECONNECT_PRIMARY.label,
        ),
        (
            QUOTA_RECONNECT_FALLBACK.upstream_token,
            QUOTA_RECONNECT_FALLBACK.label,
        ),
    ]
    .into_iter()
    .find_map(|(candidate_token, label)| (candidate_token == token).then_some(label))
}

fn quota_reconnect_role_from_label(label: Option<&str>) -> &'static str {
    match label {
        Some(candidate) if candidate == QUOTA_RECONNECT_PRIMARY.label => "primary",
        Some(candidate) if candidate == QUOTA_RECONNECT_FALLBACK.label => "fallback",
        Some(_) => "unknown",
        None => "none",
    }
}

fn quota_reconnect_usage_limit_frame() -> &'static str {
    r#"{"type":"error","status":429,"error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#
}

fn assert_codex_quota_reconnect_output_is_safe(
    output: &Output,
    last_message_path: &Path,
) -> Result<(), String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last_message = fs::read_to_string(last_message_path).map_err(|error| {
        format!(
            "quota reconnect smoke failed to read last-message file {}: {error}",
            last_message_path.display()
        )
    })?;
    for (label, contents) in [
        ("stdout", stdout.as_ref()),
        ("stderr", stderr.as_ref()),
        ("last_message", last_message.as_str()),
    ] {
        if contents.contains("usage_limit_reached") {
            return Err(format!(
                "quota reconnect leaked provider quota error to Codex {label}"
            ));
        }
        if contents.contains("codex_router_all_accounts_exhausted") {
            return Err(format!(
                "quota reconnect incorrectly reported all accounts exhausted in Codex {label}"
            ));
        }
    }
    Ok(())
}

fn assert_quota_reconnect_contract(
    transcript: &QuotaReconnectWebSocketTranscript,
) -> Result<(), String> {
    if transcript.websocket_handshake_count < 2 {
        return Err(format!(
            "quota reconnect expected at least two upstream websocket handshakes, observed {}",
            transcript.websocket_handshake_count
        ));
    }
    if transcript.non_prewarm_frame_count < 2 {
        return Err(format!(
            "quota reconnect expected at least two non-prewarm requests, observed {}",
            transcript.non_prewarm_frame_count
        ));
    }
    if !transcript.quota_error_sent || !transcript.completion_sent {
        return Err(format!(
            "quota reconnect did not send both quota error and completion: {transcript:?}"
        ));
    }
    if transcript.quota_error_connection_label.as_deref() != Some(QUOTA_RECONNECT_PRIMARY.label) {
        return Err(format!(
            "quota reconnect first real request used {:?}, expected {}",
            transcript.quota_error_connection_label, QUOTA_RECONNECT_PRIMARY.label
        ));
    }
    if transcript.completion_connection_label.as_deref() != Some(QUOTA_RECONNECT_FALLBACK.label) {
        return Err(format!(
            "quota reconnect completion used {:?}, expected {}",
            transcript.completion_connection_label, QUOTA_RECONNECT_FALLBACK.label
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouterAuditObservation {
    http_sse_local_auth_validated: bool,
    websocket_local_auth_validated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouterProcessObservation {
    binary_path: PathBuf,
    pid: u32,
    argv: Vec<String>,
    listener: String,
    readiness_line: String,
    cleanup_result: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct RouterWebSocketRegistryReport {
    handled_connections: Option<usize>,
    active_sessions: usize,
    high_water_sessions: usize,
    registered_sessions: usize,
    closed_sessions: usize,
    completed_response_sessions: usize,
    forwarded_upstream_messages: usize,
    registered_session_id_count: usize,
    completed_session_id_count: usize,
    closed_session_id_count: usize,
    session_peer_addr_count: usize,
    session_peer_join_observable: bool,
    completed_session_forwarded_upstream_message_counts: Vec<usize>,
    final_session_forwarded_upstream_message_counts: Vec<usize>,
    #[serde(default)]
    quota_reconnect_signal_count: usize,
    quota_reconnect_signal_unix_ms: Option<u128>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
struct RouterWebSocketRegistryReportFile {
    schema_version: usize,
    handled_connections: Option<usize>,
    websocket_registry: RouterWebSocketRegistryReport,
}

impl RouterWebSocketRegistryReport {
    fn from_file(path: &Path) -> Result<Self, String> {
        let contents = fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read router websocket registry report {}: {error}",
                path.display()
            )
        })?;
        let report = serde_json::from_str::<RouterWebSocketRegistryReportFile>(&contents).map_err(
            |error| {
                format!(
                    "router websocket registry report {} was invalid JSON: {error}",
                    path.display()
                )
            },
        )?;
        if report.schema_version != 2 {
            return Err(format!(
                "router websocket registry report schema_version={}, expected 2",
                report.schema_version
            ));
        }
        let mut registry = report.websocket_registry;
        registry.handled_connections = report.handled_connections;
        Ok(registry)
    }
}

impl RouterAuditObservation {
    fn from_file(path: &Path) -> Result<Self, String> {
        let audit_contents = fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read router audit file {}: {error}",
                path.display()
            )
        })?;
        let mut observation = Self {
            http_sse_local_auth_validated: false,
            websocket_local_auth_validated: false,
        };
        for line in audit_contents
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            let value = serde_json::from_str::<Value>(line)
                .map_err(|error| format!("router audit event was invalid JSON: {error}"))?;
            if value.get("local_auth_result").and_then(Value::as_str) != Some("valid")
                || value.get("outcome").and_then(Value::as_str) != Some("allowed")
            {
                continue;
            }
            match value.get("transport_kind").and_then(Value::as_str) {
                Some("http") => observation.http_sse_local_auth_validated = true,
                Some("web_socket") => observation.websocket_local_auth_validated = true,
                _ => {}
            }
        }

        Ok(observation)
    }

    fn require_mode(&self, mode: InstalledCodexSmokeMode) -> Result<(), String> {
        if mode.requires_http_sse() && !self.http_sse_local_auth_validated {
            return Err("router audit did not record valid allowed HTTP/SSE local auth".to_owned());
        }
        if mode.requires_websocket() && !self.websocket_local_auth_validated {
            return Err(
                "router audit did not record valid allowed WebSocket local auth".to_owned(),
            );
        }
        Ok(())
    }
}

struct RedactedTranscriptInput<'a> {
    mode: InstalledCodexSmokeMode,
    codex_version: &'a str,
    profile_path: &'a Path,
    http_sse_codex_status: Option<&'a ExitStatus>,
    http_sse_codex_stdout: Option<Cow<'a, str>>,
    http_sse_codex_stderr: Option<Cow<'a, str>>,
    http_sse_last_message_path: Option<&'a Path>,
    websocket_codex_status: Option<&'a ExitStatus>,
    websocket_codex_stdout: Option<Cow<'a, str>>,
    websocket_codex_stderr: Option<Cow<'a, str>>,
    websocket_last_message_path: Option<&'a Path>,
    upstream: &'a MockWebSocketTranscript,
    quota_status: &'a SmokeQuotaStatus,
    expected_account_label: &'a str,
    expected_upstream_token: &'a str,
    router_process: &'a RouterProcessObservation,
    router_audit: &'a RouterAuditObservation,
}

struct QuotaReconnectTranscriptInput<'a> {
    codex_version: &'a str,
    profile_path: &'a Path,
    codex_status: &'a ExitStatus,
    codex_stdout: &'a str,
    codex_stderr: &'a str,
    last_message_path: &'a Path,
    upstream: &'a QuotaReconnectWebSocketTranscript,
    router_process: &'a RouterProcessObservation,
    router_audit: &'a RouterAuditObservation,
    registry_report: &'a RouterWebSocketRegistryReport,
    runtime_roots: &'a InstalledCodexRuntimeRoots,
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
    let selected_account = selected_account_from_status_json(&input.quota_status.json)?;
    let selected_account_tag = selected_account.account_hash;
    let selected_account_label = selected_account.safe_label;
    let router_binary_path = sanitized_artifact_path(&input.router_process.binary_path)?;
    let router_argv = sanitized_router_argv(&input.router_process.argv);
    let redacted = serde_json::json!({
        "mode": input.mode.as_str(),
        "codex_version": input.codex_version,
        "profile_written": input.profile_path.exists(),
        "profile_env_key": null,
        "profile_uses_codex_router_token": false,
        "router_process": {
            "binary_path": router_binary_path,
            "pid": input.router_process.pid,
            "argv": router_argv,
            "listener": sanitized_loopback_endpoint_text(&input.router_process.listener),
            "readiness_line": sanitized_loopback_endpoint_text(&input.router_process.readiness_line),
            "cleanup_result": sanitized_loopback_endpoint_text(&input.router_process.cleanup_result),
            "spawned_real_serve_child": true,
        },
        "http_sse_codex_status": input.http_sse_codex_status.map(ToString::to_string),
        "http_sse_codex_stdout_contains_smoke_text": input.http_sse_codex_stdout.as_deref().map(|stdout| stdout.contains("codex-router smoke ok")),
        "http_sse_codex_stderr_line_count": input.http_sse_codex_stderr.as_deref().map(str::lines).map(Iterator::count),
        "http_sse_last_message_written": input.http_sse_last_message_path.is_some_and(Path::exists),
        "websocket_codex_status": input.websocket_codex_status.map(ToString::to_string),
        "websocket_codex_stdout_contains_smoke_text": input.websocket_codex_stdout.as_deref().map(|stdout| stdout.contains("codex-router smoke ok")),
        "websocket_codex_stderr_line_count": input.websocket_codex_stderr.as_deref().map(str::lines).map(Iterator::count),
        "websocket_last_message_written": input.websocket_last_message_path.is_some_and(Path::exists),
        "expected_account_label": input.expected_account_label,
        "selected_account": {
            "safe_label": selected_account_label,
            "safe_tag": selected_account_tag,
            "routing_reason": "preferred_next",
        },
        "quota_status": {
            "table_contains_expected_account": input.quota_status.table.contains(input.expected_account_label),
            "plain_contains_expected_account": input.quota_status.plain.contains(input.expected_account_label),
            "plain_marks_next": input.quota_status.plain.contains("\tnext"),
            "json_selected_account_label": selected_account_label,
            "selected_account_tag": selected_account_tag,
        },
        "router_completed": true,
        "http_sse": {
            "ran": input.mode.requires_http_sse(),
            "local_auth_carrier": input.mode.requires_http_sse().then_some("authorization_bearer"),
            "local_auth_validated": input.mode.requires_http_sse().then_some(input.router_audit.http_sse_local_auth_validated),
            "local_auth_audit_observed": input.mode.requires_http_sse().then_some(input.router_audit.http_sse_local_auth_validated),
            "local_auth_stripped_before_upstream": input.mode.requires_http_sse().then_some(input.upstream.http_sse.as_ref().and_then(|request| request.header("x-codex-router-token")).is_none()),
            "upstream_auth_redacted_present": input.upstream.http_sse.as_ref().and_then(|request| request.header("authorization")).map(|_| true),
            "selected_expected_account": input.upstream.http_sse.as_ref().and_then(|request| authorization_header_matches_expected(request.header("authorization"), input.expected_upstream_token)),
            "actual_safe_label": input.upstream.http_sse.as_ref().and_then(|request| upstream_label_from_authorization_header(request.header("authorization"))),
            "request_line": input.upstream.http_sse.as_ref().map(|request| request.request_line.as_str()),
            "stream_requested": input.upstream.http_sse.as_ref().map(http_sse_request_asks_streaming),
            "local_router_token_in_body": false,
        },
        "websocket": {
            "ran": input.mode.requires_websocket(),
            "local_auth_carrier": input.mode.requires_websocket().then_some("authorization_bearer"),
            "local_auth_validated": input.mode.requires_websocket().then_some(input.router_audit.websocket_local_auth_validated),
            "local_auth_audit_observed": input.mode.requires_websocket().then_some(input.router_audit.websocket_local_auth_validated),
            "local_auth_stripped_before_upstream": input.mode.requires_websocket().then_some(input.upstream.header("x-codex-router-token").is_none()),
            "upstream_auth_redacted_present": input.upstream.header("authorization").map(|_| true),
            "selected_expected_account": authorization_header_matches_expected(input.upstream.header("authorization"), input.expected_upstream_token),
            "actual_safe_label": upstream_label_from_authorization_header(input.upstream.header("authorization")),
            "local_router_token_in_first_frame": false,
            "request_frame_count": input.upstream.websocket_request_frame_count,
            "non_prewarm_request_frame_count": input.upstream.request_frames.iter().filter_map(|frame| serde_json::from_str::<Value>(frame).ok()).filter(is_non_prewarm_response_create_frame).count(),
            "first_frame_shape": first_frame_shape_summary(&first_frame),
            "routed_response_create_shape": response_create_frame_shape_summary(input.upstream),
        },
        "upstream": {
            "handshake_count": input.upstream.websocket_handshake_count(),
            "http_probe_count": input.upstream.http_probe_count,
        }
    });
    let payload = serde_json::to_string_pretty(&redacted)
        .map_err(|error| format!("failed to render redacted smoke transcript: {error}"))?;
    assert_redacted_transcript_payload(&payload, input)?;
    fs::write(&transcript_path, payload)
        .map_err(|error| format!("failed to write smoke transcript: {error}"))?;

    Ok(transcript_path)
}

fn write_redacted_quota_reconnect_transcript(
    input: &QuotaReconnectTranscriptInput<'_>,
) -> Result<PathBuf, String> {
    let artifact_dir = workspace_root()?.join("tmp").join("smoke");
    fs::create_dir_all(&artifact_dir).map_err(|error| {
        format!(
            "failed to create smoke artifact dir {}: {error}",
            artifact_dir.display()
        )
    })?;
    let transcript_path = artifact_dir.join(format!(
        "installed-codex-quota-reconnect-{}-{}.json",
        std::process::id(),
        timestamp_millis()
    ));
    let router_root = sanitized_artifact_path(&input.runtime_roots.router_root)?;
    let router_db = sanitized_artifact_path(&input.runtime_roots.state_path)?;
    let codex_home = input
        .runtime_roots
        .codex_home
        .as_deref()
        .map(sanitized_artifact_path)
        .transpose()?;
    let codex_db = input
        .runtime_roots
        .codex_home
        .as_deref()
        .map(|path| sanitized_artifact_path(&path.join("state_5.sqlite")))
        .transpose()?;
    let process_home = input
        .runtime_roots
        .process_home
        .as_deref()
        .map(sanitized_artifact_path)
        .transpose()?;
    let runtime_roots = serde_json::json!({
        "mode": input.runtime_roots.mode.as_str(),
        "router_root": router_root,
        "router_db": router_db,
        "codex_home": codex_home,
        "codex_db": codex_db,
        "process_home": process_home,
    });
    let sqlite_pressure = input.upstream.sqlite_pressure.as_ref();
    let sqlite_lock_or_maintenance_pressure = sqlite_pressure.is_some_and(|pressure| {
        pressure.acquired_before_quota_error && pressure.released_after_completion
    });
    let reconnected_to_different_account =
        input.upstream.quota_error_connection_label != input.upstream.completion_connection_label;
    let pressure = serde_json::json!({
        "pressure_mechanism": sqlite_pressure.map_or("none", |pressure| pressure.mechanism),
        "copied_db_pressure_proven": input.runtime_roots.mode == "copied-dev-state"
            && sqlite_lock_or_maintenance_pressure,
        "sqlite_lock_or_maintenance_pressure": sqlite_lock_or_maintenance_pressure,
        "provider_error_observer_delay": false,
        "sqlite_pressure": sqlite_pressure.map(|pressure| serde_json::json!({
            "mechanism": pressure.mechanism,
            "hold_duration_ms": pressure.hold_duration_ms,
            "acquired_before_quota_error": pressure.acquired_before_quota_error,
            "released_after_completion": pressure.released_after_completion,
            "acquired_unix_ms": pressure.acquired_unix_ms,
            "released_unix_ms": pressure.released_unix_ms,
        })),
    });
    let signal_ordering = serde_json::json!({
        "signal_before_persistence": sqlite_lock_or_maintenance_pressure
            && input.upstream.completion_sent,
        "basis": "fallback completion was sent before copied SQLite write-lock pressure released",
    });
    let account_selection = serde_json::json!({
        "non_reselection": reconnected_to_different_account,
        "basis": "quota error account role differs from fallback completion account role",
    });
    let router_signal_latency_ms = input
        .upstream
        .quota_error_sent_unix_ms
        .zip(input.registry_report.quota_reconnect_signal_unix_ms)
        .map(|(quota_error, router_signal)| router_signal.saturating_sub(quota_error));
    let payload = serde_json::json!({
        "git_head": current_git_head()?,
        "mode": "quota-reconnect-websocket",
        "s8_provenance": s8_smoke_provenance("quota-reconnect"),
        "codex_version": input.codex_version,
        "runtime_roots": runtime_roots,
        "pressure": pressure,
        "signal_ordering": signal_ordering,
        "account_selection": account_selection,
        "profile_written": input.profile_path.exists(),
        "profile_uses_codex_router_token": false,
        "router_process": {
            "binary_path": sanitized_artifact_path(&input.router_process.binary_path)?,
            "pid": input.router_process.pid,
            "argv": sanitized_router_argv(&input.router_process.argv),
            "listener": sanitized_loopback_endpoint_text(&input.router_process.listener),
            "readiness_line": sanitized_loopback_endpoint_text(&input.router_process.readiness_line),
            "cleanup_result": sanitized_loopback_endpoint_text(&input.router_process.cleanup_result),
            "spawned_real_serve_child": true,
        },
        "codex": {
            "status": input.codex_status.to_string(),
            "stdout_contains_smoke_text": input.codex_stdout.contains(SMOKE_EXPECTED_TEXT),
            "stderr_line_count": input.codex_stderr.lines().count(),
            "stdout_contains_usage_limit_reached": input.codex_stdout.contains("usage_limit_reached"),
            "stderr_contains_usage_limit_reached": input.codex_stderr.contains("usage_limit_reached"),
            "last_message_written": input.last_message_path.exists(),
        },
        "router_audit": {
            "websocket_local_auth_validated": input.router_audit.websocket_local_auth_validated,
        },
        "quota_reconnect": {
            "primary_account_role": "primary",
            "fallback_account_role": "fallback",
            "quota_error_hidden_from_codex": !input.codex_stdout.contains("usage_limit_reached")
                && !input.codex_stderr.contains("usage_limit_reached"),
            "first_real_request_account_role": quota_reconnect_role_from_label(
                input.upstream.quota_error_connection_label.as_deref(),
            ),
            "completion_account_role": quota_reconnect_role_from_label(
                input.upstream.completion_connection_label.as_deref(),
            ),
            "reconnected_to_different_account": reconnected_to_different_account,
        },
        "quota_reconnect_progress": {
            "signal_latency_ms": router_signal_latency_ms,
            "basis": "router quota_reconnect signal timestamp minus upstream quota exhaustion timestamp",
            "router_signal_count": input.registry_report.quota_reconnect_signal_count,
        },
        "upstream": {
            "http_probe_count": input.upstream.http_probe_count,
            "websocket_handshake_count": input.upstream.websocket_handshake_count,
            "request_frame_count": input.upstream.request_frame_count,
            "prewarm_frame_count": input.upstream.prewarm_frame_count,
            "non_prewarm_frame_count": input.upstream.non_prewarm_frame_count,
            "quota_error_sent": input.upstream.quota_error_sent,
            "completion_sent": input.upstream.completion_sent,
        },
    });
    let rendered = serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("failed to render quota reconnect transcript: {error}"))?;
    assert_redacted_quota_reconnect_payload(&rendered, input)?;
    fs::write(&transcript_path, rendered)
        .map_err(|error| format!("failed to write quota reconnect transcript: {error}"))?;
    Ok(transcript_path)
}

struct ThreeWebSocketTranscriptInput<'a> {
    mode: &'a str,
    codex_version: &'a str,
    router_process: &'a RouterProcessObservation,
    registry_report: Option<&'a RouterWebSocketRegistryReport>,
    upstream: &'a ConcurrentWebSocketTranscript,
    socket_cleanup: &'a RouterSocketCleanupObservation,
    outputs: &'a [CodexChildRun],
    seed: &'a SmokeSeed,
    runtime_roots: &'a InstalledCodexRuntimeRoots,
}

fn write_redacted_three_websocket_transcript(
    input: &ThreeWebSocketTranscriptInput<'_>,
) -> Result<PathBuf, String> {
    let artifact_dir = workspace_root()?.join("tmp").join("smoke");
    fs::create_dir_all(&artifact_dir).map_err(|error| {
        format!(
            "failed to create smoke artifact dir {}: {error}",
            artifact_dir.display()
        )
    })?;
    let transcript_path = artifact_dir.join(format!(
        "installed-codex-three-websocket-{}-{}.json",
        std::process::id(),
        timestamp_millis()
    ));
    let statuses = input
        .outputs
        .iter()
        .map(|run| {
            serde_json::json!({
                "pid": run.pid,
                "status": run.output.status.to_string(),
                "stdout_contains_smoke_text": String::from_utf8_lossy(&run.output.stdout).contains(SMOKE_EXPECTED_TEXT),
                "stderr_line_count": String::from_utf8_lossy(&run.output.stderr).lines().count(),
                "stderr_transport_error_markers": stderr_transport_error_markers(&String::from_utf8_lossy(&run.output.stderr)),
            })
        })
        .collect::<Vec<_>>();
    let router_binary_path = sanitized_artifact_path(&input.router_process.binary_path)?;
    let router_argv = sanitized_router_argv(&input.router_process.argv);
    let router_root = sanitized_artifact_path(&input.runtime_roots.router_root)?;
    let router_db = sanitized_artifact_path(&input.runtime_roots.state_path)?;
    let codex_home = input
        .runtime_roots
        .codex_home
        .as_deref()
        .map(sanitized_artifact_path)
        .transpose()?;
    let codex_db = input
        .runtime_roots
        .codex_home
        .as_deref()
        .map(|path| sanitized_artifact_path(&path.join("state_5.sqlite")))
        .transpose()?;
    let process_home = input
        .runtime_roots
        .process_home
        .as_deref()
        .map(sanitized_artifact_path)
        .transpose()?;
    let runtime_roots = serde_json::json!({
        "mode": input.runtime_roots.mode.as_str(),
        "router_root": router_root,
        "router_db": router_db,
        "codex_home": codex_home,
        "codex_db": codex_db,
        "process_home": process_home,
    });
    let sqlite_pressure = input.upstream.sqlite_pressure.as_ref();
    let sqlite_lock_or_maintenance_pressure = sqlite_pressure.is_some_and(|pressure| {
        pressure.acquired_before_quota_error && pressure.released_after_completion
    });
    let reconnected_to_different_account = input.upstream.quota_error_connection_label
        != input.upstream.completion_connection_label
        && input.upstream.quota_error_connection_label.is_some()
        && input.upstream.completion_connection_label.is_some();
    let pressure = if input.upstream.quota_error_sent {
        serde_json::json!({
            "pressure_mechanism": sqlite_pressure.map_or("none", |pressure| pressure.mechanism),
            "copied_db_pressure_proven": input.runtime_roots.mode == "copied-dev-state"
                && sqlite_lock_or_maintenance_pressure,
            "sqlite_lock_or_maintenance_pressure": sqlite_lock_or_maintenance_pressure,
            "provider_error_observer_delay": false,
            "sqlite_pressure": sqlite_pressure.map(|pressure| serde_json::json!({
                "mechanism": pressure.mechanism,
                "hold_duration_ms": pressure.hold_duration_ms,
                "acquired_before_quota_error": pressure.acquired_before_quota_error,
                "released_after_completion": pressure.released_after_completion,
                "acquired_unix_ms": pressure.acquired_unix_ms,
                "released_unix_ms": pressure.released_unix_ms,
            })),
        })
    } else {
        serde_json::json!({
            "pressure_mechanism": "blocked-missing-sqlite-pressure",
            "copied_db_pressure_proven": false,
            "sqlite_lock_or_maintenance_pressure": false,
            "provider_error_observer_delay": false,
            "blocked_reason": "copied roots were exercised, but this harness has no approved SQLite lock/maintenance pressure or forced provider-error observer delay",
        })
    };
    let signal_ordering = serde_json::json!({
        "signal_before_persistence": if input.upstream.quota_error_sent {
            sqlite_lock_or_maintenance_pressure && input.upstream.completion_sent
        } else {
            input.upstream.multi_step_completed_before_overlap_end
        },
        "basis": if input.upstream.quota_error_sent {
            "fallback completion was sent before copied SQLite write-lock pressure released"
        } else {
            "multi-step follow-up completed before overlap end; durable provider-error persistence pressure is not present in this harness"
        },
    });
    let account_selection = serde_json::json!({
        "non_reselection": if input.upstream.quota_error_sent {
            reconnected_to_different_account
        } else {
            input.upstream.upstream_client_sessions.len() >= input.outputs.len()
        },
        "basis": if input.upstream.quota_error_sent {
            "quota error account role differs from fallback completion account role"
        } else {
            "all expected upstream client sessions completed; exhausted-account scenario is not present in this harness"
        },
    });
    let runtime_correlations = runtime_correlations_for_three_websocket(input);
    let session_continuity = session_continuity_for_three_websocket(input);
    let router_process = serde_json::json!({
        "binary_path": router_binary_path,
        "pid": input.router_process.pid,
        "argv": router_argv,
        "listener": sanitized_loopback_endpoint_text(&input.router_process.listener),
        "readiness_line": sanitized_loopback_endpoint_text(&input.router_process.readiness_line),
        "cleanup_result": sanitized_loopback_endpoint_text(&input.router_process.cleanup_result),
        "spawned_real_serve_child": true,
    });
    let router_websocket_registry = input.registry_report.map(|report| serde_json::json!({
        "handled_connections": report.handled_connections,
        "active_sessions": report.active_sessions,
        "high_water_sessions": report.high_water_sessions,
        "registered_sessions": report.registered_sessions,
        "closed_sessions": report.closed_sessions,
        "completed_response_sessions": report.completed_response_sessions,
        "forwarded_upstream_messages": report.forwarded_upstream_messages,
        "registered_session_id_count": report.registered_session_id_count,
        "completed_session_id_count": report.completed_session_id_count,
        "closed_session_id_count": report.closed_session_id_count,
        "session_peer_addr_count": report.session_peer_addr_count,
        "session_peer_join_observable": report.session_peer_join_observable,
        "completed_session_forwarded_upstream_message_counts": report.completed_session_forwarded_upstream_message_counts,
        "final_session_forwarded_upstream_message_counts": report.final_session_forwarded_upstream_message_counts,
        "quota_reconnect_signal_count": report.quota_reconnect_signal_count,
        "quota_reconnect_signal_unix_ms": report.quota_reconnect_signal_unix_ms,
    }));
    let clients = serde_json::json!({
        "count": input.outputs.len(),
        "target_model": SMOKE_TARGET_MODEL,
        "all_success": input.outputs.iter().all(|run| run.output.status.success()),
        "statuses": statuses,
    });
    let selected_account = serde_json::json!({
        "safe_tag": input.seed.expected_account_tag,
        "expected_upstream_account_selected": input.upstream.upstream_client_sessions.len() >= input.outputs.len(),
    });
    let upstream = serde_json::json!({
        "expected_sessions": input.upstream.expected_sessions,
        "expected_upstream_sessions": input.upstream.expected_upstream_sessions,
        "completed_sessions": input.upstream.completed_sessions,
        "final_active_sessions": input.upstream.final_active_sessions,
        "active_high_water": input.upstream.active_high_water,
        "overlap_proven": input.upstream.active_high_water >= input.upstream.expected_sessions,
        "overlap_started_unix_ms": input.upstream.overlap_started_unix_ms,
        "overlap_completed_unix_ms": input.upstream.overlap_completed_unix_ms,
        "real_overlap_completed_unix_ms": input.upstream.real_overlap_completed_unix_ms,
        "overlap_duration_ms": input.upstream.overlap_duration_ms,
        "real_overlap_duration_ms": input.upstream.real_overlap_duration_ms,
        "hold_duration_ms": input.upstream.hold_duration.as_millis(),
        "non_prewarm_session_count": input.upstream.non_prewarm_session_count,
        "target_model": SMOKE_TARGET_MODEL,
        "target_model_session_count": input.upstream.target_model_session_count,
        "unexpected_response_create_models": input.upstream.unexpected_response_create_models,
        "upstream_session_id_count": input.upstream.upstream_session_ids.len(),
        "upstream_client_session_count": input.upstream.upstream_client_sessions.len(),
        "upstream_client_indexes": input.upstream.upstream_client_sessions.iter().map(|session| session.client_index).collect::<Vec<_>>(),
        "session_frame_counts": input.upstream.session_frame_counts,
        "session_event_counts": input.upstream.session_event_counts,
        "in_overlap_session_event_counts": input.upstream.in_overlap_session_event_counts,
        "http_probe_count": input.upstream.http_probe_count,
        "normal_close_sessions": input.upstream.normal_close_sessions,
        "abnormal_close_sessions": input.upstream.abnormal_close_sessions,
        "session_close_outcomes": input.upstream.session_close_outcomes,
        "multi_step_interleave_completed": input.upstream.multi_step_interleave_completed,
        "multi_step_followup_frame_count": input.upstream.multi_step_followup_frame_count,
        "multi_step_followup_active_session_count": input.upstream.multi_step_followup_active_session_count,
        "multi_step_followup_unix_ms": input.upstream.multi_step_followup_unix_ms,
        "multi_step_completed_before_overlap_end": input.upstream.multi_step_completed_before_overlap_end,
    });
    let quota_reconnect_progress = serde_json::json!({
        "signal_latency_ms": input.upstream.quota_error_sent_unix_ms.zip(input.registry_report.and_then(|report| report.quota_reconnect_signal_unix_ms)).map(|(quota_error, router_signal)| router_signal.saturating_sub(quota_error)),
        "basis": "router quota_reconnect signal timestamp minus upstream quota exhaustion timestamp",
        "router_signal_count": input.registry_report.map_or(0, |report| report.quota_reconnect_signal_count),
    });
    let source_artifact = serde_json::json!({
        "artifact": transcript_path.file_name().and_then(|name| name.to_str()),
        "s8_run_id": s8_smoke_run_id(),
        "git_head": current_git_head()?,
        "runtime_roots": runtime_roots,
        "mode": input.mode,
    });
    let socket_cleanup = serde_json::json!({
        "lsof_exit_status": input.socket_cleanup.lsof_exit_status,
        "tcp_line_count": input.socket_cleanup.tcp_line_count,
        "established_count": input.socket_cleanup.established_count,
        "close_wait_count": input.socket_cleanup.close_wait_count,
        "raw_state_counts": input.socket_cleanup.raw_state_counts,
    });
    let payload = serde_json::json!({
        "git_head": current_git_head()?,
        "mode": input.mode,
        "s8_provenance": s8_smoke_provenance(input.mode),
        "codex_version": input.codex_version.trim(),
        "runtime_roots": runtime_roots,
        "pressure": pressure,
        "signal_ordering": signal_ordering,
        "account_selection": account_selection,
        "quota_reconnect": {
            "primary_account_role": "primary",
            "fallback_account_role": "fallback",
            "first_real_request_account_role": quota_reconnect_role_from_label(
                input.upstream.quota_error_connection_label.as_deref(),
            ),
            "completion_account_role": quota_reconnect_role_from_label(
                input.upstream.completion_connection_label.as_deref(),
            ),
            "reconnected_to_different_account": reconnected_to_different_account,
        },
        "quota_reconnect_progress": quota_reconnect_progress,
        "source_artifacts": {
            "three_websocket_soak": source_artifact,
            "quota_reconnect": source_artifact,
        },
        "router_process": router_process,
        "router_websocket_registry": router_websocket_registry,
        "clients": clients,
        "selected_account": selected_account,
        "runtime_correlations": runtime_correlations,
        "session_continuity": session_continuity,
        "upstream": upstream,
        "socket_cleanup": socket_cleanup,
        "shared_router_pid": input.router_process.pid,
    });
    let rendered = serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("failed to render three-client transcript: {error}"))?;
    assert_redacted_three_websocket_payload(&rendered, input.outputs, input.seed)?;
    fs::write(&transcript_path, rendered)
        .map_err(|error| format!("failed to write three-client transcript: {error}"))?;
    Ok(transcript_path)
}

fn s8_smoke_provenance(scenario: &str) -> serde_json::Value {
    serde_json::json!({
        "run_id": s8_smoke_run_id(),
        "scenario": scenario,
    })
}

fn s8_smoke_run_id() -> Option<String> {
    std::env::var(S8_RUN_ID_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn runtime_correlations_for_three_websocket(
    input: &ThreeWebSocketTranscriptInput<'_>,
) -> Vec<Value> {
    let upstream_session_by_client = upstream_session_by_client_index(input.upstream);
    input
        .outputs
        .iter()
        .enumerate()
        .map(|(index, run)| {
            let stderr = String::from_utf8_lossy(&run.output.stderr);
            let markers = stderr_transport_error_markers(&stderr);
            serde_json::json!({
                "client_index": index,
                "client_pid": run.pid,
                "router_pid": input.router_process.pid,
                "router_session_observed": input.registry_report.is_some_and(|report| {
                    report.high_water_sessions >= input.outputs.len()
                        && report.registered_sessions >= input.outputs.len()
                }),
                "upstream_session_observed": upstream_session_by_client.contains_key(&index),
                "transport": if markers.is_empty() { "websocket" } else { "websocket_error" },
                "stderr_transport_error_markers": markers,
                "stdout_contains_smoke_text": String::from_utf8_lossy(&run.output.stdout).contains(SMOKE_EXPECTED_TEXT),
            })
        })
        .collect()
}

fn session_continuity_for_three_websocket(input: &ThreeWebSocketTranscriptInput<'_>) -> Value {
    let upstream_session_by_client = upstream_session_by_client_index(input.upstream);
    let router_registry_observed = input.registry_report.is_some_and(|report| {
        report.high_water_sessions >= input.outputs.len()
            && report.registered_sessions >= input.outputs.len()
            && report.closed_sessions >= input.outputs.len()
    });
    let per_client_join_observations = input
        .outputs
        .iter()
        .enumerate()
        .map(|(client_index, run)| {
            serde_json::json!({
                "client_index": client_index,
                "client_pid": run.pid,
                "router_session_observed": router_registry_observed,
                "upstream_session_observed": upstream_session_by_client.contains_key(&client_index),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "per_client_session_join_key_observed": false,
        "correlation_level": if router_registry_observed {
            "router_registry_counts_and_upstream_marker_join"
        } else {
            "upstream_marker_join"
        },
        "per_client_join_observations": per_client_join_observations,
        "router_registered_unique_session_count": input
            .registry_report
            .map_or(0, |report| report.registered_session_id_count),
        "router_closed_unique_session_count": input
            .registry_report
            .map_or(0, |report| report.closed_session_id_count),
        "upstream_unique_session_count": unique_u64_count(&input.upstream.upstream_session_ids),
        "router_pid": input.router_process.pid,
        "shared_router_pid": input.router_process.pid,
    })
}

fn upstream_session_by_client_index(
    upstream: &ConcurrentWebSocketTranscript,
) -> BTreeMap<usize, u64> {
    upstream
        .upstream_client_sessions
        .iter()
        .map(|session| (session.client_index, session.upstream_session_id))
        .collect()
}

fn unique_u64_count(values: &[u64]) -> usize {
    values.iter().copied().collect::<BTreeSet<_>>().len()
}

fn stderr_transport_error_markers(stderr: &str) -> Vec<&'static str> {
    let stderr = stderr.to_ascii_lowercase();
    [
        ("fallback", "fallback"),
        ("reconnect", "reconnect"),
        ("websocket protocol error", "websocket_protocol_error"),
        ("handshake not finished", "handshake_not_finished"),
        (
            "stream disconnected before completion",
            "stream_disconnected_before_completion",
        ),
        ("closed connection", "closed_connection"),
        ("connection closed", "closed_connection"),
        ("waiting on loopback", "waiting_on_loopback"),
        ("function_call_output", "function_call_output"),
        ("shell_command", "shell_command"),
        ("tool-call", "tool_call"),
        ("request timed out", "request_timed_out"),
    ]
    .into_iter()
    .filter_map(|(needle, marker)| stderr.contains(needle).then_some(marker))
    .collect()
}

fn sanitized_artifact_path(path: &Path) -> Result<String, String> {
    let workspace = workspace_root()?;
    if let Ok(relative) = path.strip_prefix(&workspace) {
        return Ok(format!("<repo>/{}", relative.display()));
    }
    Ok(path.file_name().and_then(|name| name.to_str()).map_or_else(
        || "<external-path>".to_owned(),
        |name| format!("<external>/{name}"),
    ))
}

fn sanitized_router_argv(argv: &[String]) -> Vec<String> {
    let path_value_flags = [
        "--port",
        "--state-db",
        "--secret-root",
        "--upstream-base-url",
        "--audit-file",
        "--websocket-registry-report-file",
    ];
    let mut sanitized = Vec::with_capacity(argv.len());
    let mut redact_next_value: Option<&str> = None;
    for value in argv {
        if let Some(flag) = redact_next_value.take() {
            sanitized.push(format!("<{flag}-path>"));
            continue;
        }
        sanitized.push(value.clone());
        if path_value_flags.contains(&value.as_str()) {
            redact_next_value = Some(value.trim_start_matches("--"));
        }
    }
    sanitized
}

fn sanitized_loopback_endpoint_text(value: &str) -> String {
    let mut sanitized = value.to_owned();
    for prefix in ["127.0.0.1:", "localhost:"] {
        sanitized = replace_port_after_prefix(&sanitized, prefix);
    }
    sanitized
}

fn replace_port_after_prefix(value: &str, prefix: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(index) = remaining.find(prefix) {
        let (before, after_before) = remaining.split_at(index);
        output.push_str(before);
        output.push_str(prefix);
        output.push_str("<port>");
        let Some(after_prefix) = after_before.strip_prefix(prefix) else {
            output.push_str(after_before);
            remaining = "";
            break;
        };
        let first_non_digit = after_prefix
            .char_indices()
            .find_map(|(offset, character)| (!character.is_ascii_digit()).then_some(offset))
            .unwrap_or(after_prefix.len());
        remaining = after_prefix.get(first_non_digit..).unwrap_or_default();
    }
    output.push_str(remaining);
    output
}

fn current_git_head() -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace_root()?)
        .output()
        .map_err(|error| format!("failed to run git rev-parse HEAD: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn observe_router_socket_cleanup(pid: u32) -> Result<RouterSocketCleanupObservation, String> {
    let output = Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid.to_string(), "-iTCP"])
        .output()
        .map_err(|error| format!("failed to run lsof for router socket cleanup: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut state_counts = BTreeMap::<String, usize>::new();
    let mut tcp_line_count = 0_usize;
    for line in stdout.lines().skip(1) {
        if !line.contains("TCP") {
            continue;
        }
        tcp_line_count = tcp_line_count.saturating_add(1);
        let state = line
            .rsplit_once('(')
            .and_then(|(_prefix, suffix)| suffix.strip_suffix(')'))
            .unwrap_or("UNKNOWN")
            .to_owned();
        *state_counts.entry(state).or_default() += 1;
    }
    Ok(RouterSocketCleanupObservation {
        lsof_exit_status: output.status.to_string(),
        tcp_line_count,
        established_count: state_counts.get("ESTABLISHED").copied().unwrap_or_default(),
        close_wait_count: state_counts.get("CLOSE_WAIT").copied().unwrap_or_default(),
        raw_state_counts: state_counts.into_iter().collect(),
    })
}

impl RouterSocketCleanupObservation {
    fn assert_no_leaked_sessions(&self) -> Result<(), String> {
        if self.established_count != 0 || self.close_wait_count != 0 {
            return Err(format!(
                "router socket cleanup found established_count={} close_wait_count={} state_counts={:?}",
                self.established_count, self.close_wait_count, self.raw_state_counts
            ));
        }
        Ok(())
    }
}

fn assert_redacted_three_websocket_payload(
    payload: &str,
    outputs: &[CodexChildRun],
    seed: &SmokeSeed,
) -> Result<(), String> {
    let forbidden_fragments = [
        Some(seed.local_token.as_str()),
        Some(seed.expected_upstream_token.as_str()),
        Some(seed.local_token_assignment.as_str()),
        Some(seed.expected_account_label.as_str()),
        Some("installed-smoke-matches-token"),
        Some("prompt-canary"),
        Some("raw-previous-response-id-canary"),
        Some("/Users/"),
        Some("/var/folders/"),
    ];
    for forbidden in forbidden_fragments
        .into_iter()
        .flatten()
        .filter(|fragment| !fragment.is_empty())
    {
        if payload.contains(forbidden) {
            return Err(format!(
                "three-client transcript leaked forbidden fragment: {forbidden}"
            ));
        }
    }
    if contains_loopback_endpoint_with_numeric_port(payload) {
        return Err(
            "three-client transcript leaked loopback endpoint with numeric port".to_owned(),
        );
    }
    let forbidden_structural_keys = [
        "registered_session_ids",
        "completed_session_ids",
        "closed_session_ids",
        "session_peer_addrs",
        "session_id",
        "local_port",
        "router_session_id",
        "upstream_session_id",
        "per_client_join_keys",
        "router_registered_session_ids",
        "router_closed_session_ids",
        "upstream_session_ids",
        "upstream_client_sessions",
    ];
    for forbidden_key in forbidden_structural_keys {
        let quoted_key = format!("\"{forbidden_key}\"");
        if payload.contains(&quoted_key) {
            return Err(format!(
                "three-client transcript leaked forbidden structural key: {forbidden_key}"
            ));
        }
    }
    for run in outputs {
        let stdout = String::from_utf8_lossy(&run.output.stdout);
        let stderr = String::from_utf8_lossy(&run.output.stderr);
        for forbidden in [stdout.as_ref(), stderr.as_ref()]
            .into_iter()
            .filter(|fragment| !fragment.is_empty())
        {
            if payload.contains(forbidden) {
                return Err("three-client transcript leaked captured child output".to_owned());
            }
        }
    }
    Ok(())
}

fn contains_loopback_endpoint_with_numeric_port(payload: &str) -> bool {
    ["127.0.0.1:", "localhost:"]
        .into_iter()
        .any(|prefix| contains_prefix_followed_by_digit(payload, prefix))
}

fn contains_prefix_followed_by_digit(payload: &str, prefix: &str) -> bool {
    let mut remaining = payload;
    while let Some(index) = remaining.find(prefix) {
        let Some(after_prefix) = remaining.get(index + prefix.len()..) else {
            return false;
        };
        if after_prefix
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
        {
            return true;
        }
        remaining = after_prefix;
    }
    false
}

fn assert_redacted_quota_reconnect_payload(
    payload: &str,
    input: &QuotaReconnectTranscriptInput<'_>,
) -> Result<(), String> {
    let forbidden_fragments = [
        QUOTA_RECONNECT_PRIMARY.upstream_token,
        QUOTA_RECONNECT_FALLBACK.upstream_token,
        "installed-quota-primary-token-refresh",
        "installed-quota-fallback-token-refresh",
        QUOTA_RECONNECT_PRIMARY.label,
        QUOTA_RECONNECT_FALLBACK.label,
        quota_reconnect_usage_limit_frame(),
        input.codex_stdout,
        input.codex_stderr,
        "/Users/",
        "/var/folders/",
    ];
    for forbidden in forbidden_fragments
        .into_iter()
        .filter(|fragment| !fragment.is_empty())
    {
        if payload.contains(forbidden) {
            return Err(format!(
                "quota reconnect transcript leaked forbidden fragment: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn first_frame_shape_summary(first_frame: &Value) -> Value {
    serde_json::json!({
        "json_object": first_frame.is_object(),
        "non_prewarm_response_create": is_non_prewarm_response_create_frame(first_frame),
    })
}

fn response_create_frame_shape_summary(upstream: &MockWebSocketTranscript) -> Value {
    let response_create = upstream
        .request_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(is_non_prewarm_response_create_frame)
        .unwrap_or(Value::Null);
    serde_json::json!({
        "present": response_create.is_object(),
        "non_prewarm_response_create": is_non_prewarm_response_create_frame(&response_create),
    })
}

fn assert_redacted_transcript_payload(
    payload: &str,
    input: RedactedTranscriptInput<'_>,
) -> Result<(), String> {
    let forbidden_fragments = [
        input.http_sse_codex_stdout.as_deref(),
        input.http_sse_codex_stderr.as_deref(),
        input.websocket_codex_stdout.as_deref(),
        input.websocket_codex_stderr.as_deref(),
        (!input.upstream.first_frame.is_empty()).then_some(input.upstream.first_frame.as_str()),
        input
            .expected_account_label
            .strip_prefix("unsafe:")
            .filter(|value| !value.is_empty()),
        Some("first_frame_model"),
        Some("first_frame_has_input"),
        Some("first_frame_stream"),
        Some("local-token-canary"),
        Some("installed-smoke-matches-token"),
        Some("prompt-canary"),
        Some("raw-previous-response-id-canary"),
        Some("affinity-secret-canary"),
        Some("/Users/"),
        Some("/var/folders/"),
    ];
    for forbidden in forbidden_fragments
        .into_iter()
        .flatten()
        .filter(|fragment| !fragment.is_empty())
    {
        if payload.contains(forbidden) {
            return Err(format!(
                "redacted smoke transcript leaked forbidden fragment: {forbidden}"
            ));
        }
    }

    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MockWebSocketTranscript {
    headers: Vec<(String, String)>,
    first_frame: String,
    request_frames: Vec<String>,
    websocket_request_frame_count: usize,
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

    const fn websocket_handshake_count(&self) -> usize {
        if self.headers.is_empty() { 0 } else { 1 }
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
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<Result<(), String>>>,
}

struct MockConcurrentWebSocketUpstream {
    address: String,
    state: Arc<ConcurrentUpstreamSharedState>,
    shutdown: Arc<AtomicBool>,
    pressure_handles: PressureHandles,
    handle: Option<thread::JoinHandle<Result<(), String>>>,
}

struct MockQuotaReconnectWebSocketUpstream {
    address: String,
    state: Arc<Mutex<QuotaReconnectUpstreamState>>,
    shutdown: Arc<AtomicBool>,
    pressure_handles: PressureHandles,
    handle: Option<thread::JoinHandle<Result<(), String>>>,
}

struct S8OverlapQuotaErrorContext {
    shared: Arc<ConcurrentUpstreamSharedState>,
    overlap_started_at: Instant,
    config: ConcurrentUpstreamConfig,
    sqlite_pressure: Option<QuotaReconnectSqlitePressureConfig>,
    pressure_handles: PressureHandles,
    frame_count: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct QuotaReconnectWebSocketTranscript {
    http_probe_count: usize,
    websocket_handshake_count: usize,
    request_frame_count: usize,
    prewarm_frame_count: usize,
    non_prewarm_frame_count: usize,
    quota_error_sent: bool,
    completion_sent: bool,
    quota_error_connection_label: Option<String>,
    completion_connection_label: Option<String>,
    quota_error_sent_unix_ms: Option<u128>,
    signal_latency_ms: Option<u128>,
    sqlite_pressure: Option<QuotaReconnectSqlitePressureTranscript>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct QuotaReconnectUpstreamState {
    http_probe_count: usize,
    websocket_handshake_count: usize,
    request_frame_count: usize,
    prewarm_frame_count: usize,
    non_prewarm_frame_count: usize,
    quota_error_sent: bool,
    completion_sent: bool,
    quota_error_connection_token: Option<String>,
    completion_connection_token: Option<String>,
    quota_error_sent_unix_ms: Option<u128>,
    completion_sent_unix_ms: Option<u128>,
    sqlite_pressure_requested: bool,
    sqlite_pressure_acquired_unix_ms: Option<u128>,
    sqlite_pressure_released_unix_ms: Option<u128>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaReconnectSqlitePressureConfig {
    state_path: PathBuf,
    hold_duration: Duration,
}

impl QuotaReconnectSqlitePressureConfig {
    fn new(state_path: PathBuf) -> Self {
        Self {
            state_path,
            hold_duration: QUOTA_RECONNECT_SQLITE_PRESSURE_HOLD,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaReconnectSqlitePressureTranscript {
    mechanism: &'static str,
    hold_duration_ms: u128,
    acquired_before_quota_error: bool,
    released_after_completion: bool,
    acquired_unix_ms: Option<u128>,
    released_unix_ms: Option<u128>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConcurrentWebSocketTranscript {
    expected_sessions: usize,
    expected_upstream_sessions: usize,
    completed_sessions: usize,
    final_active_sessions: usize,
    active_high_water: usize,
    overlap_started_unix_ms: Option<u128>,
    overlap_completed_unix_ms: Option<u128>,
    real_overlap_completed_unix_ms: Option<u128>,
    overlap_duration_ms: u128,
    real_overlap_duration_ms: u128,
    hold_duration: Duration,
    http_probe_count: usize,
    upstream_session_ids: Vec<u64>,
    upstream_client_sessions: Vec<UpstreamClientSessionObservation>,
    session_frame_counts: Vec<usize>,
    session_event_counts: Vec<usize>,
    in_overlap_session_event_counts: Vec<usize>,
    non_prewarm_session_count: usize,
    normal_close_sessions: usize,
    abnormal_close_sessions: usize,
    session_close_outcomes: Vec<String>,
    target_model_session_count: usize,
    unexpected_response_create_models: Vec<String>,
    multi_step_interleave_completed: bool,
    multi_step_followup_frame_count: usize,
    multi_step_followup_active_session_count: usize,
    multi_step_followup_unix_ms: Option<u128>,
    multi_step_completed_before_overlap_end: bool,
    quota_error_sent: bool,
    completion_sent: bool,
    quota_error_connection_label: Option<String>,
    completion_connection_label: Option<String>,
    quota_error_sent_unix_ms: Option<u128>,
    signal_latency_ms: Option<u128>,
    sqlite_pressure: Option<QuotaReconnectSqlitePressureTranscript>,
}

#[derive(Debug)]
struct ConcurrentUpstreamSharedState {
    state: Mutex<ConcurrentUpstreamState>,
    condition: Condvar,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConcurrentUpstreamState {
    expected_sessions: usize,
    expected_upstream_sessions: usize,
    hold_duration: Duration,
    active_non_prewarm_sessions: usize,
    active_high_water: usize,
    completed_sessions: usize,
    final_active_sessions: usize,
    overlap_started_at: Option<Instant>,
    overlap_started_unix_ms: Option<u128>,
    overlap_completed_unix_ms: Option<u128>,
    real_overlap_completed_unix_ms: Option<u128>,
    http_probe_count: usize,
    upstream_session_ids: Vec<u64>,
    upstream_client_sessions: Vec<UpstreamClientSessionObservation>,
    session_frame_counts: Vec<usize>,
    session_event_counts: Vec<usize>,
    in_overlap_session_event_counts: Vec<usize>,
    non_prewarm_session_count: usize,
    normal_close_sessions: usize,
    abnormal_close_sessions: usize,
    session_close_outcomes: Vec<String>,
    target_model_session_count: usize,
    unexpected_response_create_models: Vec<String>,
    multi_step_interleave_claimed: bool,
    multi_step_interleave_completed: bool,
    sessions_with_overlap_proof_events: usize,
    multi_step_followup_frame_count: usize,
    multi_step_followup_active_session_count: usize,
    multi_step_followup_unix_ms: Option<u128>,
    multi_step_completed_unix_ms: Option<u128>,
    quota_reconnect_claimed: bool,
    quota_error_sent: bool,
    completion_sent: bool,
    quota_error_connection_token: Option<String>,
    completion_connection_token: Option<String>,
    quota_error_sent_unix_ms: Option<u128>,
    completion_sent_unix_ms: Option<u128>,
    sqlite_pressure_requested: bool,
    sqlite_pressure_acquired_unix_ms: Option<u128>,
    sqlite_pressure_released_unix_ms: Option<u128>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouterSocketCleanupObservation {
    lsof_exit_status: String,
    tcp_line_count: usize,
    established_count: usize,
    close_wait_count: usize,
    raw_state_counts: Vec<(String, usize)>,
}

struct MockNoConnectionUpstream {
    address: String,
    handle: Option<thread::JoinHandle<Result<usize, String>>>,
}

struct RouterProcessGuard {
    child: Option<Child>,
    stdout_handle: Option<thread::JoinHandle<Vec<String>>>,
    stderr_handle: Option<thread::JoinHandle<Vec<String>>>,
    observation: RouterProcessObservation,
}

impl RouterProcessGuard {
    fn stop(mut self, label: &str) -> Result<RouterProcessObservation, String> {
        self.terminate_child(label, Duration::ZERO)?;
        self.join_output_readers(label)?;
        Ok(self.observation.clone())
    }

    fn wait(mut self, label: &str, timeout: Duration) -> Result<RouterProcessObservation, String> {
        let Some(mut child) = self.child.take() else {
            self.join_output_readers(label)?;
            return Ok(self.observation.clone());
        };
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let stdout_lines = self.join_output_reader(label, "stdout")?;
                    let stderr_lines = self.join_output_reader(label, "stderr")?;
                    if !status.success() {
                        self.observation.cleanup_result = format!("exited:{status}");
                        return Err(format!(
                            "{label} exited with status {status}\nstdout:\n{}\nstderr:\n{}",
                            stdout_lines.join("\n"),
                            stderr_lines.join("\n")
                        ));
                    }
                    self.observation.cleanup_result = format!("exited:{status}");
                    return Ok(self.observation.clone());
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let status = child
                        .wait()
                        .map_err(|error| format!("failed to wait for {label}: {error}"))?;
                    self.observation.cleanup_result = format!("wait-timeout-terminated:{status}");
                    let stdout_lines = self.join_output_reader(label, "stdout")?;
                    let stderr_lines = self.join_output_reader(label, "stderr")?;
                    return Err(format!(
                        "{label} did not exit before timeout\nstdout:\n{}\nstderr:\n{}",
                        stdout_lines.join("\n"),
                        stderr_lines.join("\n")
                    ));
                }
                Err(error) => {
                    return Err(format!("failed to inspect {label}: {error}"));
                }
            }
        }
    }

    fn terminate_child(&mut self, label: &str, grace: Duration) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        let deadline = Instant::now() + grace;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.observation.cleanup_result = format!("already-exited:{status}");
                    return Ok(());
                }
                Ok(None) if !grace.is_zero() && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let status = child
                        .wait()
                        .map_err(|error| format!("failed to wait for {label}: {error}"))?;
                    self.observation.cleanup_result = format!("terminated:{status}");
                    return Ok(());
                }
                Err(error) => {
                    return Err(format!("failed to inspect {label}: {error}"));
                }
            }
        }
    }

    fn join_output_readers(&mut self, label: &str) -> Result<(), String> {
        let _stdout_lines = self.join_output_reader(label, "stdout")?;
        let _stderr_lines = self.join_output_reader(label, "stderr")?;
        Ok(())
    }

    fn join_output_reader(
        &mut self,
        label: &str,
        stream_name: &str,
    ) -> Result<Vec<String>, String> {
        let handle = match stream_name {
            "stdout" => self.stdout_handle.take(),
            "stderr" => self.stderr_handle.take(),
            _ => None,
        };
        let Some(handle) = handle else {
            return Ok(Vec::new());
        };
        join_router_output_reader(handle, label, stream_name)
    }
}

impl Drop for RouterProcessGuard {
    fn drop(&mut self) {
        let _ = self.terminate_child("router child cleanup", Duration::ZERO);
        let _ = self.join_output_readers("router child cleanup");
    }
}

fn join_router_output_reader(
    handle: thread::JoinHandle<Vec<String>>,
    label: &str,
    stream_name: &str,
) -> Result<Vec<String>, String> {
    handle
        .join()
        .map_err(|_| format!("{label} {stream_name} reader panicked"))
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

        Ok(Self {
            address,
            handle: Some(handle),
        })
    }

    fn address(&self) -> &str {
        &self.address
    }

    fn join(mut self) -> Result<usize, String> {
        let handle = self
            .handle
            .take()
            .ok_or_else(|| "no-connection upstream was already joined".to_owned())?;
        join_result(handle, "no-connection upstream")
    }
}

impl Drop for MockNoConnectionUpstream {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = join_result(handle, "no-connection upstream cleanup");
        }
    }
}

impl MockWebSocketUpstream {
    fn start(mode: InstalledCodexSmokeMode) -> Result<Self, String> {
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
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_transcript = Arc::clone(&transcript);
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = thread::Builder::new()
            .name("codex-router-installed-smoke-upstream".to_owned())
            .spawn(move || run_mock_upstream(listener, thread_transcript, thread_shutdown, mode))
            .map_err(|error| format!("failed to spawn mock upstream thread: {error}"))?;

        Ok(Self {
            address,
            transcript,
            shutdown,
            handle: Some(handle),
        })
    }

    fn address(&self) -> &str {
        &self.address
    }

    fn join(mut self) -> Result<MockWebSocketTranscript, String> {
        self.shutdown.store(true, Ordering::SeqCst);
        wake_mock_upstream_accept(&self.address);
        let handle = self
            .handle
            .take()
            .ok_or_else(|| "mock websocket upstream was already joined".to_owned())?;
        join_result(handle, "mock websocket upstream")?;
        let mut transcript = self
            .transcript
            .lock()
            .map_err(|_| "mock upstream transcript mutex poisoned".to_owned())?;
        transcript
            .take()
            .ok_or_else(|| "mock upstream recorded no websocket transcript".to_owned())
    }
}

impl Drop for MockWebSocketUpstream {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.shutdown.store(true, Ordering::SeqCst);
            wake_mock_upstream_accept(&self.address);
            let _ = join_result(handle, "mock websocket upstream cleanup");
        }
    }
}

impl MockQuotaReconnectWebSocketUpstream {
    fn start(sqlite_pressure: Option<QuotaReconnectSqlitePressureConfig>) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("failed to bind quota reconnect upstream: {error}"))?;
        listener.set_nonblocking(true).map_err(|error| {
            format!("failed to configure quota reconnect upstream nonblocking: {error}")
        })?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("failed to read quota reconnect upstream address: {error}"))?
            .to_string();
        let state = Arc::new(Mutex::new(QuotaReconnectUpstreamState::default()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let pressure_handles = Arc::new(Mutex::new(Vec::new()));
        let thread_state = Arc::clone(&state);
        let thread_shutdown = Arc::clone(&shutdown);
        let thread_pressure_handles = Arc::clone(&pressure_handles);
        let handle = thread::Builder::new()
            .name("codex-router-quota-reconnect-upstream".to_owned())
            .spawn(move || {
                run_quota_reconnect_mock_upstream(
                    listener,
                    thread_state,
                    thread_shutdown,
                    sqlite_pressure,
                    thread_pressure_handles,
                )
            })
            .map_err(|error| format!("failed to spawn quota reconnect upstream thread: {error}"))?;

        Ok(Self {
            address,
            state,
            shutdown,
            pressure_handles,
            handle: Some(handle),
        })
    }

    fn address(&self) -> &str {
        &self.address
    }

    fn join(mut self) -> Result<QuotaReconnectWebSocketTranscript, String> {
        self.shutdown.store(true, Ordering::SeqCst);
        wake_mock_upstream_accept(&self.address);
        let handle = self
            .handle
            .take()
            .ok_or_else(|| "quota reconnect upstream was already joined".to_owned())?;
        join_result(handle, "quota reconnect upstream")?;
        let pressure_handles = self
            .pressure_handles
            .lock()
            .map_err(|_| "quota reconnect pressure handle mutex poisoned".to_owned())?
            .drain(..)
            .collect::<Vec<_>>();
        for pressure_handle in pressure_handles {
            join_result(pressure_handle, "quota reconnect sqlite pressure")?;
        }
        let state = self
            .state
            .lock()
            .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())?
            .clone();
        let sqlite_pressure =
            state
                .sqlite_pressure_requested
                .then(|| QuotaReconnectSqlitePressureTranscript {
                    mechanism: "copied-db-sqlite-write-lock",
                    hold_duration_ms: QUOTA_RECONNECT_SQLITE_PRESSURE_HOLD.as_millis(),
                    acquired_before_quota_error: state
                        .sqlite_pressure_acquired_unix_ms
                        .zip(state.quota_error_sent_unix_ms)
                        .is_some_and(|(acquired, quota_error)| acquired <= quota_error),
                    released_after_completion: state
                        .sqlite_pressure_released_unix_ms
                        .zip(state.completion_sent_unix_ms)
                        .is_some_and(|(released, completion)| released >= completion),
                    acquired_unix_ms: state.sqlite_pressure_acquired_unix_ms,
                    released_unix_ms: state.sqlite_pressure_released_unix_ms,
                });
        Ok(QuotaReconnectWebSocketTranscript {
            http_probe_count: state.http_probe_count,
            websocket_handshake_count: state.websocket_handshake_count,
            request_frame_count: state.request_frame_count,
            prewarm_frame_count: state.prewarm_frame_count,
            non_prewarm_frame_count: state.non_prewarm_frame_count,
            quota_error_sent: state.quota_error_sent,
            completion_sent: state.completion_sent,
            quota_error_connection_label: state
                .quota_error_connection_token
                .as_deref()
                .and_then(quota_reconnect_label_from_upstream_token)
                .map(str::to_owned),
            completion_connection_label: state
                .completion_connection_token
                .as_deref()
                .and_then(quota_reconnect_label_from_upstream_token)
                .map(str::to_owned),
            quota_error_sent_unix_ms: state.quota_error_sent_unix_ms,
            signal_latency_ms: state
                .quota_error_sent_unix_ms
                .zip(state.completion_sent_unix_ms)
                .map(|(quota_error, completion)| completion.saturating_sub(quota_error)),
            sqlite_pressure,
        })
    }
}

impl Drop for MockQuotaReconnectWebSocketUpstream {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.shutdown.store(true, Ordering::SeqCst);
            wake_mock_upstream_accept(&self.address);
            let _ = join_result(handle, "quota reconnect upstream cleanup");
        }
    }
}

impl MockConcurrentWebSocketUpstream {
    fn start(
        config: ConcurrentUpstreamConfig,
        sqlite_pressure: Option<QuotaReconnectSqlitePressureConfig>,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("failed to bind concurrent mock upstream: {error}"))?;
        listener.set_nonblocking(true).map_err(|error| {
            format!("failed to configure concurrent mock upstream nonblocking: {error}")
        })?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("failed to read concurrent upstream address: {error}"))?
            .to_string();
        let state = Arc::new(ConcurrentUpstreamSharedState {
            state: Mutex::new(ConcurrentUpstreamState {
                expected_sessions: config.expected_sessions,
                expected_upstream_sessions: config.expected_upstream_sessions,
                hold_duration: config.hold_duration,
                active_non_prewarm_sessions: 0,
                active_high_water: 0,
                completed_sessions: 0,
                final_active_sessions: 0,
                overlap_started_at: None,
                overlap_started_unix_ms: None,
                overlap_completed_unix_ms: None,
                real_overlap_completed_unix_ms: None,
                http_probe_count: 0,
                upstream_session_ids: Vec::new(),
                upstream_client_sessions: Vec::new(),
                session_frame_counts: Vec::new(),
                session_event_counts: Vec::new(),
                in_overlap_session_event_counts: Vec::new(),
                non_prewarm_session_count: 0,
                normal_close_sessions: 0,
                abnormal_close_sessions: 0,
                session_close_outcomes: Vec::new(),
                target_model_session_count: 0,
                unexpected_response_create_models: Vec::new(),
                multi_step_interleave_claimed: false,
                multi_step_interleave_completed: false,
                sessions_with_overlap_proof_events: 0,
                multi_step_followup_frame_count: 0,
                multi_step_followup_active_session_count: 0,
                multi_step_followup_unix_ms: None,
                multi_step_completed_unix_ms: None,
                quota_reconnect_claimed: false,
                quota_error_sent: false,
                completion_sent: false,
                quota_error_connection_token: None,
                completion_connection_token: None,
                quota_error_sent_unix_ms: None,
                completion_sent_unix_ms: None,
                sqlite_pressure_requested: false,
                sqlite_pressure_acquired_unix_ms: None,
                sqlite_pressure_released_unix_ms: None,
            }),
            condition: Condvar::new(),
        });
        let shutdown = Arc::new(AtomicBool::new(false));
        let pressure_handles = Arc::new(Mutex::new(Vec::new()));
        let thread_state = Arc::clone(&state);
        let thread_shutdown = Arc::clone(&shutdown);
        let thread_pressure_handles = Arc::clone(&pressure_handles);
        let handle = thread::Builder::new()
            .name("codex-router-three-client-upstream".to_owned())
            .spawn(move || {
                run_concurrent_mock_upstream(
                    listener,
                    thread_state,
                    thread_shutdown,
                    config,
                    sqlite_pressure,
                    thread_pressure_handles,
                )
            })
            .map_err(|error| format!("failed to spawn concurrent mock upstream: {error}"))?;

        Ok(Self {
            address,
            state,
            shutdown,
            pressure_handles,
            handle: Some(handle),
        })
    }

    fn address(&self) -> &str {
        &self.address
    }

    fn join(mut self) -> Result<ConcurrentWebSocketTranscript, String> {
        let handle = self
            .handle
            .take()
            .ok_or_else(|| "concurrent mock upstream was already joined".to_owned())?;
        join_result(handle, "concurrent mock upstream")?;
        let pressure_handles = self
            .pressure_handles
            .lock()
            .map_err(|_| "concurrent pressure handle mutex poisoned".to_owned())?
            .drain(..)
            .collect::<Vec<_>>();
        for pressure_handle in pressure_handles {
            join_result(pressure_handle, "S8 overlap quota sqlite pressure")?;
        }
        let state = self
            .state
            .state
            .lock()
            .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?
            .clone();
        if state.active_high_water < state.expected_sessions {
            return Err(format!(
                "concurrent upstream high-water was {}, expected at least {}",
                state.active_high_water, state.expected_sessions
            ));
        }
        if state.completed_sessions < state.expected_sessions {
            return Err(format!(
                "concurrent upstream completed {} sessions, expected at least {}",
                state.completed_sessions, state.expected_sessions
            ));
        }
        let sqlite_pressure =
            state
                .sqlite_pressure_requested
                .then(|| QuotaReconnectSqlitePressureTranscript {
                    mechanism: "copied-db-sqlite-write-lock",
                    hold_duration_ms: QUOTA_RECONNECT_SQLITE_PRESSURE_HOLD.as_millis(),
                    acquired_before_quota_error: state
                        .sqlite_pressure_acquired_unix_ms
                        .zip(state.quota_error_sent_unix_ms)
                        .is_some_and(|(acquired, quota_error)| acquired <= quota_error),
                    released_after_completion: state
                        .sqlite_pressure_released_unix_ms
                        .zip(state.completion_sent_unix_ms)
                        .is_some_and(|(released, completion)| released >= completion),
                    acquired_unix_ms: state.sqlite_pressure_acquired_unix_ms,
                    released_unix_ms: state.sqlite_pressure_released_unix_ms,
                });
        Ok(ConcurrentWebSocketTranscript {
            expected_sessions: state.expected_sessions,
            expected_upstream_sessions: state.expected_upstream_sessions,
            completed_sessions: state.completed_sessions,
            final_active_sessions: state.final_active_sessions,
            active_high_water: state.active_high_water,
            overlap_started_unix_ms: state.overlap_started_unix_ms,
            overlap_completed_unix_ms: state.overlap_completed_unix_ms,
            real_overlap_completed_unix_ms: state.real_overlap_completed_unix_ms,
            overlap_duration_ms: overlap_duration_ms(&state),
            real_overlap_duration_ms: real_overlap_duration_ms(&state),
            hold_duration: state.hold_duration,
            http_probe_count: state.http_probe_count,
            upstream_session_ids: state.upstream_session_ids,
            upstream_client_sessions: state.upstream_client_sessions,
            session_frame_counts: state.session_frame_counts,
            session_event_counts: state.session_event_counts,
            in_overlap_session_event_counts: state.in_overlap_session_event_counts,
            non_prewarm_session_count: state.non_prewarm_session_count,
            normal_close_sessions: state.normal_close_sessions,
            abnormal_close_sessions: state.abnormal_close_sessions,
            session_close_outcomes: state.session_close_outcomes,
            target_model_session_count: state.target_model_session_count,
            unexpected_response_create_models: state.unexpected_response_create_models,
            multi_step_interleave_completed: state.multi_step_interleave_completed,
            multi_step_followup_frame_count: state.multi_step_followup_frame_count,
            multi_step_followup_active_session_count: state
                .multi_step_followup_active_session_count,
            multi_step_followup_unix_ms: state.multi_step_followup_unix_ms,
            multi_step_completed_before_overlap_end: state
                .multi_step_completed_unix_ms
                .zip(state.real_overlap_completed_unix_ms)
                .is_some_and(|(multi_step_completed, overlap_completed)| {
                    multi_step_completed <= overlap_completed
                }),
            quota_error_sent: state.quota_error_sent,
            completion_sent: state.completion_sent,
            quota_error_connection_label: state
                .quota_error_connection_token
                .as_deref()
                .and_then(quota_reconnect_label_from_upstream_token)
                .map(str::to_owned),
            completion_connection_label: state
                .completion_connection_token
                .as_deref()
                .and_then(quota_reconnect_label_from_upstream_token)
                .map(str::to_owned),
            quota_error_sent_unix_ms: state.quota_error_sent_unix_ms,
            signal_latency_ms: state
                .quota_error_sent_unix_ms
                .zip(state.completion_sent_unix_ms)
                .map(|(quota_error, completion)| completion.saturating_sub(quota_error)),
            sqlite_pressure,
        })
    }
}

impl Drop for MockConcurrentWebSocketUpstream {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.shutdown.store(true, Ordering::SeqCst);
            wake_mock_upstream_accept(&self.address);
            self.state.condition.notify_all();
            let _ = join_result(handle, "concurrent mock upstream cleanup");
        }
    }
}

fn run_mock_upstream(
    listener: TcpListener,
    transcript: Arc<Mutex<Option<MockWebSocketTranscript>>>,
    shutdown: Arc<AtomicBool>,
    mode: InstalledCodexSmokeMode,
) -> Result<(), String> {
    let mut http_probe_count = 0_usize;
    let mut http_sse_count = 0_usize;
    let mut http_sse = None;
    let mut websocket_count = 0_usize;
    loop {
        if shutdown.load(Ordering::SeqCst) && (!mode.requires_websocket() || websocket_count > 0) {
            return Ok(());
        }
        let deadline = Instant::now() + UPSTREAM_ACCEPT_TIMEOUT;
        let stream = match accept_with_deadline(
            &listener,
            &shutdown,
            deadline,
            http_probe_count,
            http_sse_count,
        ) {
            Ok(stream) => stream,
            Err(_error)
                if shutdown.load(Ordering::SeqCst)
                    && (!mode.requires_websocket() || websocket_count > 0) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if !looks_like_websocket_upgrade(&stream)? {
            match respond_to_http_request(stream)? {
                MockHttpRequestResult::Probe => http_probe_count += 1,
                MockHttpRequestResult::Responses(http_sse_transcript) => {
                    http_sse_count += 1;
                    http_sse = Some(http_sse_transcript);
                    if !mode.requires_websocket() {
                        record_http_sse_only_transcript(
                            &transcript,
                            http_probe_count,
                            http_sse.take(),
                        )?;
                        return Ok(());
                    }
                }
            }
            continue;
        }
        if !mode.requires_websocket() {
            return Err("mock upstream received unexpected websocket in HTTP/SSE mode".to_owned());
        }
        run_mock_websocket(
            stream,
            Arc::clone(&transcript),
            http_probe_count,
            http_sse.take(),
        )?;
        websocket_count = websocket_count.saturating_add(1);
        if websocket_count >= 8 {
            return Ok(());
        }
    }
}

fn run_quota_reconnect_mock_upstream(
    listener: TcpListener,
    state: Arc<Mutex<QuotaReconnectUpstreamState>>,
    shutdown: Arc<AtomicBool>,
    sqlite_pressure: Option<QuotaReconnectSqlitePressureConfig>,
    pressure_handles: PressureHandles,
) -> Result<(), String> {
    let deadline = Instant::now() + UPSTREAM_ACCEPT_TIMEOUT;
    loop {
        if quota_reconnect_completion_sent(&state)? {
            return Ok(());
        }
        if shutdown.load(Ordering::SeqCst) {
            return Err("quota reconnect upstream shut down before completion".to_owned());
        }
        if Instant::now() >= deadline {
            return Err("quota reconnect upstream timed out before completion".to_owned());
        }
        match listener.accept() {
            Ok((stream, _peer)) => {
                stream.set_nonblocking(false).map_err(|error| {
                    format!("failed to restore quota reconnect stream blocking mode: {error}")
                })?;
                if !looks_like_websocket_upgrade(&stream)? {
                    respond_to_http_request(stream)?;
                    let mut state = state
                        .lock()
                        .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())?;
                    state.http_probe_count = state.http_probe_count.saturating_add(1);
                    continue;
                }
                run_quota_reconnect_mock_websocket_session(
                    stream,
                    Arc::clone(&state),
                    sqlite_pressure.clone(),
                    Arc::clone(&pressure_handles),
                )?;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("quota reconnect upstream accept failed: {error}")),
        }
    }
}

#[allow(clippy::result_large_err)]
fn run_quota_reconnect_mock_websocket_session(
    stream: std::net::TcpStream,
    state: Arc<Mutex<QuotaReconnectUpstreamState>>,
    sqlite_pressure: Option<QuotaReconnectSqlitePressureConfig>,
    pressure_handles: PressureHandles,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| {
            format!("quota reconnect upstream failed to set websocket read timeout: {error}")
        })?;
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
    .map_err(|error| format!("quota reconnect websocket handshake failed: {error}"))?;
    {
        let mut state = state
            .lock()
            .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())?;
        state.websocket_handshake_count = state.websocket_handshake_count.saturating_add(1);
    }
    let token = {
        let headers = captured_headers
            .lock()
            .map_err(|_| "quota reconnect upstream header mutex poisoned".to_owned())?;
        bearer_token_from_headers(&headers)
            .ok_or_else(|| "quota reconnect upstream missing bearer token".to_owned())?
            .to_owned()
    };
    for request_index in 0..4 {
        let frame = match websocket.read() {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Binary(bytes)) => String::from_utf8(bytes.to_vec()).map_err(|error| {
                format!("quota reconnect upstream frame was not UTF-8: {error}")
            })?,
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_other) => continue,
            Err(tungstenite::Error::Io(error))
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut =>
            {
                return Ok(());
            }
            Err(error) => {
                return Err(format!(
                    "quota reconnect upstream failed to read frame: {error}"
                ));
            }
        };
        {
            let mut state = state
                .lock()
                .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())?;
            state.request_frame_count = state.request_frame_count.saturating_add(1);
        }
        if is_prewarm_request_frame(&frame) {
            {
                let mut state = state
                    .lock()
                    .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())?;
                state.prewarm_frame_count = state.prewarm_frame_count.saturating_add(1);
            }
            for event in smoke_prewarm_events(request_index) {
                websocket
                    .send(Message::Text(event.into()))
                    .map_err(|error| {
                        format!("quota reconnect upstream failed to send prewarm event: {error}")
                    })?;
            }
            continue;
        }

        let send_quota_error = {
            let mut state = state
                .lock()
                .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())?;
            state.non_prewarm_frame_count = state.non_prewarm_frame_count.saturating_add(1);
            if !state.quota_error_sent {
                state.quota_error_sent = true;
                state.quota_error_connection_token = Some(token);
                true
            } else {
                state.completion_sent = true;
                state.completion_connection_token = Some(token);
                false
            }
        };
        if send_quota_error {
            if let Some(sqlite_pressure) = sqlite_pressure {
                let pressure_handle =
                    start_quota_reconnect_sqlite_pressure(sqlite_pressure, Arc::clone(&state))?;
                pressure_handles
                    .lock()
                    .map_err(|_| "quota reconnect pressure handle mutex poisoned".to_owned())?
                    .push(pressure_handle);
            }
            websocket
                .send(Message::Text(quota_reconnect_usage_limit_frame().into()))
                .map_err(|error| {
                    format!("quota reconnect upstream failed to send usage limit: {error}")
                })?;
            {
                let mut state = state
                    .lock()
                    .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())?;
                state.quota_error_sent_unix_ms = Some(timestamp_millis());
            }
            let _close_result = websocket.close(None);
            return Ok(());
        }
        for event in smoke_response_events(request_index) {
            websocket
                .send(Message::Text(event.into()))
                .map_err(|error| {
                    format!("quota reconnect upstream failed to send completion event: {error}")
                })?;
        }
        {
            let mut state = state
                .lock()
                .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())?;
            state.completion_sent_unix_ms = Some(timestamp_millis());
        }
        let _close_result = websocket.close(None);
        return Ok(());
    }
    Ok(())
}

fn start_quota_reconnect_sqlite_pressure(
    config: QuotaReconnectSqlitePressureConfig,
    state: Arc<Mutex<QuotaReconnectUpstreamState>>,
) -> Result<thread::JoinHandle<Result<(), String>>, String> {
    let (ready_sender, ready_receiver) = mpsc::channel();
    let handle = thread::Builder::new()
        .name("codex-router-quota-reconnect-sqlite-pressure".to_owned())
        .spawn(move || {
            let mut child = Command::new("python3");
            child
                .arg("-c")
                .arg(
                    r#"
import sqlite3
import sys
import time

database_path = sys.argv[1]
hold_seconds = float(sys.argv[2])
connection = sqlite3.connect(database_path, timeout=0)
try:
    connection.execute("BEGIN IMMEDIATE")
    print("acquired", flush=True)
    time.sleep(hold_seconds)
    connection.commit()
finally:
    connection.close()
"#,
                )
                .arg(&config.state_path)
                .arg(format!("{}", config.hold_duration.as_secs_f64()))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = match child.spawn() {
                Ok(child) => child,
                Err(error) => {
                    let error =
                        format!("failed to spawn quota reconnect pressure helper: {error}");
                    let _send_result = ready_sender.send(Err(error.clone()));
                    return Err(error);
                }
            };
            let Some(stdout) = child.stdout.take() else {
                let error = "quota reconnect pressure helper stdout was unavailable".to_owned();
                let _send_result = ready_sender.send(Err(error.clone()));
                return Err(error);
            };
            let mut stdout = BufReader::new(stdout);
            let mut ready_line = String::new();
            stdout.read_line(&mut ready_line).map_err(|error| {
                format!("failed to read quota reconnect pressure readiness: {error}")
            })?;
            if ready_line.trim() != "acquired" {
                let output = child.wait_with_output().map_err(|error| {
                    format!("failed to wait for quota reconnect pressure helper: {error}")
                })?;
                let error = format!(
                    "failed to acquire quota reconnect pressure write lock on {}: status={} stderr={}",
                    config.state_path.display(),
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
                let _send_result = ready_sender.send(Err(error.clone()));
                return Err(error);
            };
            {
                let mut state = state.lock().map_err(|_| {
                    "quota reconnect upstream state mutex poisoned during pressure acquire"
                        .to_owned()
                })?;
                state.sqlite_pressure_requested = true;
                state.sqlite_pressure_acquired_unix_ms = Some(timestamp_millis());
            }
            let _ready_result = ready_sender.send(Ok(()));
            let output = child.wait_with_output().map_err(|error| {
                format!(
                    "failed to wait for quota reconnect pressure helper on {}: {error}",
                    config.state_path.display(),
                )
            })?;
            if !output.status.success() {
                return Err(format!(
                    "quota reconnect pressure helper failed on {}: status={} stderr={}",
                    config.state_path.display(),
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            {
                let mut state = state.lock().map_err(|_| {
                    "quota reconnect upstream state mutex poisoned during pressure release"
                        .to_owned()
                })?;
                state.sqlite_pressure_released_unix_ms = Some(timestamp_millis());
            }
            Ok(())
        })
        .map_err(|error| format!("failed to spawn quota reconnect sqlite pressure: {error}"))?;
    match ready_receiver.recv_timeout(QUOTA_RECONNECT_SQLITE_PRESSURE_READY_TIMEOUT) {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(format!(
            "quota reconnect sqlite pressure did not acquire before timeout: {error}"
        )),
    }
}

fn start_s8_overlap_quota_sqlite_pressure(
    config: QuotaReconnectSqlitePressureConfig,
    shared: Arc<ConcurrentUpstreamSharedState>,
) -> Result<thread::JoinHandle<Result<(), String>>, String> {
    let (ready_sender, ready_receiver) = mpsc::channel();
    let handle = thread::Builder::new()
        .name("codex-router-s8-overlap-quota-sqlite-pressure".to_owned())
        .spawn(move || {
            let mut child = Command::new("python3");
            child
                .arg("-c")
                .arg(
                    r#"
import sqlite3
import sys
import time

database_path = sys.argv[1]
hold_seconds = float(sys.argv[2])
connection = sqlite3.connect(database_path, timeout=0)
try:
    connection.execute("BEGIN IMMEDIATE")
    print("acquired", flush=True)
    time.sleep(hold_seconds)
    connection.commit()
finally:
    connection.close()
"#,
                )
                .arg(&config.state_path)
                .arg(format!("{}", config.hold_duration.as_secs_f64()))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = match child.spawn() {
                Ok(child) => child,
                Err(error) => {
                    let error = format!("failed to spawn S8 overlap quota pressure helper: {error}");
                    let _send_result = ready_sender.send(Err(error.clone()));
                    return Err(error);
                }
            };
            let Some(stdout) = child.stdout.take() else {
                let error = "S8 overlap quota pressure helper stdout was unavailable".to_owned();
                let _send_result = ready_sender.send(Err(error.clone()));
                return Err(error);
            };
            let mut stdout = BufReader::new(stdout);
            let mut ready_line = String::new();
            stdout.read_line(&mut ready_line).map_err(|error| {
                format!("failed to read S8 overlap quota pressure readiness: {error}")
            })?;
            if ready_line.trim() != "acquired" {
                let output = child.wait_with_output().map_err(|error| {
                    format!("failed to wait for S8 overlap quota pressure helper: {error}")
                })?;
                let error = format!(
                    "failed to acquire S8 overlap quota pressure write lock on {}: status={} stderr={}",
                    config.state_path.display(),
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
                let _send_result = ready_sender.send(Err(error.clone()));
                return Err(error);
            };
            {
                let mut state = shared.state.lock().map_err(|_| {
                    "concurrent upstream state mutex poisoned during pressure acquire".to_owned()
                })?;
                state.sqlite_pressure_requested = true;
                state.sqlite_pressure_acquired_unix_ms = Some(timestamp_millis());
            }
            shared.condition.notify_all();
            let _ready_result = ready_sender.send(Ok(()));
            let output = child.wait_with_output().map_err(|error| {
                format!(
                    "failed to wait for S8 overlap quota pressure helper on {}: {error}",
                    config.state_path.display(),
                )
            })?;
            if !output.status.success() {
                return Err(format!(
                    "S8 overlap quota pressure helper failed on {}: status={} stderr={}",
                    config.state_path.display(),
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            {
                let mut state = shared.state.lock().map_err(|_| {
                    "concurrent upstream state mutex poisoned during pressure release".to_owned()
                })?;
                state.sqlite_pressure_released_unix_ms = Some(timestamp_millis());
            }
            shared.condition.notify_all();
            Ok(())
        })
        .map_err(|error| format!("failed to spawn S8 overlap quota sqlite pressure: {error}"))?;
    match ready_receiver.recv_timeout(QUOTA_RECONNECT_SQLITE_PRESSURE_READY_TIMEOUT) {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(format!(
            "S8 overlap quota sqlite pressure did not acquire before timeout: {error}"
        )),
    }
}

fn quota_reconnect_completion_sent(
    state: &Arc<Mutex<QuotaReconnectUpstreamState>>,
) -> Result<bool, String> {
    state
        .lock()
        .map(|state| state.completion_sent)
        .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())
}

fn run_concurrent_mock_upstream(
    listener: TcpListener,
    state: Arc<ConcurrentUpstreamSharedState>,
    shutdown: Arc<AtomicBool>,
    config: ConcurrentUpstreamConfig,
    sqlite_pressure: Option<QuotaReconnectSqlitePressureConfig>,
    pressure_handles: PressureHandles,
) -> Result<(), String> {
    let deadline = Instant::now()
        + Duration::from_secs(45)
            .saturating_add(config.hold_duration)
            .saturating_add(config.heartbeat_interval);
    let mut handles = Vec::new();
    loop {
        {
            let state_guard = state
                .state
                .lock()
                .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
            if state_guard.completed_sessions >= config.expected_upstream_sessions {
                break;
            }
        }
        if shutdown.load(Ordering::SeqCst) {
            return Err(
                "concurrent upstream shut down before expected sessions completed".to_owned(),
            );
        }
        if Instant::now() >= deadline {
            return Err("concurrent upstream timed out waiting for sessions".to_owned());
        }
        match listener.accept() {
            Ok((stream, _peer)) => {
                stream.set_nonblocking(false).map_err(|error| {
                    format!("failed to restore concurrent upstream stream blocking mode: {error}")
                })?;
                if !looks_like_websocket_upgrade(&stream)? {
                    respond_to_http_request(stream)?;
                    let mut state_guard = state
                        .state
                        .lock()
                        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
                    state_guard.http_probe_count = state_guard.http_probe_count.saturating_add(1);
                    continue;
                }
                let session_state = Arc::clone(&state);
                let session_shutdown = Arc::clone(&shutdown);
                let session_sqlite_pressure = sqlite_pressure.clone();
                let session_pressure_handles = Arc::clone(&pressure_handles);
                handles.push(
                    thread::Builder::new()
                        .name("codex-router-three-client-upstream-session".to_owned())
                        .spawn(move || {
                            run_concurrent_mock_websocket_session(
                                stream,
                                session_state,
                                session_shutdown,
                                config,
                                session_sqlite_pressure,
                                session_pressure_handles,
                            )
                        })
                        .map_err(|error| {
                            format!("failed to spawn concurrent upstream session: {error}")
                        })?,
                );
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("concurrent upstream accept failed: {error}")),
        }
    }
    state.condition.notify_all();
    for (session_index, handle) in handles.into_iter().enumerate() {
        join_result(
            handle,
            &format!("concurrent upstream session {session_index}"),
        )?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn run_concurrent_mock_websocket_session(
    stream: std::net::TcpStream,
    state: Arc<ConcurrentUpstreamSharedState>,
    shutdown: Arc<AtomicBool>,
    config: ConcurrentUpstreamConfig,
    sqlite_pressure: Option<QuotaReconnectSqlitePressureConfig>,
    pressure_handles: PressureHandles,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(
            Duration::from_secs(30)
                .saturating_add(config.hold_duration)
                .saturating_add(config.heartbeat_interval),
        ))
        .map_err(|error| {
            format!("concurrent mock upstream failed to set websocket read timeout: {error}")
        })?;
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
    .map_err(|error| format!("concurrent mock upstream websocket handshake failed: {error}"))?;
    let token = {
        let headers = captured_headers
            .lock()
            .map_err(|_| "concurrent upstream header mutex poisoned".to_owned())?;
        bearer_token_from_headers(&headers)
            .ok_or_else(|| "concurrent upstream missing bearer token".to_owned())?
            .to_owned()
    };
    let mut frame_count = 0_usize;
    for request_index in 0..4 {
        if shutdown.load(Ordering::SeqCst) {
            return Err("concurrent mock upstream session shut down before request".to_owned());
        }
        let frame = match websocket.read() {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Binary(bytes)) => String::from_utf8(bytes.to_vec()).map_err(|error| {
                format!("concurrent mock upstream frame was not UTF-8: {error}")
            })?,
            Ok(Message::Close(_)) => break,
            Ok(_other) => continue,
            Err(_error) => {
                return Ok(());
            }
        };
        frame_count = frame_count.saturating_add(1);
        if is_prewarm_request_frame(&frame) {
            for event in smoke_prewarm_events(request_index) {
                websocket
                    .send(Message::Text(event.into()))
                    .map_err(|error| {
                        format!("concurrent mock upstream failed to send prewarm event: {error}")
                    })?;
            }
            continue;
        }
        let client_index = extract_harness_client_index(&frame);
        let observed_model = response_create_frame_model(&frame);
        let _upstream_session_id = register_concurrent_non_prewarm_session(
            &state,
            client_index,
            observed_model.as_deref(),
        )?;
        let overlap_started_at = wait_for_concurrent_session_barrier(&state)?;
        if claim_quota_reconnect_interleave(&state, &token, &frame)? {
            return send_s8_overlap_quota_error(
                &mut websocket,
                &token,
                S8OverlapQuotaErrorContext {
                    shared: Arc::clone(&state),
                    overlap_started_at,
                    config,
                    sqlite_pressure,
                    pressure_handles,
                    frame_count,
                },
            );
        }
        let quota_completion_session = record_quota_reconnect_completion_if_needed(&state, &token)?;
        let run_multi_step_interleave =
            !quota_completion_session && claim_multi_step_interleave(&state)?;
        let (event_count, in_overlap_event_count) = if run_multi_step_interleave {
            send_concurrent_multi_step_response_events(
                &mut websocket,
                request_index,
                overlap_started_at,
                config,
                &state,
                &mut frame_count,
            )?
        } else {
            send_concurrent_response_events(
                &mut websocket,
                request_index,
                overlap_started_at,
                config,
                &state,
            )?
        };
        wait_for_all_concurrent_overlap_proof_events(&state, config)?;
        let close_outcome = match websocket.close(None) {
            Ok(()) => "normal".to_owned(),
            Err(error) => format!("abnormal:{error}"),
        };
        finish_concurrent_non_prewarm_session(
            &state,
            frame_count,
            event_count,
            in_overlap_event_count,
            close_outcome,
        )?;
        return Ok(());
    }
    Err("concurrent mock upstream did not receive non-prewarm request frame".to_owned())
}

fn complete_multi_step_interleave(
    shared: &ConcurrentUpstreamSharedState,
    followup_frame_count: usize,
) -> Result<(), String> {
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    state.multi_step_interleave_completed = true;
    state.multi_step_followup_frame_count = state
        .multi_step_followup_frame_count
        .saturating_add(followup_frame_count);
    let completed_unix_ms = timestamp_millis();
    state.multi_step_followup_active_session_count = state.active_non_prewarm_sessions;
    state.multi_step_followup_unix_ms = Some(completed_unix_ms);
    state.multi_step_completed_unix_ms = Some(completed_unix_ms);
    shared.condition.notify_all();
    Ok(())
}

fn register_concurrent_non_prewarm_session(
    shared: &ConcurrentUpstreamSharedState,
    client_index: Option<usize>,
    observed_model: Option<&str>,
) -> Result<u64, String> {
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    state.active_non_prewarm_sessions = state.active_non_prewarm_sessions.saturating_add(1);
    state.non_prewarm_session_count = state.non_prewarm_session_count.saturating_add(1);
    let upstream_session_id = u64::try_from(state.non_prewarm_session_count)
        .map_err(|_| "concurrent upstream session id overflowed u64".to_owned())?;
    state.upstream_session_ids.push(upstream_session_id);
    if let Some(client_index) = client_index {
        state
            .upstream_client_sessions
            .push(UpstreamClientSessionObservation {
                client_index,
                upstream_session_id,
            });
    }
    match observed_model {
        Some(SMOKE_TARGET_MODEL) => {
            state.target_model_session_count = state.target_model_session_count.saturating_add(1);
        }
        Some(other) => state
            .unexpected_response_create_models
            .push(other.to_owned()),
        None => state
            .unexpected_response_create_models
            .push("<missing>".to_owned()),
    }
    state.active_high_water = state
        .active_high_water
        .max(state.active_non_prewarm_sessions);
    if state.active_high_water >= state.expected_sessions && state.overlap_started_at.is_none() {
        state.overlap_started_at = Some(Instant::now());
        state.overlap_started_unix_ms = Some(timestamp_millis());
    }
    shared.condition.notify_all();
    Ok(upstream_session_id)
}

fn extract_harness_client_index(frame: &str) -> Option<usize> {
    let marker = "codex-router-client-";
    let marker_start = frame.find(marker)? + marker.len();
    let digits = frame
        .get(marker_start..)?
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse::<usize>().ok()
}

fn response_create_frame_model(frame: &str) -> Option<String> {
    serde_json::from_str::<Value>(frame).ok().and_then(|value| {
        is_non_prewarm_response_create_frame(&value)
            .then(|| {
                value
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten()
    })
}

fn wait_for_concurrent_session_barrier(
    shared: &ConcurrentUpstreamSharedState,
) -> Result<Instant, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    loop {
        if state.active_high_water >= state.expected_sessions
            && let Some(overlap_started_at) = state.overlap_started_at
        {
            return Ok(overlap_started_at);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(format!(
                "concurrent upstream barrier timed out with active_high_water={} expected={}",
                state.active_high_water, state.expected_sessions
            ));
        }
        let wait = deadline.saturating_duration_since(now);
        let (next_state, _timeout) = shared
            .condition
            .wait_timeout(state, wait.min(Duration::from_millis(100)))
            .map_err(|_| "concurrent upstream condition wait poisoned".to_owned())?;
        state = next_state;
    }
}

fn finish_concurrent_non_prewarm_session(
    shared: &ConcurrentUpstreamSharedState,
    frame_count: usize,
    event_count: usize,
    in_overlap_event_count: usize,
    close_outcome: String,
) -> Result<(), String> {
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    if close_outcome == "normal" {
        state.normal_close_sessions = state.normal_close_sessions.saturating_add(1);
    } else {
        state.abnormal_close_sessions = state.abnormal_close_sessions.saturating_add(1);
    }
    state.session_close_outcomes.push(close_outcome);
    state.active_non_prewarm_sessions = state.active_non_prewarm_sessions.saturating_sub(1);
    if state.overlap_started_unix_ms.is_some()
        && state.real_overlap_completed_unix_ms.is_none()
        && state.active_non_prewarm_sessions < state.expected_sessions
    {
        state.real_overlap_completed_unix_ms = Some(timestamp_millis());
    }
    state.completed_sessions = state.completed_sessions.saturating_add(1);
    state.final_active_sessions = state.active_non_prewarm_sessions;
    if state.completed_sessions >= state.expected_sessions {
        state.overlap_completed_unix_ms = Some(timestamp_millis());
    }
    state.session_frame_counts.push(frame_count);
    state.session_event_counts.push(event_count);
    state
        .in_overlap_session_event_counts
        .push(in_overlap_event_count);
    shared.condition.notify_all();
    Ok(())
}

fn wait_for_all_concurrent_overlap_proof_events(
    shared: &ConcurrentUpstreamSharedState,
    config: ConcurrentUpstreamConfig,
) -> Result<(), String> {
    let deadline = Instant::now()
        + Duration::from_secs(20)
            .saturating_add(config.hold_duration)
            .saturating_add(config.heartbeat_interval);
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    state.sessions_with_overlap_proof_events =
        state.sessions_with_overlap_proof_events.saturating_add(1);
    shared.condition.notify_all();
    loop {
        if state.sessions_with_overlap_proof_events >= state.expected_sessions {
            return Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(format!(
                "concurrent upstream timed out waiting for overlap proof events sessions_with_overlap_proof_events={} expected={}",
                state.sessions_with_overlap_proof_events, state.expected_sessions
            ));
        }
        let wait = deadline.saturating_duration_since(now);
        let (next_state, _timeout) = shared
            .condition
            .wait_timeout(state, wait.min(Duration::from_millis(100)))
            .map_err(|_| "concurrent upstream condition wait poisoned".to_owned())?;
        state = next_state;
    }
}

fn claim_multi_step_interleave(shared: &ConcurrentUpstreamSharedState) -> Result<bool, String> {
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    if state.multi_step_interleave_claimed {
        return Ok(false);
    }
    state.multi_step_interleave_claimed = true;
    Ok(true)
}

fn claim_quota_reconnect_interleave(
    shared: &ConcurrentUpstreamSharedState,
    token: &str,
    frame: &str,
) -> Result<bool, String> {
    if token != QUOTA_RECONNECT_PRIMARY.upstream_token {
        return Ok(false);
    }
    if !frame.contains("codex-router-s8-quota-client") {
        return Ok(false);
    }
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    if state.expected_upstream_sessions == state.expected_sessions {
        return Ok(false);
    }
    if state.quota_reconnect_claimed {
        return Ok(false);
    }
    state.quota_reconnect_claimed = true;
    Ok(true)
}

fn record_quota_reconnect_completion_if_needed(
    shared: &ConcurrentUpstreamSharedState,
    token: &str,
) -> Result<bool, String> {
    if token != QUOTA_RECONNECT_FALLBACK.upstream_token {
        return Ok(false);
    }
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    if state.expected_upstream_sessions == state.expected_sessions {
        return Ok(false);
    }
    state.completion_sent = true;
    state.completion_connection_token = Some(token.to_owned());
    state.completion_sent_unix_ms = Some(timestamp_millis());
    shared.condition.notify_all();
    Ok(true)
}

fn send_s8_overlap_quota_error(
    websocket: &mut WebSocket<std::net::TcpStream>,
    token: &str,
    context: S8OverlapQuotaErrorContext,
) -> Result<(), String> {
    if !context.config.hold_duration.is_zero() {
        let quota_deadline = context.overlap_started_at + context.config.hold_duration;
        let remaining = quota_deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            thread::sleep(remaining);
        }
    }
    if let Some(sqlite_pressure) = context.sqlite_pressure {
        let pressure_handle =
            start_s8_overlap_quota_sqlite_pressure(sqlite_pressure, Arc::clone(&context.shared))?;
        context
            .pressure_handles
            .lock()
            .map_err(|_| "S8 overlap quota pressure handle mutex poisoned".to_owned())?
            .push(pressure_handle);
    }
    websocket
        .send(Message::Text(quota_reconnect_usage_limit_frame().into()))
        .map_err(|error| {
            format!("S8 overlap quota upstream failed to send usage limit: {error}")
        })?;
    {
        let mut state = context
            .shared
            .state
            .lock()
            .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
        state.quota_error_sent = true;
        state.quota_error_connection_token = Some(token.to_owned());
        state.quota_error_sent_unix_ms = Some(timestamp_millis());
    }
    let _close_result = websocket.close(None);
    finish_concurrent_non_prewarm_session(
        &context.shared,
        context.frame_count,
        1,
        usize::from(is_concurrent_overlap_active(&context.shared)?),
        "normal".to_owned(),
    )?;
    Ok(())
}

fn send_concurrent_response_events(
    websocket: &mut WebSocket<std::net::TcpStream>,
    request_index: usize,
    overlap_started_at: Instant,
    config: ConcurrentUpstreamConfig,
    state: &ConcurrentUpstreamSharedState,
) -> Result<(usize, usize), String> {
    let response_events = smoke_response_events(request_index);
    let mut event_count = 0_usize;
    let mut in_overlap_event_count = 0_usize;
    let Some(first_response_event) = response_events.first() else {
        return Err("mock upstream response events must not be empty".to_owned());
    };
    send_concurrent_response_event(websocket, first_response_event)?;
    event_count = event_count.saturating_add(1);
    in_overlap_event_count =
        in_overlap_event_count.saturating_add(usize::from(is_concurrent_overlap_active(state)?));

    if !config.hold_duration.is_zero() {
        let hold_deadline = overlap_started_at + config.hold_duration + SOAK_PROOF_MARGIN;
        let remaining = hold_deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            thread::sleep(remaining);
        }
    }

    for event in response_events.iter().skip(1) {
        send_concurrent_response_event(websocket, event)?;
        event_count = event_count.saturating_add(1);
        in_overlap_event_count = in_overlap_event_count
            .saturating_add(usize::from(is_concurrent_overlap_active(state)?));
    }

    Ok((event_count, in_overlap_event_count))
}

fn send_concurrent_multi_step_response_events(
    websocket: &mut WebSocket<std::net::TcpStream>,
    request_index: usize,
    overlap_started_at: Instant,
    config: ConcurrentUpstreamConfig,
    state: &ConcurrentUpstreamSharedState,
    frame_count: &mut usize,
) -> Result<(usize, usize), String> {
    let call_id = format!("codex-router-tool-call-{request_index}");
    let mut event_count = 0_usize;
    let mut in_overlap_event_count = 0_usize;
    let response_id = format!("resp-smoke-tool-{request_index}");
    send_concurrent_response_event(
        websocket,
        &serde_json::json!({
            "type": "response.created",
            "response": {"id": response_id}
        })
        .to_string(),
    )?;
    event_count = event_count.saturating_add(1);
    in_overlap_event_count =
        in_overlap_event_count.saturating_add(usize::from(is_concurrent_overlap_active(state)?));

    let tool_arguments = serde_json::json!({
        "command": "printf codex-router-tool-ok",
        "timeout_ms": 1000,
    })
    .to_string();
    send_concurrent_response_event(
        websocket,
        &serde_json::json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": call_id,
                "name": "shell_command",
                "arguments": tool_arguments,
            }
        })
        .to_string(),
    )?;
    event_count = event_count.saturating_add(1);
    in_overlap_event_count =
        in_overlap_event_count.saturating_add(usize::from(is_concurrent_overlap_active(state)?));
    send_concurrent_response_event(
        websocket,
        &serde_json::json!({
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
    )?;
    event_count = event_count.saturating_add(1);
    in_overlap_event_count =
        in_overlap_event_count.saturating_add(usize::from(is_concurrent_overlap_active(state)?));

    let followup_frame = read_concurrent_text_frame(websocket)?;
    *frame_count = frame_count.saturating_add(1);
    if !frame_contains_function_call_output(&followup_frame, &call_id) {
        return Err("multi-step follow-up frame did not contain function_call_output".to_owned());
    }
    complete_multi_step_interleave(state, 1)?;

    if !config.hold_duration.is_zero() {
        let hold_deadline = overlap_started_at + config.hold_duration + SOAK_PROOF_MARGIN;
        let remaining = hold_deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            thread::sleep(remaining);
        }
    }

    for event in smoke_response_events(request_index.saturating_add(10)) {
        send_concurrent_response_event(websocket, &event)?;
        event_count = event_count.saturating_add(1);
        in_overlap_event_count = in_overlap_event_count
            .saturating_add(usize::from(is_concurrent_overlap_active(state)?));
    }
    Ok((event_count, in_overlap_event_count))
}

fn is_concurrent_overlap_active(shared: &ConcurrentUpstreamSharedState) -> Result<bool, String> {
    let state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    Ok(state.active_non_prewarm_sessions >= state.expected_sessions)
}

fn read_concurrent_text_frame(
    websocket: &mut WebSocket<std::net::TcpStream>,
) -> Result<String, String> {
    loop {
        match websocket.read() {
            Ok(Message::Text(text)) => return Ok(text.to_string()),
            Ok(Message::Binary(bytes)) => {
                return String::from_utf8(bytes.to_vec()).map_err(|error| {
                    format!("concurrent mock upstream follow-up frame was not UTF-8: {error}")
                });
            }
            Ok(Message::Close(_)) => {
                return Err("concurrent mock upstream closed before follow-up frame".to_owned());
            }
            Ok(_other) => {}
            Err(error) => {
                return Err(format!(
                    "concurrent mock upstream failed to read follow-up frame: {error}"
                ));
            }
        }
    }
}

fn frame_contains_function_call_output(frame: &str, call_id: &str) -> bool {
    serde_json::from_str::<Value>(frame)
        .ok()
        .and_then(|value| value.get("input").and_then(Value::as_array).cloned())
        .is_some_and(|input| {
            input.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("function_call_output")
                    && item.get("call_id").and_then(Value::as_str) == Some(call_id)
            })
        })
}

fn send_concurrent_response_event(
    websocket: &mut WebSocket<std::net::TcpStream>,
    event: &str,
) -> Result<(), String> {
    websocket
        .send(Message::Text(event.to_owned().into()))
        .map_err(|error| format!("concurrent mock upstream failed to send response event: {error}"))
}

fn overlap_duration_ms(state: &ConcurrentUpstreamState) -> u128 {
    match (
        state.overlap_started_unix_ms,
        state.overlap_completed_unix_ms,
    ) {
        (Some(started), Some(completed)) => completed.saturating_sub(started),
        _ => 0,
    }
}

fn real_overlap_duration_ms(state: &ConcurrentUpstreamState) -> u128 {
    match (
        state.overlap_started_unix_ms,
        state.real_overlap_completed_unix_ms,
    ) {
        (Some(started), Some(completed)) => completed.saturating_sub(started),
        _ => 0,
    }
}

fn wake_mock_upstream_accept(address: &str) {
    if let Ok(stream) = std::net::TcpStream::connect(address) {
        let _ = stream.shutdown(Shutdown::Both);
    }
}

fn record_http_sse_only_transcript(
    transcript: &Arc<Mutex<Option<MockWebSocketTranscript>>>,
    http_probe_count: usize,
    http_sse: Option<MockHttpSseTranscript>,
) -> Result<(), String> {
    let mut transcript = transcript
        .lock()
        .map_err(|_| "mock upstream transcript mutex poisoned".to_owned())?;
    *transcript = Some(MockWebSocketTranscript {
        headers: Vec::new(),
        first_frame: String::new(),
        request_frames: Vec::new(),
        websocket_request_frame_count: 0,
        http_probe_count,
        http_sse,
    });
    Ok(())
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
    shutdown: &AtomicBool,
    deadline: Instant,
    http_probe_count: usize,
    http_sse_count: usize,
) -> Result<std::net::TcpStream, String> {
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return Err("mock upstream shut down before expected request arrived".to_owned());
        }
        match listener.accept() {
            Ok((stream, _peer)) => {
                if shutdown.load(Ordering::SeqCst) {
                    let _ = stream.shutdown(Shutdown::Both);
                    return Err(
                        "mock upstream shut down before expected request arrived".to_owned()
                    );
                }
                stream.set_nonblocking(false).map_err(|error| {
                    format!("failed to restore accepted stream blocking mode: {error}")
                })?;
                return Ok(stream);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "mock upstream timed out waiting for websocket (http_probe_count={http_probe_count}, http_sse_count={http_sse_count})"
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("mock upstream accept failed: {error}")),
        }
    }
}

fn redacted_command_text(bytes: &[u8], seed: &SmokeSeed) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.replace(&seed.local_token, "<local-router-token>")
        .replace(&seed.expected_upstream_token, "<selected-upstream-token>")
        .lines()
        .take(24)
        .collect::<Vec<_>>()
        .join("\\n")
}

fn redacted_optional_command_text(bytes: Option<&Vec<u8>>, seed: &SmokeSeed) -> String {
    bytes.map_or_else(
        || "<not-run>".to_owned(),
        |bytes| redacted_command_text(bytes, seed),
    )
}

fn output_status_text(output: Option<&Output>) -> String {
    output
        .map(|output| output.status.to_string())
        .unwrap_or_else(|| "not-run".to_owned())
}

fn http_sse_transcript_summary(transcript: &MockWebSocketTranscript) -> String {
    let http_sse = transcript.http_sse.as_ref();
    let request_line = http_sse
        .map(|request| request.request_line.as_str())
        .unwrap_or("<none>");
    let body_len = http_sse.map_or(0, |request| request.body.len());
    let stream_flag = http_sse.is_some_and(|request| request.body.contains("\"stream\":true"));
    format!(
        "http_request_line={request_line}; http_body_len={body_len}; stream_flag={stream_flag}; http_probe_count={}; websocket_frame_count={}",
        transcript.http_probe_count, transcript.websocket_request_frame_count
    )
}

fn looks_like_websocket_upgrade(stream: &std::net::TcpStream) -> Result<bool, String> {
    let mut buffer = [0_u8; 1024];
    let byte_count = stream
        .peek(&mut buffer)
        .map_err(|error| format!("mock upstream failed to peek request: {error}"))?;
    let request_bytes = buffer
        .get(..byte_count)
        .ok_or_else(|| "mock upstream peek byte count exceeded buffer length".to_owned())?;
    let request = String::from_utf8_lossy(request_bytes);
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
        let read_bytes = buffer
            .get(..byte_count)
            .ok_or_else(|| "mock upstream read byte count exceeded buffer length".to_owned())?;
        bytes.extend_from_slice(read_bytes);
        if let Some(header_end) = find_header_end(&bytes) {
            let header_bytes = bytes
                .get(..header_end)
                .ok_or_else(|| "mock upstream header boundary exceeded buffer length".to_owned())?;
            let header_text = String::from_utf8_lossy(header_bytes).to_string();
            let body_start = header_end + 4;
            if header_uses_chunked_transfer(&header_text) {
                let body_bytes = bytes.get(body_start..).ok_or_else(|| {
                    "mock upstream body boundary exceeded buffer length".to_owned()
                })?;
                if let Some(body) = decode_complete_chunked_body(body_bytes)? {
                    let (request_line, headers) = parse_http_head(&header_text)?;
                    return Ok(MockHttpSseTranscript {
                        request_line,
                        headers,
                        body,
                    });
                }
            } else {
                let content_length = parse_content_length(&header_text);
                if bytes.len() >= body_start + content_length {
                    let body_bytes = bytes
                        .get(body_start..body_start + content_length)
                        .ok_or_else(|| {
                            "mock upstream body length exceeded buffer length".to_owned()
                        })?;
                    let body = String::from_utf8_lossy(body_bytes).to_string();
                    let (request_line, headers) = parse_http_head(&header_text)?;
                    return Ok(MockHttpSseTranscript {
                        request_line,
                        headers,
                        body,
                    });
                }
            }
        }
    }
    Err("mock upstream received incomplete HTTP request".to_owned())
}

fn header_uses_chunked_transfer(header_text: &str) -> bool {
    header_text.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.eq_ignore_ascii_case("transfer-encoding")
            && value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
    })
}

fn decode_complete_chunked_body(bytes: &[u8]) -> Result<Option<String>, String> {
    let mut position = 0_usize;
    let mut body = Vec::new();
    loop {
        let Some(remaining) = bytes.get(position..) else {
            return Ok(None);
        };
        let Some(size_line_end) = find_crlf(remaining) else {
            return Ok(None);
        };
        let size_line_bytes = bytes
            .get(position..position + size_line_end)
            .ok_or_else(|| "chunk size line exceeded buffer length".to_owned())?;
        let size_line = std::str::from_utf8(size_line_bytes)
            .map_err(|error| format!("chunk size line was not UTF-8: {error}"))?;
        let size_text = size_line
            .split_once(';')
            .map_or(size_line, |(size, _)| size);
        let chunk_size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|error| format!("chunk size was invalid: {error}"))?;
        position = position.saturating_add(size_line_end + 2);
        if chunk_size == 0 {
            if bytes.get(position..position + 2) == Some(b"\r\n") {
                return String::from_utf8(body)
                    .map(Some)
                    .map_err(|error| format!("chunked body was not UTF-8: {error}"));
            }
            let Some(remaining) = bytes.get(position..) else {
                return Ok(None);
            };
            let Some(trailer_end) = find_header_end(remaining) else {
                return Ok(None);
            };
            let _consumed = position.saturating_add(trailer_end + 4);
            return String::from_utf8(body)
                .map(Some)
                .map_err(|error| format!("chunked body was not UTF-8: {error}"));
        }
        if bytes.len() < position.saturating_add(chunk_size).saturating_add(2) {
            return Ok(None);
        }
        let chunk = bytes
            .get(position..position + chunk_size)
            .ok_or_else(|| "chunk data exceeded buffer length".to_owned())?;
        body.extend_from_slice(chunk);
        position = position.saturating_add(chunk_size);
        if bytes.get(position..position + 2) != Some(b"\r\n") {
            return Err("chunk data was not followed by CRLF".to_owned());
        }
        position = position.saturating_add(2);
    }
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
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
    let mut request_frames = Vec::new();
    let mut websocket_request_frame_count = 0_usize;
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
        request_frames.push(frame.clone());
        websocket_request_frame_count = websocket_request_frame_count.saturating_add(1);
        let events = if is_prewarm_request_frame(&frame) {
            smoke_prewarm_events(request_index)
        } else {
            smoke_response_events(request_index)
        };
        for event in events {
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
    let mut candidate = MockWebSocketTranscript {
        headers,
        first_frame,
        request_frames,
        websocket_request_frame_count,
        http_probe_count,
        http_sse,
    };
    let candidate_has_non_prewarm = transcript_has_non_prewarm_request(&candidate);
    let mut transcript = transcript
        .lock()
        .map_err(|_| "mock upstream transcript mutex poisoned".to_owned())?;
    let should_replace = match transcript.as_ref() {
        None => true,
        Some(existing) => {
            candidate_has_non_prewarm || !transcript_has_non_prewarm_request(existing)
        }
    };
    if should_replace {
        if candidate.http_sse.is_none()
            && let Some(existing) = transcript.as_ref()
        {
            candidate.http_sse = existing.http_sse.clone();
        }
        *transcript = Some(candidate);
    }

    Ok(())
}

fn transcript_has_non_prewarm_request(transcript: &MockWebSocketTranscript) -> bool {
    transcript
        .request_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .any(|value| is_non_prewarm_response_create_frame(&value))
}

fn is_prewarm_request_frame(frame: &str) -> bool {
    serde_json::from_str::<Value>(frame)
        .ok()
        .is_some_and(|value| value.get("generate").and_then(Value::as_bool) == Some(false))
}

fn is_non_prewarm_response_create_frame(value: &Value) -> bool {
    value.get("generate").and_then(Value::as_bool) != Some(false)
        && value
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| !model.is_empty())
        && value
            .get("input")
            .and_then(Value::as_array)
            .is_some_and(|input| !input.is_empty())
        && value.get("stream").and_then(Value::as_bool) == Some(true)
}

fn smoke_prewarm_events(request_index: usize) -> Vec<String> {
    let response_id = format!("resp-smoke-prewarm-{request_index}");
    vec![
        serde_json::json!({
            "type": "response.created",
            "response": {"id": response_id, "status": "in_progress", "output": []}
        })
        .to_string(),
        serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": response_id,
                "status": "completed",
                "output": [],
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
    let token = assignment
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .ok_or_else(|| "token export assignment did not use expected POSIX shape".to_owned())?;
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

fn run_s8_overlap_quota_local_probe(router_port: u16, local_token: &str) -> Result<(), String> {
    let request_payload = serde_json::json!({
        "model": SMOKE_TARGET_MODEL,
        "input": [{
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": format!("{SMOKE_PROMPT}\n\nHarness marker: codex-router-s8-quota-client"),
            }],
        }],
        "stream": true,
    })
    .to_string();

    let mut last_error = None;
    for attempt in 0..2 {
        match run_s8_overlap_quota_local_probe_attempt(router_port, local_token, &request_payload) {
            Ok(()) => return Ok(()),
            Err(error) if attempt == 0 => {
                last_error = Some(error);
            }
            Err(error) => {
                return Err(format!(
                    "S8 overlap quota local probe failed after reconnect; first_error={} second_error={error}",
                    last_error.unwrap_or_else(|| "<none>".to_owned())
                ));
            }
        }
    }
    Err("S8 overlap quota local probe exited without an attempt".to_owned())
}

fn run_s8_overlap_quota_local_probe_attempt(
    router_port: u16,
    local_token: &str,
    request_payload: &str,
) -> Result<(), String> {
    let mut request = format!("ws://127.0.0.1:{router_port}/v1/responses")
        .into_client_request()
        .map_err(|error| format!("failed to build S8 overlap quota request: {error}"))?;
    let authorization = format!("Bearer {local_token}")
        .parse()
        .map_err(|error| format!("failed to build S8 overlap quota authorization: {error}"))?;
    request.headers_mut().insert("authorization", authorization);
    let (mut websocket, _response) = connect(request)
        .map_err(|error| format!("S8 overlap quota local probe connect failed: {error}"))?;
    let MaybeTlsStream::Plain(stream) = websocket.get_mut() else {
        return Err("S8 overlap quota local probe expected a plain loopback stream".to_owned());
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| format!("S8 overlap quota local probe read timeout failed: {error}"))?;
    websocket
        .send(Message::text(request_payload.to_owned()))
        .map_err(|error| format!("S8 overlap quota local probe send failed: {error}"))?;

    let mut saw_expected_text = false;
    let mut saw_completed = false;
    loop {
        match websocket.read() {
            Ok(Message::Text(text)) => {
                let text = text.to_string();
                saw_expected_text |= text.contains(SMOKE_EXPECTED_TEXT);
                saw_completed |= text.contains("response.completed");
                if saw_expected_text && saw_completed {
                    let _close_result = websocket.close(None);
                    return Ok(());
                }
            }
            Ok(Message::Binary(bytes)) => {
                let text = String::from_utf8(bytes.to_vec()).map_err(|error| {
                    format!("S8 overlap quota local probe binary frame was not UTF-8: {error}")
                })?;
                saw_expected_text |= text.contains(SMOKE_EXPECTED_TEXT);
                saw_completed |= text.contains("response.completed");
                if saw_expected_text && saw_completed {
                    let _close_result = websocket.close(None);
                    return Ok(());
                }
            }
            Ok(Message::Close(_)) => {
                return Err(format!(
                    "closed before completion; saw_expected_text={saw_expected_text} saw_completed={saw_completed}"
                ));
            }
            Ok(_other) => {}
            Err(error) => {
                return Err(format!(
                    "read failed before completion; saw_expected_text={saw_expected_text} saw_completed={saw_completed}: {error}"
                ));
            }
        }
    }
}

fn account_id(value: &str) -> Result<AccountId, String> {
    AccountId::new(value.to_owned()).map_err(|_| format!("invalid smoke account id: {value}"))
}

#[cfg(test)]
fn upstream_account_token() -> &'static str {
    "installed-smoke-upstream-token"
}

fn timestamp_millis() -> u128 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_millis(),
        Err(_) => 0,
    }
}

fn timestamp_seconds() -> u64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
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
        if std::env::var_os(RETAIN_SMOKE_ROOT_ENV).is_some() {
            eprintln!("retaining smoke temp root: {}", self.path.display());
            return;
        }
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::fs;
    use std::os::unix::fs as unix_fs;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::process::Output;

    use super::InstalledCodexSmokeMode;
    use super::MockHttpSseTranscript;
    use super::MockWebSocketTranscript;
    use super::RETAIN_SMOKE_ROOT_ENV;
    use super::RedactedTranscriptInput;
    use super::RouterAuditObservation;
    use super::RouterProcessObservation;
    use super::SMOKE_EXPECTED_TEXT;
    use super::SmokeContractAssertion;
    use super::SmokeQuotaStatus;
    use super::SmokeSeed;
    use super::SmokeTempRoot;

    use super::assert_codex_visible_output;
    use super::assert_redacted_three_websocket_payload;
    use super::assert_smoke_contract;
    use super::first_frame_shape_summary;
    use super::run_hostile_no_token_smoke;
    use super::run_installed_codex_http_sse_mock_smoke;
    use super::run_installed_codex_mock_smoke;
    use super::run_installed_codex_quota_reconnect_websocket_mock_smoke;
    use super::run_installed_codex_s8_overlap_quota_websocket_mock_smoke;
    use super::run_installed_codex_three_websocket_mock_e2e;
    use super::run_installed_codex_three_websocket_mock_soak;
    use super::run_installed_codex_websocket_mock_smoke;
    use super::run_with_timeout;
    use super::upstream_account_token;
    use super::validate_copied_dev_state_roots;
    use super::write_redacted_transcript;

    fn expect_string_error(result: Result<(), String>, context: &'static str) -> String {
        match result {
            Ok(()) => panic!("{context}"),
            Err(error) => error,
        }
    }

    fn success_status() -> ExitStatus {
        ExitStatus::from_raw(0)
    }

    fn valid_transcript(
        local_token_in_http_body: bool,
        local_token_in_first_frame: bool,
    ) -> MockWebSocketTranscript {
        let local_token = "local-token-canary";
        let first_frame = if local_token_in_first_frame {
            format!(
                r#"{{"model":"gpt-5.5","input":[{{"role":"user","content":[{{"type":"input_text","text":"hello"}}]}}],"stream":true,"token":"{local_token}"}}"#
            )
        } else {
            r#"{"model":"gpt-5.5","input":[{"role":"user","content":[{"type":"input_text","text":"hello"}]}],"stream":true}"#.to_owned()
        };
        MockWebSocketTranscript {
            headers: vec![(
                "authorization".to_owned(),
                format!("Bearer {}", upstream_account_token()),
            )],
            first_frame: first_frame.clone(),
            request_frames: vec![first_frame],
            websocket_request_frame_count: 1,
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

    fn valid_quota_status() -> SmokeQuotaStatus {
        SmokeQuotaStatus {
            table: "matches 5h weekly resets available routing next use\n".to_owned(),
            plain: "account\tstatus\t5h\tweekly\tresets available\trouting\tnext use\nmatches\tenabled\t██████████ 91% resets in 4h\t██████░░░░ 54% resets in 6d\t-\t✓ preferred 5h 91%\tnext\nresponses route\tnext: matches\twhy: ✓ preferred 5h 91%\n".to_owned(),
            json: r#"{"preferred_next_account_hash":"hash_matches","accounts":[{"account_hash":"hash_matches","safe_account_label":"matches","preferred_next":true}]}"#.to_owned(),
        }
    }

    fn valid_router_audit_observation() -> RouterAuditObservation {
        RouterAuditObservation {
            http_sse_local_auth_validated: true,
            websocket_local_auth_validated: true,
        }
    }

    #[test]
    fn websocket_scenario_all_runs_serial_concurrent_and_soak_filters() -> Result<(), String> {
        let script_path = super::workspace_root()?
            .join("tests")
            .join("smoke")
            .join("installed_codex_mock.sh");
        let script = fs::read_to_string(script_path)
            .map_err(|error| format!("failed to read smoke script: {error}"))?;

        assert!(script.contains(
            r#"elif [[ "${scenario}" == "all" && "${transport}" == "websocket" ]]; then"#
        ));
        assert!(script.contains(r#"run_test_filter "installed_codex_websocket_""#));
        assert!(script.contains(r#"run_test_filter "three_codex_websocket_concurrent_e2e_""#));
        assert!(
            script.contains(r#"run_three_websocket_soak_filter "three_codex_websocket_soak_""#)
        );
        assert!(script.contains(
            r#"run_s8_overlap_quota_filter "installed_codex_websocket_s8_overlap_quota_""#
        ));
        assert!(script.contains(
            r#"run_quota_reconnect_filter "installed_codex_websocket_quota_reconnect_""#
        ));
        assert!(script.contains(r#"smoke_target_model="gpt-5.4-mini""#));
        assert!(script.contains(r#"smoke_client_summary="3 concurrent clients""#));
        assert!(script.contains(r#"smoke_client_summary="1 client with quota reconnect""#));
        assert!(
            script.contains(r#"smoke_client_summary="3 concurrent clients with quota reconnect""#)
        );
        assert!(script.contains(r#"clients=%s"#));
        assert!(script.contains("bounded explicit exact-reply prompt"));
        assert!(
            script.contains("uses the existing codex CLI from PATH; it does not install Codex")
        );
        Ok(())
    }

    #[test]
    fn proxy_db_runtime_isolation_uses_copied_roots_and_blocks_false_receipts() -> Result<(), String>
    {
        let workspace_root = super::workspace_root()?;
        let proxy_script = fs::read_to_string(
            workspace_root
                .join("tests")
                .join("smoke")
                .join("proxy_db_runtime_isolation.sh"),
        )
        .map_err(|error| format!("failed to read proxy DB smoke script: {error}"))?;
        let validator_script = fs::read_to_string(
            workspace_root
                .join("scripts")
                .join("validate-proxy-db-runtime-isolation-artifact.py"),
        )
        .map_err(|error| format!("failed to read proxy DB artifact validator: {error}"))?;
        let installed_script = fs::read_to_string(
            workspace_root
                .join("tests")
                .join("smoke")
                .join("installed_codex_mock.sh"),
        )
        .map_err(|error| format!("failed to read installed Codex smoke script: {error}"))?;

        assert!(proxy_script.contains(r#"--runtime-root-mode copied-dev-state"#));
        assert!(proxy_script.contains(r#"--router-root "${router_root_resolved}""#));
        assert!(proxy_script.contains(r#"--codex-home "${codex_home_resolved}""#));
        assert!(proxy_script.contains(r#"--process-home "${home_resolved}""#));
        assert!(proxy_script.contains("validate-proxy-db-runtime-isolation-artifact.py"));
        assert!(proxy_script.contains(r#"--scenario s8-overlap-quota"#));
        assert!(!proxy_script.contains(r#"--scenario quota-reconnect"#));
        assert!(proxy_script.contains("installed-codex-s8-overlap-quota-artifact.txt"));
        assert!(!proxy_script.contains("${quota_artifact_path}"));
        assert!(validator_script.contains(r#"runtime_roots.get("mode") == "copied-dev-state""#));
        assert!(validator_script.contains(r#"pressure.get("copied_db_pressure_proven") is True"#));
        assert!(
            validator_script
                .contains(r#"pressure.get("sqlite_lock_or_maintenance_pressure") is True"#)
        );
        assert!(
            validator_script
                .contains(r#"signal_ordering.get("signal_before_persistence") is True"#)
        );
        assert!(validator_script.contains(r#"account_selection.get("non_reselection") is True"#));
        assert!(validator_script.contains(r#"router_signal_count"#));
        assert!(validator_script.contains(r#"source_artifacts_same_s8_run_id"#));
        assert!(proxy_script.contains(r#"status=BLOCKED"#));
        assert!(proxy_script.contains(r#"scrubbed_signal_log_path"#));
        assert!(proxy_script.contains(r#"CODEX_ROUTER_S8_RUN_ID"#));
        assert!(proxy_script.contains(r#"receipt_path_value()"#));
        assert!(!proxy_script.contains(r#"printf 'router_root=%s\n' "${router_root_resolved}""#));
        assert!(!proxy_script.contains(r#"printf 'router_db=%s\n' "${router_db}""#));
        assert!(!proxy_script.contains(r#"printf 'codex_home=%s\n' "${codex_home_resolved}""#));
        assert!(!proxy_script.contains(r#"printf 'codex_db=%s\n' "${codex_db}""#));
        assert!(!proxy_script.contains(r#"printf 'sentinel_home=%s\n' "${home_resolved}""#));
        assert!(validator_script.contains(r#"pass_count="#));
        assert!(validator_script.contains(r#"fail_count="#));
        assert!(installed_script.contains(r#"--runtime-root-mode"#));
        assert!(installed_script.contains(r#"CODEX_ROUTER_INSTALLED_SMOKE_RUNTIME_ROOT_MODE"#));
        assert!(installed_script.contains(r#"CODEX_ROUTER_INSTALLED_SMOKE_ROUTER_ROOT"#));
        assert!(installed_script.contains(r#"CODEX_ROUTER_INSTALLED_SMOKE_CODEX_HOME"#));
        assert!(installed_script.contains(r#"CODEX_ROUTER_INSTALLED_SMOKE_PROCESS_HOME"#));
        assert!(installed_script.contains(r#"CODEX_ROUTER_S8_RUN_ID"#));
        assert!(installed_script.contains("os.path.realpath"));
        assert!(!installed_script.contains("os.path.abspath(candidate)"));

        Ok(())
    }

    fn valid_router_process_observation(test_root: &SmokeTempRoot) -> RouterProcessObservation {
        RouterProcessObservation {
            binary_path: test_root.path().join("target/debug/codex-router"),
            pid: 42,
            argv: vec!["serve".to_owned(), "--port".to_owned(), "8787".to_owned()],
            listener: "127.0.0.1:8787".to_owned(),
            readiness_line: "listening: 127.0.0.1:8787".to_owned(),
            cleanup_result: "terminated:signal: 9 (SIGKILL)".to_owned(),
        }
    }

    #[test]
    fn smoke_contract_rejects_local_token_in_upstream_http_body() {
        let routable_upstream_tokens = [upstream_account_token().to_owned()];
        let quota_status = valid_quota_status();
        let upstream = valid_transcript(true, false);
        let error = match assert_smoke_contract(SmokeContractAssertion {
            mode: InstalledCodexSmokeMode::Combined,
            http_sse_codex_status: Some(&success_status()),
            websocket_codex_status: Some(&success_status()),
            upstream: &upstream,
            local_token: "local-token-canary",
            expected_account_label: "matches",
            expected_upstream_token: upstream_account_token(),
            routable_upstream_tokens: &routable_upstream_tokens,
            quota_status: &quota_status,
        }) {
            Ok(()) => panic!("HTTP/SSE body local-token leak must fail smoke contract"),
            Err(error) => error,
        };

        assert!(error.contains("HTTP/SSE request body leaked local router token"));
    }

    #[test]
    fn smoke_contract_rejects_local_token_in_upstream_websocket_frame() {
        let routable_upstream_tokens = [upstream_account_token().to_owned()];
        let quota_status = valid_quota_status();
        let upstream = valid_transcript(false, true);
        let error = match assert_smoke_contract(SmokeContractAssertion {
            mode: InstalledCodexSmokeMode::Combined,
            http_sse_codex_status: Some(&success_status()),
            websocket_codex_status: Some(&success_status()),
            upstream: &upstream,
            local_token: "local-token-canary",
            expected_account_label: "matches",
            expected_upstream_token: upstream_account_token(),
            routable_upstream_tokens: &routable_upstream_tokens,
            quota_status: &quota_status,
        }) {
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
    fn mock_http_reader_consumes_chunked_request_body_before_responding() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("failed to bind chunked request fixture: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("failed to read chunked fixture address: {error}"));
        let client = std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(address)
                .unwrap_or_else(|error| panic!("failed to connect chunked fixture: {error}"));
            std::io::Write::write_all(
                &mut stream,
                b"POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
            )
            .unwrap_or_else(|error| panic!("failed to write chunked fixture: {error}"));
        });
        let (mut stream, _peer) = listener
            .accept()
            .unwrap_or_else(|error| panic!("failed to accept chunked fixture: {error}"));

        let request = super::read_http_request(&mut stream)
            .unwrap_or_else(|error| panic!("failed to read chunked request: {error}"));

        assert_eq!(request.request_line, "POST /v1/responses HTTP/1.1");
        assert_eq!(request.body, "hello world");
        client
            .join()
            .unwrap_or_else(|error| panic!("chunked request client panicked: {error:?}"));
    }

    #[test]
    fn timed_out_codex_output_suppresses_captured_stdout_stderr() {
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg(
                "printf 'local-secret-canary shell_command'; printf 'workdir: /tmp/raw-path session id: raw-session-id user prompt-canary diagnostic: waiting on loopback closed connection function_call_output' >&2; sleep 2",
            )
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let error = match run_with_timeout(command, std::time::Duration::from_millis(250)) {
            Ok(_) => panic!("sleeping command must time out"),
            Err(error) => error,
        };

        assert!(!error.contains("local-secret-canary"));
        assert!(!error.contains("raw-path"));
        assert!(!error.contains("raw-session-id"));
        assert!(!error.contains("prompt-canary"));
        assert!(!error.contains("waiting on loopback"));
        assert!(error.contains("captured stdout/stderr suppressed"));
        assert!(error.contains("stdout_preview=<suppressed>"));
        assert!(error.contains("stderr_preview=<suppressed>"));
        assert!(error.contains("stdout_markers=shell_command"));
        assert!(
            error.contains(
                "stderr_markers=closed_connection,waiting_on_loopback,function_call_output"
            )
        );
    }

    #[test]
    fn run_with_timeout_drains_child_stderr_while_waiting() {
        let mut command = std::process::Command::new("python3");
        command
            .arg("-c")
            .arg("import sys; sys.stderr.write('x' * 200000); sys.stderr.flush()")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = match run_with_timeout(command, std::time::Duration::from_secs(2)) {
            Ok(output) => output,
            Err(error) => panic!("child output pipes should be drained while waiting: {error}"),
        };

        assert!(
            output.status.success(),
            "stderr writer should exit cleanly: {}",
            output.status
        );
        assert_eq!(output.stderr.len(), 200000);
    }

    #[test]
    fn copied_dev_state_roots_reject_live_style_paths_before_mutation() {
        let test_root = SmokeTempRoot::new("copied-dev-state-live-style-rejection")
            .unwrap_or_else(|error| panic!("failed to create temp root: {error}"));
        let live_style_home = test_root.path().join("live-style-home");
        let router_root = live_style_home.join(".codex-router");
        let codex_home = live_style_home.join(".codex");

        let error = expect_string_error(
            validate_copied_dev_state_roots(&router_root, &codex_home, &live_style_home),
            "live-style copied-dev-state roots must be rejected",
        );

        assert!(
            error.contains("tmp/dev-state"),
            "error should tell operators to use repo-local tmp/dev-state roots: {error}"
        );
        assert!(
            !router_root.exists(),
            "unsafe router root must be rejected before directory creation"
        );
        assert!(
            !codex_home.exists(),
            "unsafe Codex home must be rejected before directory creation"
        );
    }

    #[test]
    fn copied_dev_state_roots_reject_symlinked_roots_before_mutation() {
        let workspace_root =
            super::workspace_root().unwrap_or_else(|error| panic!("workspace root: {error}"));
        let dev_state_root = workspace_root.join("tmp/dev-state");
        fs::create_dir_all(&dev_state_root)
            .unwrap_or_else(|error| panic!("failed to create dev-state fixture root: {error}"));
        let test_root = SmokeTempRoot::new("copied-dev-state-symlink-root-rejection")
            .unwrap_or_else(|error| panic!("failed to create temp root: {error}"));
        let external_router_target = test_root.path().join("external-router");
        fs::create_dir_all(&external_router_target)
            .unwrap_or_else(|error| panic!("failed to create external router target: {error}"));
        let router_root = dev_state_root.join(format!("symlink-router-{}", std::process::id()));
        let _ = fs::remove_file(&router_root);
        unix_fs::symlink(&external_router_target, &router_root)
            .unwrap_or_else(|error| panic!("failed to create router symlink fixture: {error}"));
        let codex_home = dev_state_root.join(format!("codex-home-{}", std::process::id()));
        let process_home = dev_state_root.join(format!("process-home-{}", std::process::id()));

        let error = expect_string_error(
            validate_copied_dev_state_roots(&router_root, &codex_home, &process_home),
            "symlinked router root must be rejected",
        );

        assert!(
            error.contains("symlink") || error.contains("tmp/dev-state"),
            "error should explain copied-dev-state symlink/root rejection: {error}"
        );
        fs::remove_file(&router_root)
            .unwrap_or_else(|error| panic!("failed to clean router symlink fixture: {error}"));
    }

    #[test]
    fn copied_dev_state_roots_reject_symlinked_db_and_secret_targets_before_mutation() {
        let workspace_root =
            super::workspace_root().unwrap_or_else(|error| panic!("workspace root: {error}"));
        let dev_state_root = workspace_root.join("tmp/dev-state");
        fs::create_dir_all(&dev_state_root)
            .unwrap_or_else(|error| panic!("failed to create dev-state fixture root: {error}"));
        let test_root = SmokeTempRoot::new("copied-dev-state-symlink-target-rejection")
            .unwrap_or_else(|error| panic!("failed to create temp root: {error}"));
        let fixture_suffix = format!("targets-{}", std::process::id());
        let router_root = dev_state_root.join(format!("router-{fixture_suffix}"));
        let codex_home = dev_state_root.join(format!("codex-{fixture_suffix}"));
        let process_home = dev_state_root.join(format!("home-{fixture_suffix}"));
        fs::create_dir_all(&router_root)
            .unwrap_or_else(|error| panic!("failed to create router root fixture: {error}"));
        fs::create_dir_all(&codex_home)
            .unwrap_or_else(|error| panic!("failed to create codex home fixture: {error}"));
        fs::create_dir_all(&process_home)
            .unwrap_or_else(|error| panic!("failed to create process home fixture: {error}"));
        let external_router_db = test_root.path().join("external-state.sqlite");
        let external_codex_db = test_root.path().join("external-state_5.sqlite");
        let external_secrets = test_root.path().join("external-secrets");
        fs::write(&external_router_db, b"outside router db")
            .unwrap_or_else(|error| panic!("failed to write external router DB: {error}"));
        fs::write(&external_codex_db, b"outside codex db")
            .unwrap_or_else(|error| panic!("failed to write external Codex DB: {error}"));
        fs::create_dir_all(&external_secrets)
            .unwrap_or_else(|error| panic!("failed to create external secrets dir: {error}"));
        let _ = fs::remove_file(router_root.join("state.sqlite"));
        let _ = fs::remove_file(codex_home.join("state_5.sqlite"));
        let _ = fs::remove_file(router_root.join("secrets"));
        unix_fs::symlink(&external_router_db, router_root.join("state.sqlite"))
            .unwrap_or_else(|error| panic!("failed to create router DB symlink: {error}"));
        unix_fs::symlink(&external_codex_db, codex_home.join("state_5.sqlite"))
            .unwrap_or_else(|error| panic!("failed to create Codex DB symlink: {error}"));
        unix_fs::symlink(&external_secrets, router_root.join("secrets"))
            .unwrap_or_else(|error| panic!("failed to create secrets symlink: {error}"));

        let error = expect_string_error(
            validate_copied_dev_state_roots(&router_root, &codex_home, &process_home),
            "symlinked copied-dev-state DB/secret targets must be rejected",
        );

        assert!(
            error.contains("symlink") || error.contains("tmp/dev-state"),
            "error should explain copied-dev-state symlink target rejection: {error}"
        );
        fs::remove_file(router_root.join("state.sqlite"))
            .unwrap_or_else(|error| panic!("failed to clean router DB symlink: {error}"));
        fs::remove_file(codex_home.join("state_5.sqlite"))
            .unwrap_or_else(|error| panic!("failed to clean Codex DB symlink: {error}"));
        fs::remove_file(router_root.join("secrets"))
            .unwrap_or_else(|error| panic!("failed to clean secrets symlink: {error}"));
    }

    #[test]
    fn child_timeout_diagnostics_report_last_message_shape_without_content_or_path() {
        let test_root = SmokeTempRoot::new("child-timeout-diagnostics")
            .unwrap_or_else(|error| panic!("failed to create temp root: {error}"));
        let last_message_path = test_root.path().join("last-message.txt");
        fs::write(&last_message_path, SMOKE_EXPECTED_TEXT)
            .unwrap_or_else(|error| panic!("failed to write last message: {error}"));

        let diagnostics = super::codex_child_timeout_diagnostics(Some(2), &last_message_path);

        assert!(diagnostics.contains("client_index:2"));
        assert!(diagnostics.contains("last_message_exists:true"));
        assert!(diagnostics.contains("last_message_bytes:21"));
        assert!(diagnostics.contains("last_message_contains_expected:true"));
        assert!(!diagnostics.contains(SMOKE_EXPECTED_TEXT));
        assert!(!diagnostics.contains(&last_message_path.display().to_string()));
    }

    #[test]
    fn smoke_temp_root_is_retained_when_explicitly_requested() {
        if std::env::var_os(RETAIN_SMOKE_ROOT_ENV).is_none() {
            eprintln!("skipping retained-root assertion; set {RETAIN_SMOKE_ROOT_ENV}=1 to run it");
            return;
        }
        let test_root = match SmokeTempRoot::new("retain-fixture") {
            Ok(test_root) => test_root,
            Err(error) => panic!("failed to create temp root: {error}"),
        };
        let retained_path = test_root.path().to_path_buf();

        drop(test_root);

        assert!(
            retained_path.exists(),
            "smoke temp root should remain when CODEX_ROUTER_RETAIN_SMOKE_ROOT=1"
        );
        fs::remove_dir_all(&retained_path).unwrap_or_else(|error| {
            panic!(
                "failed to clean retained fixture {}: {error}",
                retained_path.display()
            )
        });
    }

    #[test]
    fn redacted_transcript_omits_forbidden_request_canaries() {
        let test_root = match SmokeTempRoot::new("redacted-transcript") {
            Ok(test_root) => test_root,
            Err(error) => panic!("failed to create temp root: {error}"),
        };
        let http_sse_last_message_path = test_root.path().join("http-sse-last-message.txt");
        let websocket_last_message_path = test_root.path().join("websocket-last-message.txt");
        let upstream = MockWebSocketTranscript {
            headers: vec![(
                "authorization".to_owned(),
                "Bearer installed-smoke-matches-token".to_owned(),
            )],
            first_frame: r#"{"type":"response.create","model":"gpt-5.5","input":[{"role":"user","content":[{"type":"input_text","text":"prompt-canary"}]}],"stream":true,"previous_response_id":"raw-previous-response-id-canary"}"#.to_owned(),
            request_frames: Vec::new(),
            websocket_request_frame_count: 1,
            http_probe_count: 0,
            http_sse: Some(MockHttpSseTranscript {
                request_line: "POST /v1/responses HTTP/1.1".to_owned(),
                headers: vec![(
                    "authorization".to_owned(),
                    "Bearer installed-smoke-matches-token".to_owned(),
                )],
                body: r#"{"stream":true,"input":"prompt-canary"}"#.to_owned(),
            }),
        };
        let transcript_path = match write_redacted_transcript(RedactedTranscriptInput {
            mode: InstalledCodexSmokeMode::Combined,
            codex_version: "OpenAI Codex v0.test",
            profile_path: test_root.path(),
            http_sse_codex_status: Some(&success_status()),
            http_sse_codex_stdout: Some(Cow::Borrowed("codex-router smoke ok")),
            http_sse_codex_stderr: Some(Cow::Borrowed("")),
            http_sse_last_message_path: Some(&http_sse_last_message_path),
            websocket_codex_status: Some(&success_status()),
            websocket_codex_stdout: Some(Cow::Borrowed("codex-router smoke ok")),
            websocket_codex_stderr: Some(Cow::Borrowed("")),
            websocket_last_message_path: Some(&websocket_last_message_path),
            upstream: &upstream,
            quota_status: &valid_quota_status(),
            expected_account_label: "matches",
            expected_upstream_token: upstream_account_token(),
            router_process: &valid_router_process_observation(&test_root),
            router_audit: &valid_router_audit_observation(),
        }) {
            Ok(path) => path,
            Err(error) => panic!("redacted transcript fixture failed: {error}"),
        };
        let payload = match fs::read_to_string(&transcript_path) {
            Ok(payload) => payload,
            Err(error) => panic!("failed to read transcript fixture: {error}"),
        };

        for forbidden in [
            "first_frame_model",
            "first_frame_has_input",
            "first_frame_stream",
            "gpt-5.5",
            "prompt-canary",
            "raw-previous-response-id-canary",
            "installed-smoke-matches-token",
        ] {
            assert!(
                !payload.contains(forbidden),
                "redacted transcript leaked {forbidden}"
            );
        }
        assert!(payload.contains("first_frame_shape"));
    }

    #[test]
    fn three_websocket_artifact_rejects_raw_session_and_local_port_fields() {
        let seed = SmokeSeed {
            local_token_assignment: "CODEX_ROUTER_TOKEN=local-secret-canary".to_owned(),
            local_token: "local-secret-canary".to_owned(),
            expected_upstream_token: "upstream-secret-canary".to_owned(),
            expected_account_tag: "safe-tag".to_owned(),
            expected_account_label: "unsafe:raw-account-label".to_owned(),
            routable_upstream_tokens: vec!["upstream-secret-canary".to_owned()],
            quota_status: valid_quota_status(),
        };
        let payload = serde_json::json!({
            "router_process": {
                "listener": "127.0.0.1:43210",
                "readiness_line": "listening: 127.0.0.1:43210",
                "argv": ["serve", "--port", "43210"],
            },
            "router_websocket_registry": {
                "registered_session_ids": [1, 2, 3],
                "session_peer_addrs": [{"session_id": 1, "local_port": 60001}],
            },
            "runtime_correlations": [{
                "router_session_id": 1,
                "upstream_session_id": 10,
            }],
            "session_continuity": {
                "per_client_join_keys": [{
                    "router_session_id": 1,
                    "upstream_session_id": 10,
                }],
                "router_registered_session_ids": [1, 2, 3],
                "upstream_session_ids": [10, 11, 12],
            },
            "upstream": {
                "upstream_session_ids": [10, 11, 12],
                "upstream_client_sessions": [{"client_index": 0, "upstream_session_id": 10}],
            },
        });

        let error = expect_string_error(
            assert_redacted_three_websocket_payload(&payload.to_string(), &[], &seed),
            "raw session identifiers and local ports must be rejected",
        );

        assert!(
            error.contains("forbidden structural key")
                || error.contains("forbidden fragment")
                || error.contains("loopback endpoint with numeric port"),
            "unexpected redaction error: {error}"
        );
    }

    #[test]
    fn persisted_websocket_registry_report_accepts_sanitized_schema_without_raw_identifiers()
    -> Result<(), String> {
        let test_root = SmokeTempRoot::new("sanitized-registry-report")?;
        let report_path = test_root.path().join("websocket-registry-report.json");
        let report_json = serde_json::json!({
            "schema_version": 2,
            "handled_connections": 3,
            "websocket_registry": {
                "active_sessions": 0,
                "high_water_sessions": 3,
                "registered_sessions": 3,
                "closed_sessions": 3,
                "completed_response_sessions": 3,
                "forwarded_upstream_messages": 9,
                "registered_session_id_count": 3,
                "completed_session_id_count": 3,
                "closed_session_id_count": 3,
                "session_peer_addr_count": 3,
                "session_peer_join_observable": true,
                "completed_session_forwarded_upstream_message_counts": [3, 3, 3],
                "final_session_forwarded_upstream_message_counts": [3, 3, 3],
                "quota_reconnect_signal_count": 1,
                "quota_reconnect_signal_unix_ms": 1_720_000_000_000u64,
            }
        });
        let rendered = serde_json::to_string_pretty(&report_json)
            .map_err(|error| format!("failed to render sanitized registry report: {error}"))?;
        fs::write(&report_path, &rendered)
            .map_err(|error| format!("failed to write sanitized registry report: {error}"))?;

        for forbidden_key in [
            "registered_session_ids",
            "completed_session_ids",
            "closed_session_ids",
            "session_peer_addrs",
            "session_id",
            "peer_addr",
            "local_port",
        ] {
            assert!(
                !rendered.contains(&format!("\"{forbidden_key}\"")),
                "sanitized persisted registry report leaked raw key {forbidden_key}"
            );
        }
        assert!(
            !rendered.contains("127.0.0.1:") && !rendered.contains("[::1]:"),
            "sanitized persisted registry report leaked a raw loopback peer port"
        );

        let _report = super::RouterWebSocketRegistryReport::from_file(&report_path)?;

        Ok(())
    }

    #[test]
    #[ignore = "T8a inventory preflight; route-native proof belongs to the next route-native slice"]
    fn route_native_harness_inventory_preflight() {
        let first_frame = serde_json::json!({
            "type": "response.create",
            "model": "gpt-5.5",
            "input": [{"role": "user", "content": [{"type": "input_text", "text": "prompt-canary"}]}],
            "stream": true
        });
        let summary = first_frame_shape_summary(&first_frame);

        assert_eq!(
            summary.get("json_object").and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            summary
                .get("non_prewarm_response_create")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert!(!summary.to_string().contains("prompt-canary"));
        assert!(!summary.to_string().contains("gpt-5.5"));
    }

    #[test]
    #[ignore = "T8a inventory preflight; run full HTTP/SSE smoke through tests/smoke/installed_codex_mock.sh --transport http-sse"]
    fn installed_codex_http_sse_harness_inventory_preflight() {
        let routable_upstream_tokens = [upstream_account_token().to_owned()];
        let quota_status = valid_quota_status();
        let upstream = valid_transcript(false, false);

        if let Err(error) = assert_smoke_contract(SmokeContractAssertion {
            mode: InstalledCodexSmokeMode::Combined,
            http_sse_codex_status: Some(&success_status()),
            websocket_codex_status: Some(&success_status()),
            upstream: &upstream,
            local_token: "local-token-canary",
            expected_account_label: "matches",
            expected_upstream_token: upstream_account_token(),
            routable_upstream_tokens: &routable_upstream_tokens,
            quota_status: &quota_status,
        }) {
            panic!("HTTP/SSE harness preflight failed: {error}");
        }
    }

    #[test]
    #[ignore = "T9 installed-Codex HTTP/SSE e2e; run through tests/smoke/installed_codex_mock.sh --transport http-sse"]
    fn installed_codex_http_sse_e2e_exercises_generated_profile_token() {
        let report = match run_installed_codex_http_sse_mock_smoke() {
            Ok(report) => report,
            Err(error) => panic!("installed Codex HTTP/SSE e2e failed: {error}"),
        };

        assert!(report.transcript_path().exists());
        println!(
            "codex_router_installed_codex_artifact={}",
            report.transcript_path().display()
        );
    }

    #[test]
    #[ignore = "T8a inventory preflight; run full WebSocket smoke through tests/smoke/installed_codex_mock.sh --transport websocket"]
    fn installed_codex_websocket_harness_inventory_preflight() {
        let routable_upstream_tokens = [upstream_account_token().to_owned()];
        let quota_status = valid_quota_status();
        let upstream = valid_transcript(false, false);

        if let Err(error) = assert_smoke_contract(SmokeContractAssertion {
            mode: InstalledCodexSmokeMode::Combined,
            http_sse_codex_status: Some(&success_status()),
            websocket_codex_status: Some(&success_status()),
            upstream: &upstream,
            local_token: "local-token-canary",
            expected_account_label: "matches",
            expected_upstream_token: upstream_account_token(),
            routable_upstream_tokens: &routable_upstream_tokens,
            quota_status: &quota_status,
        }) {
            panic!("WebSocket harness preflight failed: {error}");
        }
    }

    #[test]
    #[ignore = "T10 installed-Codex WebSocket e2e; run through tests/smoke/installed_codex_mock.sh --transport websocket"]
    fn installed_codex_websocket_e2e_exercises_generated_profile_token() {
        let report = match run_installed_codex_websocket_mock_smoke() {
            Ok(report) => report,
            Err(error) => panic!("installed Codex WebSocket e2e failed: {error}"),
        };

        assert!(report.transcript_path().exists());
        println!(
            "codex_router_installed_codex_artifact={}",
            report.transcript_path().display()
        );
    }

    #[test]
    #[ignore = "T8 installed-Codex concurrent WebSocket e2e; run through tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent"]
    fn three_codex_websocket_concurrent_e2e_shares_router_pid_and_overlaps() {
        let report = match run_installed_codex_three_websocket_mock_e2e() {
            Ok(report) => report,
            Err(error) => panic!("installed Codex concurrent WebSocket e2e failed: {error}"),
        };

        assert!(report.transcript_path().exists());
        println!(
            "codex_router_three_websocket_artifact={}",
            report.transcript_path().display()
        );
    }

    #[test]
    #[ignore = "T8 installed-Codex five-minute WebSocket soak; run through tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak"]
    fn three_codex_websocket_soak_holds_overlap_and_records_activity() {
        let report = match run_installed_codex_three_websocket_mock_soak() {
            Ok(report) => report,
            Err(error) => panic!("installed Codex concurrent WebSocket soak failed: {error}"),
        };

        assert!(report.transcript_path().exists());
        println!(
            "codex_router_three_websocket_artifact={}",
            report.transcript_path().display()
        );
    }

    #[test]
    #[ignore = "S8 installed-Codex WebSocket overlap quota proof; run through tests/smoke/installed_codex_mock.sh --transport websocket --scenario s8-overlap-quota"]
    fn installed_codex_websocket_s8_overlap_quota_reconnects_during_three_client_overlap() {
        let report = match run_installed_codex_s8_overlap_quota_websocket_mock_smoke() {
            Ok(report) => report,
            Err(error) => panic!("installed Codex S8 overlap quota smoke failed: {error}"),
        };

        assert!(report.transcript_path().exists());
        println!(
            "codex_router_s8_overlap_quota_artifact={}",
            report.transcript_path().display()
        );
    }

    #[test]
    #[ignore = "T8 installed-Codex quota reconnect WebSocket e2e; run through tests/smoke/installed_codex_mock.sh --transport websocket --scenario quota-reconnect"]
    fn installed_codex_websocket_quota_reconnect_e2e_switches_account_and_completes() {
        let report = match run_installed_codex_quota_reconnect_websocket_mock_smoke() {
            Ok(report) => report,
            Err(error) => panic!("installed Codex quota reconnect WebSocket e2e failed: {error}"),
        };

        assert!(report.transcript_path().exists());
        println!(
            "codex_router_quota_reconnect_artifact={}",
            report.transcript_path().display()
        );
    }

    #[test]
    #[ignore = "run through tests/smoke/installed_codex_mock.sh"]
    fn installed_codex_mock_smoke_exercises_generated_profile_token_and_websocket() {
        let report = match run_installed_codex_mock_smoke() {
            Ok(report) => report,
            Err(error) => panic!("installed Codex smoke failed: {error}"),
        };

        assert!(report.transcript_path().exists());
        println!(
            "codex_router_installed_codex_artifact={}",
            report.transcript_path().display()
        );
    }

    #[test]
    #[ignore = "run through tests/smoke/installed_codex_mock.sh"]
    fn installed_codex_hostile_no_token_smoke_keeps_upstream_empty() {
        if let Err(error) = run_hostile_no_token_smoke() {
            panic!("hostile no-token smoke failed: {error}");
        }
    }
}
