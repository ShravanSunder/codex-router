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
    seed_quota_reconnect_router_state(
        &runtime_roots.state_path,
        &runtime_roots.secret_root,
        QUOTA_RECONNECT_PRIMARY_FOR_INITIAL_ADMISSION,
    )?;
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
