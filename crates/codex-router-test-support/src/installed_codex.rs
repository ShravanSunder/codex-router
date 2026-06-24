//! Installed Codex smoke harness.

use std::borrow::Cow;
use std::collections::BTreeMap;
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
use codex_router_cli::profile::CodexRouterProfile;
use codex_router_cli::profile::CodexRouterProfileWriter;
use codex_router_cli::run_with_io;
use codex_router_cli::token::LocalRouterTokenService;
use codex_router_cli::token::Shell;
use codex_router_cli::token::export_token_assignment;
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
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use tungstenite::Message;
use tungstenite::WebSocket;
use tungstenite::accept_hdr;
use tungstenite::client::IntoClientRequest;
use tungstenite::connect;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;

const SMOKE_EXPECTED_TEXT: &str = "codex-router smoke ok";
const CODEX_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_ACCEPT_TIMEOUT: Duration = Duration::from_secs(35);
const DEFAULT_SOAK_DURATION: Duration = Duration::from_secs(300);
const SOAK_COMMAND_TIMEOUT_SLACK: Duration = Duration::from_secs(90);
const SOAK_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const SOAK_PROOF_MARGIN: Duration = Duration::from_secs(1);
const ROUTER_REGISTRY_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);

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
    router_max_upstream_messages: usize,
    router_max_connections: usize,
    capture_registry_report: bool,
}

impl ConcurrentWebSocketHarnessConfig {
    const fn quick() -> Self {
        Self {
            artifact_mode: "three-websocket",
            upstream: ConcurrentUpstreamConfig::quick(3),
            codex_command_timeout: CODEX_COMMAND_TIMEOUT,
            router_max_upstream_messages: 4,
            router_max_connections: 3,
            capture_registry_report: false,
        }
    }

    fn soak() -> Self {
        let hold_duration = soak_duration_from_env();
        Self {
            artifact_mode: "three-websocket-soak",
            upstream: ConcurrentUpstreamConfig::soak(3, hold_duration),
            codex_command_timeout: hold_duration.saturating_add(SOAK_COMMAND_TIMEOUT_SLACK),
            router_max_upstream_messages: 64,
            router_max_connections: 3,
            capture_registry_report: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConcurrentUpstreamConfig {
    expected_sessions: usize,
    hold_duration: Duration,
    heartbeat_interval: Duration,
}

impl ConcurrentUpstreamConfig {
    const fn quick(expected_sessions: usize) -> Self {
        Self {
            expected_sessions,
            hold_duration: Duration::ZERO,
            heartbeat_interval: SOAK_HEARTBEAT_INTERVAL,
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
    let profile_preview = profile_writer
        .dry_run(&profile)
        .map_err(|error| format!("failed to preview generated Codex profile: {error}"))?;
    let profile_path = profile_writer
        .write(&profile, true, Some(profile_preview.preview_token()))
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
        assert_codex_visible_output("HTTP/SSE", &output, &http_sse_last_message_path)?;
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
    let router_root = smoke_root.path().join("router");
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    fs::create_dir_all(&router_root).map_err(|error| {
        format!(
            "failed to create temp router root {}: {error}",
            router_root.display()
        )
    })?;

    let codex_version = command_output_text(Command::new("codex").arg("--version"))?;
    let upstream = MockConcurrentWebSocketUpstream::start(config.upstream)?;
    let seed = seed_router_state(&state_path, &secret_root)?;
    let router_port = reserve_loopback_port()?;
    let audit_path = router_root.join("audit").join("events.jsonl");
    let registry_report_path = config
        .capture_registry_report
        .then(|| router_root.join("websocket-registry-report.json"));
    let router_process = start_router_process_with_options(RouterProcessStartOptions {
        router_port,
        state_path,
        secret_root,
        local_token: None,
        upstream_base_url: format!("http://{}/v1", upstream.address()),
        audit_path,
        max_websocket_upstream_messages: config.router_max_upstream_messages,
        max_connections: config.router_max_connections,
        websocket_registry_report_file: registry_report_path.clone(),
    })?;

    let start_barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for client_index in 0..3 {
        let client_root = smoke_root.path().join(format!("client-{client_index}"));
        let codex_home = client_root.join("codex-home");
        let workdir = client_root.join("workdir");
        let process_home = client_root.join("home");
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
        let profile_preview = profile_writer.dry_run(&profile).map_err(|error| {
            format!("failed to preview generated Codex profile for client {client_index}: {error}")
        })?;
        profile_writer
            .write(&profile, true, Some(profile_preview.preview_token()))
            .map_err(|error| {
                format!(
                    "failed to write generated Codex profile for client {client_index}: {error}"
                )
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
                    let output = run_codex_exec_with_timeout(
                        CodexTransportMode::WebSocket,
                        &codex_home,
                        &workdir,
                        &last_message_path,
                        child_environment,
                        config.codex_command_timeout,
                    )?;
                    assert_codex_visible_output(
                        &format!("WebSocket client {client_index}"),
                        &output,
                        &last_message_path,
                    )?;
                    Ok::<Output, String>(output)
                })
                .map_err(|error| {
                    format!("failed to spawn installed Codex client {client_index}: {error}")
                })?,
        );
    }

    let mut outputs = Vec::new();
    for (client_index, handle) in handles.into_iter().enumerate() {
        let output = join_result(handle, &format!("installed Codex client {client_index}"))?;
        outputs.push(output);
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
        })?;

    Ok(InstalledCodexSmokeReport { transcript_path })
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
    if upstream.completed_sessions != config.upstream.expected_sessions {
        return Err(format!(
            "concurrent upstream completed {} sessions, expected {}",
            upstream.completed_sessions, config.upstream.expected_sessions
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
    if config.upstream.hold_duration > Duration::ZERO {
        if upstream.real_overlap_duration_ms < config.upstream.hold_duration.as_millis() {
            return Err(format!(
                "soak real overlap duration was {}ms, expected at least {}ms",
                upstream.real_overlap_duration_ms,
                config.upstream.hold_duration.as_millis()
            ));
        }
        if upstream
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
        if upstream.normal_close_sessions < config.upstream.expected_sessions
            || upstream.abnormal_close_sessions != 0
        {
            return Err(format!(
                "soak close outcomes were {:?}; normal={} abnormal={} expected all {} normal",
                upstream.session_close_outcomes,
                upstream.normal_close_sessions,
                upstream.abnormal_close_sessions,
                config.upstream.expected_sessions
            ));
        }
        if !upstream.multi_step_interleave_completed {
            return Err("soak did not complete a multi-step WebSocket interleave".to_owned());
        }
        if upstream.multi_step_followup_frame_count == 0 {
            return Err(
                "soak did not observe a follow-up local frame before completion".to_owned(),
            );
        }
        if upstream.multi_step_followup_active_session_count < config.upstream.expected_sessions {
            return Err(format!(
                "multi-step follow-up saw {} active sessions, expected at least {}",
                upstream.multi_step_followup_active_session_count,
                config.upstream.expected_sessions
            ));
        }
        if !upstream.multi_step_completed_before_overlap_end {
            return Err(
                "multi-step WebSocket interleave did not complete before true 3-way overlap ended"
                    .to_owned(),
            );
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

    for fixture in SMOKE_ACCOUNT_FIXTURES {
        seed_smoke_account(&state, &secrets, *fixture)?;
    }

    let quota_status = capture_quota_status(state_path)?;
    let selected_account_id = selected_account_id_from_status_json(&quota_status.json)?;
    let selected_fixture = SMOKE_ACCOUNT_FIXTURES
        .iter()
        .find(|fixture| fixture.account_id == selected_account_id)
        .ok_or_else(|| {
            format!("quota status selected unknown smoke account: {selected_account_id}")
        })?;

    Ok(SmokeSeed {
        local_token_assignment,
        local_token: exported_token,
        expected_account_label: selected_fixture.label.to_owned(),
        expected_upstream_token: selected_fixture.upstream_token.to_owned(),
        routable_upstream_tokens: SMOKE_ACCOUNT_FIXTURES
            .iter()
            .filter(|fixture| fixture.weekly_status == SelectorQuotaWindowStatus::Eligible)
            .map(|fixture| fixture.upstream_token.to_owned())
            .collect(),
        quota_status,
    })
}

fn seed_smoke_account(
    state: &SqliteStateStore,
    secrets: &FileSecretStore,
    fixture: SmokeAccountFixture,
) -> Result<(), String> {
    let account_id = account_id(fixture.account_id)?;
    let account = AccountRecord::new(account_id.clone(), fixture.label, AccountStatus::Enabled)
        .with_active_credential_generation(1);
    AccountStateRepository::upsert_account(state, &account)
        .map_err(|error| format!("failed to seed smoke account {}: {error}", fixture.label))?;
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", fixture.short_remaining)
            .with_reset_unix_seconds(fixture.short_reset);
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
    .with_reset_unix_seconds(fixture.short_reset)
    .with_effective(true)
    .with_observed_unix_seconds(1_000);
    SelectorQuotaRepository::upsert_selector_window(state, &short_window).map_err(|error| {
        format!(
            "failed to seed smoke short selector window for {}: {error}",
            fixture.label
        )
    })?;
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        604_800,
        fixture.weekly_status,
    )
    .with_remaining_headroom(fixture.weekly_remaining)
    .with_reset_unix_seconds(fixture.weekly_reset)
    .with_observed_unix_seconds(1_000);
    SelectorQuotaRepository::upsert_selector_window(state, &weekly_window).map_err(|error| {
        format!(
            "failed to seed smoke weekly selector window for {}: {error}",
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

fn selected_account_id_from_status_json(payload: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(payload)
        .map_err(|error| format!("quota status json was invalid: {error}"))?;
    value
        .get("preferred_next_account_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "quota status json did not name preferred account".to_owned())
}

fn selected_account_label_from_status_json(payload: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(payload)
        .map_err(|error| format!("quota status json was invalid: {error}"))?;
    let selected_account_id = value
        .get("preferred_next_account_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "quota status json did not name preferred account".to_owned())?;
    value
        .get("accounts")
        .and_then(Value::as_array)
        .and_then(|accounts| {
            accounts.iter().find_map(|account| {
                let account_id = account.get("account_id").and_then(Value::as_str)?;
                if account_id == selected_account_id {
                    account
                        .get("safe_account_label")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| "quota status json did not include selected account label".to_owned())
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
        router_port,
        state_path,
        secret_root,
        local_token,
        upstream_base_url,
        audit_path,
        max_websocket_upstream_messages: 4,
        max_connections: 64,
        websocket_registry_report_file: None,
    })
}

struct RouterProcessStartOptions {
    router_port: u16,
    state_path: PathBuf,
    secret_root: PathBuf,
    local_token: Option<String>,
    upstream_base_url: String,
    audit_path: PathBuf,
    max_websocket_upstream_messages: usize,
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
        "--now-unix-seconds".to_owned(),
        "1030".to_owned(),
        "--max-snapshot-age-seconds".to_owned(),
        "60".to_owned(),
        "--disable-background-quota-refresh".to_owned(),
        "--max-websocket-upstream-messages".to_owned(),
        options.max_websocket_upstream_messages.to_string(),
        "--max-connections".to_owned(),
        options.max_connections.to_string(),
        "--audit-file".to_owned(),
        options.audit_path.display().to_string(),
    ];
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }

    run_with_timeout(command, timeout)
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{label} smoke stdout did not contain expected response text; status={}; stdout_preview={}; stderr_preview={}",
            output.status,
            redacted_process_output_preview(&stdout),
            redacted_process_output_preview(&stderr),
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
    const MAX_CHARS: usize = 1200;
    let mut redacted = String::new();
    let mut words = output.split_whitespace().peekable();
    while let Some(word) = words.next() {
        if !redacted.is_empty() {
            redacted.push(' ');
        }
        if word == "Bearer" {
            redacted.push_str("Bearer <redacted>");
            let _ = words.next();
        } else if let Some(token) = word.strip_prefix("Bearer ") {
            let _ = token;
            redacted.push_str("Bearer <redacted>");
        } else {
            redacted.push_str(word);
        }
        if redacted.chars().count() >= MAX_CHARS {
            redacted.truncate(MAX_CHARS);
            redacted.push_str("<truncated>");
            break;
        }
    }
    if redacted.is_empty() {
        "<empty>".to_owned()
    } else {
        redacted
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
    if !http_sse.body.contains("\"stream\":true") {
        return Err("HTTP/SSE request did not ask for a streaming response".to_owned());
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouterWebSocketRegistryReport {
    handled_connections: Option<usize>,
    active_sessions: usize,
    high_water_sessions: usize,
    registered_sessions: usize,
    closed_sessions: usize,
    completed_response_sessions: usize,
    forwarded_upstream_messages: usize,
    completed_session_forwarded_upstream_message_counts: Vec<usize>,
    final_session_forwarded_upstream_message_counts: Vec<usize>,
}

impl RouterWebSocketRegistryReport {
    fn from_file(path: &Path) -> Result<Self, String> {
        let contents = fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read router websocket registry report {}: {error}",
                path.display()
            )
        })?;
        let value = serde_json::from_str::<Value>(&contents).map_err(|error| {
            format!(
                "router websocket registry report {} was invalid JSON: {error}",
                path.display()
            )
        })?;
        let registry = value
            .get("websocket_registry")
            .ok_or_else(|| "router websocket registry report was missing registry".to_owned())?;
        let schema_version = required_usize_field(&value, "schema_version")?;
        if schema_version != 1 {
            return Err(format!(
                "router websocket registry report schema_version={schema_version}, expected 1"
            ));
        }
        Ok(Self {
            handled_connections: optional_usize_field(&value, "handled_connections")?,
            active_sessions: required_usize_field(registry, "active_sessions")?,
            high_water_sessions: required_usize_field(registry, "high_water_sessions")?,
            registered_sessions: required_usize_field(registry, "registered_sessions")?,
            closed_sessions: required_usize_field(registry, "closed_sessions")?,
            completed_response_sessions: required_usize_field(
                registry,
                "completed_response_sessions",
            )?,
            forwarded_upstream_messages: required_usize_field(
                registry,
                "forwarded_upstream_messages",
            )?,
            completed_session_forwarded_upstream_message_counts: required_usize_array_field(
                registry,
                "completed_session_forwarded_upstream_message_counts",
            )?,
            final_session_forwarded_upstream_message_counts: required_usize_array_field(
                registry,
                "final_session_forwarded_upstream_message_counts",
            )?,
        })
    }
}

fn required_usize_field(value: &Value, field: &'static str) -> Result<usize, String> {
    let raw = value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("router websocket registry report missing numeric {field}"))?;
    usize::try_from(raw)
        .map_err(|_| format!("router websocket registry report field {field} overflowed usize"))
}

fn optional_usize_field(value: &Value, field: &'static str) -> Result<Option<usize>, String> {
    let Some(raw) = value.get(field) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let raw = raw
        .as_u64()
        .ok_or_else(|| format!("router websocket registry report field {field} was not numeric"))?;
    usize::try_from(raw)
        .map(Some)
        .map_err(|_| format!("router websocket registry report field {field} overflowed usize"))
}

fn required_usize_array_field(value: &Value, field: &'static str) -> Result<Vec<usize>, String> {
    let raw = value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("router websocket registry report missing numeric array {field}"))?;
    raw.iter()
        .map(|item| {
            let raw = item.as_u64().ok_or_else(|| {
                format!("router websocket registry report field {field} contained non-number")
            })?;
            usize::try_from(raw).map_err(|_| {
                format!("router websocket registry report field {field} overflowed usize")
            })
        })
        .collect()
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
    let selected_account_id = selected_account_id_from_status_json(&input.quota_status.json)?;
    let selected_account_tag = safe_account_tag(&selected_account_id);
    let selected_account_label = selected_account_label_from_status_json(&input.quota_status.json)?;
    let redacted = serde_json::json!({
        "mode": input.mode.as_str(),
        "codex_version": input.codex_version,
        "profile_written": input.profile_path.exists(),
        "profile_env_key": null,
        "profile_uses_codex_router_token": false,
        "router_process": {
            "binary_path": input.router_process.binary_path.display().to_string(),
            "pid": input.router_process.pid,
            "argv": input.router_process.argv,
            "listener": input.router_process.listener,
            "readiness_line": input.router_process.readiness_line,
            "cleanup_result": input.router_process.cleanup_result,
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
            "stream_requested": input.upstream.http_sse.as_ref().map(|request| request.body.contains("\"stream\":true")),
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

struct ThreeWebSocketTranscriptInput<'a> {
    mode: &'a str,
    codex_version: &'a str,
    router_process: &'a RouterProcessObservation,
    registry_report: Option<&'a RouterWebSocketRegistryReport>,
    upstream: &'a ConcurrentWebSocketTranscript,
    socket_cleanup: &'a RouterSocketCleanupObservation,
    outputs: &'a [Output],
    seed: &'a SmokeSeed,
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
        .map(|output| {
            serde_json::json!({
                "status": output.status.to_string(),
                "stdout_contains_smoke_text": String::from_utf8_lossy(&output.stdout).contains(SMOKE_EXPECTED_TEXT),
                "stderr_line_count": String::from_utf8_lossy(&output.stderr).lines().count(),
            })
        })
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "git_head": current_git_head()?,
        "mode": input.mode,
        "codex_version": input.codex_version.trim(),
        "router_process": {
            "binary_path": input.router_process.binary_path.display().to_string(),
            "pid": input.router_process.pid,
            "argv": input.router_process.argv,
            "listener": input.router_process.listener,
            "readiness_line": input.router_process.readiness_line,
            "cleanup_result": input.router_process.cleanup_result,
            "spawned_real_serve_child": true,
        },
        "router_websocket_registry": input.registry_report.map(|report| serde_json::json!({
            "handled_connections": report.handled_connections,
            "active_sessions": report.active_sessions,
            "high_water_sessions": report.high_water_sessions,
            "registered_sessions": report.registered_sessions,
            "closed_sessions": report.closed_sessions,
            "completed_response_sessions": report.completed_response_sessions,
            "forwarded_upstream_messages": report.forwarded_upstream_messages,
            "completed_session_forwarded_upstream_message_counts": report.completed_session_forwarded_upstream_message_counts,
            "final_session_forwarded_upstream_message_counts": report.final_session_forwarded_upstream_message_counts,
        })),
        "clients": {
            "count": input.outputs.len(),
            "all_success": input.outputs.iter().all(|output| output.status.success()),
            "statuses": statuses,
        },
        "upstream": {
            "expected_sessions": input.upstream.expected_sessions,
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
        },
        "socket_cleanup": {
            "lsof_exit_status": input.socket_cleanup.lsof_exit_status,
            "tcp_line_count": input.socket_cleanup.tcp_line_count,
            "established_count": input.socket_cleanup.established_count,
            "close_wait_count": input.socket_cleanup.close_wait_count,
            "raw_state_counts": input.socket_cleanup.raw_state_counts,
        },
        "shared_router_pid": input.router_process.pid,
    });
    let rendered = serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("failed to render three-client transcript: {error}"))?;
    assert_redacted_three_websocket_payload(&rendered, input.outputs, input.seed)?;
    fs::write(&transcript_path, rendered)
        .map_err(|error| format!("failed to write three-client transcript: {error}"))?;
    Ok(transcript_path)
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
    outputs: &[Output],
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
    for output in outputs {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
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

fn safe_account_tag(account_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-router:smoke-selected-account:v1:");
    hasher.update(account_id.as_bytes());
    let digest = hasher.finalize();
    let mut output = String::from("acct-");
    for byte in digest.iter().take(6) {
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    output
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'a' + (nibble - 10)),
        _ => unreachable!("nibble is masked to four bits"),
    }
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
    handle: Option<thread::JoinHandle<Result<(), String>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConcurrentWebSocketTranscript {
    expected_sessions: usize,
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
    session_frame_counts: Vec<usize>,
    session_event_counts: Vec<usize>,
    in_overlap_session_event_counts: Vec<usize>,
    non_prewarm_session_count: usize,
    normal_close_sessions: usize,
    abnormal_close_sessions: usize,
    session_close_outcomes: Vec<String>,
    multi_step_interleave_completed: bool,
    multi_step_followup_frame_count: usize,
    multi_step_followup_active_session_count: usize,
    multi_step_followup_unix_ms: Option<u128>,
    multi_step_completed_before_overlap_end: bool,
}

#[derive(Debug)]
struct ConcurrentUpstreamSharedState {
    state: Mutex<ConcurrentUpstreamState>,
    condition: Condvar,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConcurrentUpstreamState {
    expected_sessions: usize,
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
    session_frame_counts: Vec<usize>,
    session_event_counts: Vec<usize>,
    in_overlap_session_event_counts: Vec<usize>,
    non_prewarm_session_count: usize,
    normal_close_sessions: usize,
    abnormal_close_sessions: usize,
    session_close_outcomes: Vec<String>,
    multi_step_interleave_claimed: bool,
    multi_step_interleave_completed: bool,
    multi_step_followup_frame_count: usize,
    multi_step_followup_active_session_count: usize,
    multi_step_followup_unix_ms: Option<u128>,
    multi_step_completed_unix_ms: Option<u128>,
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

impl MockConcurrentWebSocketUpstream {
    fn start(config: ConcurrentUpstreamConfig) -> Result<Self, String> {
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
                session_frame_counts: Vec::new(),
                session_event_counts: Vec::new(),
                in_overlap_session_event_counts: Vec::new(),
                non_prewarm_session_count: 0,
                normal_close_sessions: 0,
                abnormal_close_sessions: 0,
                session_close_outcomes: Vec::new(),
                multi_step_interleave_claimed: false,
                multi_step_interleave_completed: false,
                multi_step_followup_frame_count: 0,
                multi_step_followup_active_session_count: 0,
                multi_step_followup_unix_ms: None,
                multi_step_completed_unix_ms: None,
            }),
            condition: Condvar::new(),
        });
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = thread::Builder::new()
            .name("codex-router-three-client-upstream".to_owned())
            .spawn(move || {
                run_concurrent_mock_upstream(listener, thread_state, thread_shutdown, config)
            })
            .map_err(|error| format!("failed to spawn concurrent mock upstream: {error}"))?;

        Ok(Self {
            address,
            state,
            shutdown,
            handle: Some(handle),
        })
    }

    fn address(&self) -> &str {
        &self.address
    }

    fn join(mut self) -> Result<ConcurrentWebSocketTranscript, String> {
        self.shutdown.store(true, Ordering::SeqCst);
        wake_mock_upstream_accept(&self.address);
        self.state.condition.notify_all();
        let handle = self
            .handle
            .take()
            .ok_or_else(|| "concurrent mock upstream was already joined".to_owned())?;
        join_result(handle, "concurrent mock upstream")?;
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
                "concurrent upstream completed {} sessions, expected {}",
                state.completed_sessions, state.expected_sessions
            ));
        }
        Ok(ConcurrentWebSocketTranscript {
            expected_sessions: state.expected_sessions,
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
            session_frame_counts: state.session_frame_counts,
            session_event_counts: state.session_event_counts,
            in_overlap_session_event_counts: state.in_overlap_session_event_counts,
            non_prewarm_session_count: state.non_prewarm_session_count,
            normal_close_sessions: state.normal_close_sessions,
            abnormal_close_sessions: state.abnormal_close_sessions,
            session_close_outcomes: state.session_close_outcomes,
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

fn run_concurrent_mock_upstream(
    listener: TcpListener,
    state: Arc<ConcurrentUpstreamSharedState>,
    shutdown: Arc<AtomicBool>,
    config: ConcurrentUpstreamConfig,
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
            if state_guard.completed_sessions >= config.expected_sessions {
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
                handles.push(
                    thread::Builder::new()
                        .name("codex-router-three-client-upstream-session".to_owned())
                        .spawn(move || {
                            run_concurrent_mock_websocket_session(
                                stream,
                                session_state,
                                session_shutdown,
                                config,
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
    let mut websocket = accept_hdr(stream, |_request: &Request, response: Response| {
        Ok(response)
    })
    .map_err(|error| format!("concurrent mock upstream websocket handshake failed: {error}"))?;
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
        register_concurrent_non_prewarm_session(&state)?;
        let overlap_started_at = wait_for_concurrent_session_barrier(&state)?;
        let run_multi_step_interleave = claim_multi_step_interleave(&state)?;
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
) -> Result<(), String> {
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "concurrent upstream state mutex poisoned".to_owned())?;
    state.active_non_prewarm_sessions = state.active_non_prewarm_sessions.saturating_add(1);
    state.non_prewarm_session_count = state.non_prewarm_session_count.saturating_add(1);
    state.active_high_water = state
        .active_high_water
        .max(state.active_non_prewarm_sessions);
    if state.active_high_water >= state.expected_sessions && state.overlap_started_at.is_none() {
        state.overlap_started_at = Some(Instant::now());
        state.overlap_started_unix_ms = Some(timestamp_millis());
    }
    shared.condition.notify_all();
    Ok(())
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
    send_concurrent_response_event(websocket, &response_events[0])?;
    event_count = event_count.saturating_add(1);
    in_overlap_event_count =
        in_overlap_event_count.saturating_add(usize::from(is_concurrent_overlap_active(state)?));

    if !config.hold_duration.is_zero() {
        let hold_deadline = overlap_started_at + config.hold_duration + SOAK_PROOF_MARGIN;
        let mut heartbeat_index = 0_usize;
        while Instant::now() < hold_deadline {
            let remaining = hold_deadline.saturating_duration_since(Instant::now());
            thread::sleep(remaining.min(config.heartbeat_interval));
            if Instant::now() >= hold_deadline {
                break;
            }
            let heartbeat = serde_json::json!({
                "type": "response.output_text.delta",
                "delta": "",
                "sequence_number": heartbeat_index,
            })
            .to_string();
            send_concurrent_response_event(websocket, &heartbeat)?;
            heartbeat_index = heartbeat_index.saturating_add(1);
            event_count = event_count.saturating_add(1);
            in_overlap_event_count = in_overlap_event_count
                .saturating_add(usize::from(is_concurrent_overlap_active(state)?));
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
        let mut heartbeat_index = 0_usize;
        while Instant::now() < hold_deadline {
            let remaining = hold_deadline.saturating_duration_since(Instant::now());
            thread::sleep(remaining.min(config.heartbeat_interval));
            if Instant::now() >= hold_deadline {
                break;
            }
            let heartbeat = serde_json::json!({
                "type": "response.output_text.delta",
                "delta": "",
                "sequence_number": heartbeat_index,
            })
            .to_string();
            send_concurrent_response_event(websocket, &heartbeat)?;
            heartbeat_index = heartbeat_index.saturating_add(1);
            event_count = event_count.saturating_add(1);
            in_overlap_event_count = in_overlap_event_count
                .saturating_add(usize::from(is_concurrent_overlap_active(state)?));
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
    use std::borrow::Cow;
    use std::fs;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::process::Output;

    use super::InstalledCodexSmokeMode;
    use super::MockHttpSseTranscript;
    use super::MockWebSocketTranscript;
    use super::RedactedTranscriptInput;
    use super::RouterAuditObservation;
    use super::RouterProcessObservation;
    use super::SMOKE_EXPECTED_TEXT;
    use super::SmokeContractAssertion;
    use super::SmokeQuotaStatus;
    use super::SmokeTempRoot;
    use super::assert_codex_visible_output;
    use super::assert_smoke_contract;
    use super::first_frame_shape_summary;
    use super::run_hostile_no_token_smoke;
    use super::run_installed_codex_http_sse_mock_smoke;
    use super::run_installed_codex_mock_smoke;
    use super::run_installed_codex_three_websocket_mock_e2e;
    use super::run_installed_codex_three_websocket_mock_soak;
    use super::run_installed_codex_websocket_mock_smoke;
    use super::run_with_timeout;
    use super::upstream_account_token;
    use super::write_redacted_transcript;

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
            json: r#"{"preferred_next_account_id":"acct_matches","accounts":[{"account_id":"acct_matches","safe_account_label":"matches"}]}"#.to_owned(),
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
