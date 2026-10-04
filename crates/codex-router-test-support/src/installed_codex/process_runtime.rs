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
