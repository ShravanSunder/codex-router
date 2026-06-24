//! Installed Codex smoke harness.

use std::borrow::Cow;
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
use codex_router_proxy::server::LoopbackBindAddress;
use codex_router_proxy::server::LoopbackRouterRuntime;
use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
use codex_router_proxy::upstream::UpstreamEndpoint;
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
use tungstenite::accept_hdr;
use tungstenite::client::IntoClientRequest;
use tungstenite::connect;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;

const SMOKE_EXPECTED_TEXT: &str = "codex-router smoke ok";
const CODEX_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_ACCEPT_TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstalledCodexSmokeMode {
    HttpSse,
    WebSocket,
    Combined,
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
    let router_thread = RouterThreadGuard::new(
        router_port,
        start_router_once(
            router_port,
            state_path,
            secret_root,
            None,
            format!("http://{}/v1", upstream.address()),
            audit_path.clone(),
        )?,
    );

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
    router_thread.join("router runtime")?;
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
        router_audit: &router_audit,
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
    let seed = seed_router_state(&state_path, &secret_root)?;
    let router_port = reserve_loopback_port()?;
    let audit_path = router_root.join("audit").join("events.jsonl");
    let router_thread = RouterThreadGuard::new(
        router_port,
        start_router_once(
            router_port,
            state_path,
            secret_root,
            Some(seed.local_token),
            format!("http://{}/v1", upstream.address()),
            audit_path,
        )?,
    );

    send_hostile_no_token_websocket(router_port)?;
    router_thread.join("hostile no-token router runtime")?;
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

fn start_router_once(
    router_port: u16,
    state_path: PathBuf,
    secret_root: PathBuf,
    local_token: Option<String>,
    upstream_base_url: String,
    audit_path: PathBuf,
) -> Result<thread::JoinHandle<Result<(), String>>, String> {
    let (ready_sender, ready_receiver) = mpsc::channel();
    let handle = thread::Builder::new()
        .name("codex-router-installed-smoke-router".to_owned())
        .spawn(move || {
            let bind_address = LoopbackBindAddress::new("127.0.0.1", router_port)
                .map_err(|error| format!("failed to build router bind address: {error}"))?;
            let upstream_endpoint = UpstreamEndpoint::new(upstream_base_url)
                .map_err(|error| format!("failed to build upstream endpoint: {error}"))?;
            let mut runtime_config = LoopbackRouterRuntimeConfig::new_tokenless(
                bind_address,
                upstream_endpoint,
                state_path,
                secret_root,
            )
            .with_quota_clock(1_030, 60)
            .with_max_websocket_upstream_messages(4)
            .with_audit_file(audit_path);
            if let Some(local_token) = local_token {
                let local_token = codex_router_core::local_auth::LocalRouterTokenRecord::new(
                    SecretString::new(local_token),
                    codex_router_core::ids::TokenGeneration::new(1),
                );
                runtime_config = runtime_config.with_required_local_token(local_token);
            }
            let runtime = LoopbackRouterRuntime::start(runtime_config)
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
    _local_token_assignment: &str,
    child_environment: CodexChildEnvironment,
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

struct MockNoConnectionUpstream {
    address: String,
    handle: Option<thread::JoinHandle<Result<usize, String>>>,
}

struct RouterThreadGuard {
    router_port: u16,
    handle: Option<thread::JoinHandle<Result<(), String>>>,
}

impl RouterThreadGuard {
    fn new(router_port: u16, handle: thread::JoinHandle<Result<(), String>>) -> Self {
        Self {
            router_port,
            handle: Some(handle),
        }
    }

    fn join(mut self, label: &str) -> Result<(), String> {
        drain_optional_router_connections(self.router_port, 16);
        let handle = self
            .handle
            .take()
            .ok_or_else(|| format!("{label} was already joined"))?;
        join_result(handle, label)
    }
}

impl Drop for RouterThreadGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            drain_optional_router_connections(self.router_port, 16);
            let _ = join_result(handle, "router runtime cleanup");
        }
    }
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

fn run_mock_upstream(
    listener: TcpListener,
    transcript: Arc<Mutex<Option<MockWebSocketTranscript>>>,
    shutdown: Arc<AtomicBool>,
    mode: InstalledCodexSmokeMode,
) -> Result<(), String> {
    let mut http_probe_count = 0_usize;
    let mut http_sse_count = 0_usize;
    let mut http_sse = None;
    loop {
        let deadline = Instant::now() + UPSTREAM_ACCEPT_TIMEOUT;
        let stream = accept_with_deadline(
            &listener,
            &shutdown,
            deadline,
            http_probe_count,
            http_sse_count,
        )?;
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
        run_mock_websocket(stream, transcript, http_probe_count, http_sse.take())?;
        return Ok(());
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
    let mut transcript = transcript
        .lock()
        .map_err(|_| "mock upstream transcript mutex poisoned".to_owned())?;
    *transcript = Some(MockWebSocketTranscript {
        headers,
        first_frame,
        request_frames,
        websocket_request_frame_count,
        http_probe_count,
        http_sse,
    });

    Ok(())
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
