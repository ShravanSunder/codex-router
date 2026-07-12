use super::*;

const TEST_STREAM_MAX_RETRIES: u64 = 100;
const TEST_ACCOUNT_TOKENS: [&str; 3] = [
    "retry-integration-account-a",
    "retry-integration-account-b",
    "retry-integration-account-c",
];

#[derive(Clone, Copy, Debug)]
enum RetryScenario {
    ThreeAccountShortQuota,
    WeeklyTerminal,
    CapacityThenSuccess(&'static str),
    CapacityLimit,
}

#[derive(Clone, Debug, Default)]
struct RetryUpstreamState {
    handshakes: usize,
    non_prewarm_requests: usize,
    completion_sent: bool,
    observed_tokens: Vec<String>,
    observed_thread_ids: Vec<String>,
    terminal_original_sent: bool,
}

struct RetryUpstream {
    address: String,
    state: Arc<Mutex<RetryUpstreamState>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<Result<(), String>>>,
}

pub fn run_three_account_short_quota_reconnect() -> Result<(), String> {
    run_retry_scenario(RetryScenario::ThreeAccountShortQuota)
}

pub fn run_all_weekly_exhausted_terminal() -> Result<(), String> {
    run_retry_scenario(RetryScenario::WeeklyTerminal)
}

pub fn run_model_capacity_reconnect() -> Result<(), String> {
    for code in ["server_is_overloaded", "slow_down"] {
        run_retry_scenario(RetryScenario::CapacityThenSuccess(code))?;
    }
    Ok(())
}

pub fn run_capacity_retry_limit_terminal() -> Result<(), String> {
    run_retry_scenario(RetryScenario::CapacityLimit)
}

fn run_retry_scenario(scenario: RetryScenario) -> Result<(), String> {
    let smoke_root = SmokeTempRoot::new("installed-codex-retry-integration")?;
    let codex_home = smoke_root.path().join("codex-home");
    let workdir = smoke_root.path().join("workdir");
    let process_home = smoke_root.path().join("home");
    let xdg_config_home = smoke_root.path().join("xdg-config");
    let xdg_state_home = smoke_root.path().join("xdg-state");
    let xdg_cache_home = smoke_root.path().join("xdg-cache");
    let router_root = smoke_root.path().join("router");
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    for path in [
        &codex_home,
        &workdir,
        &process_home,
        &xdg_config_home,
        &xdg_state_home,
        &xdg_cache_home,
        &router_root,
    ] {
        fs::create_dir_all(path)
            .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    }

    let test_now_unix_seconds = timestamp_seconds();
    seed_retry_accounts(&state_path, &secret_root, scenario, test_now_unix_seconds)?;
    let upstream = RetryUpstream::start(scenario)?;
    let router_port = reserve_loopback_port()?;
    let profile_path = CodexRouterProfileWriter::new(&codex_home)
        .write(&CodexRouterProfile::new(router_port), true)
        .map_err(|error| format!("failed to write retry integration profile: {error}"))?;
    append_stream_retry_override(&profile_path)?;
    let audit_path = router_root.join("retry-integration-audit.jsonl");
    let registry_report_path = router_root.join("retry-integration-registry.json");
    let router = start_router_process_with_options(RouterProcessStartOptions {
        now_unix_seconds: None,
        router_port,
        state_path,
        secret_root,
        local_token: None,
        upstream_base_url: format!("http://{}/v1", upstream.address()),
        audit_path,
        max_connections: 256,
        websocket_registry_report_file: Some(registry_report_path),
    })?;
    let last_message_path = smoke_root.path().join("last-message.txt");
    let output_result = run_codex_exec_with_timeout(
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
        Duration::from_secs(75),
    );
    let router_result = router.stop("retry integration router");
    let state = upstream.join()?;
    let output = output_result.map_err(|error| {
        format!("{error}; retry_upstream_state={state:?}; router_cleanup={router_result:?}")
    })?;
    let _router_observation = router_result?;
    assert_retry_scenario(scenario, &output, &last_message_path, &state)
}

fn append_stream_retry_override(profile_path: &Path) -> Result<(), String> {
    let profile = fs::read_to_string(profile_path)
        .map_err(|error| format!("failed to read {}: {error}", profile_path.display()))?;
    let anchor = "supports_websockets = true";
    if !profile.contains(anchor) {
        return Err("generated profile omitted WebSocket provider anchor".to_owned());
    }
    let profile = profile.replacen(
        anchor,
        &format!("{anchor}\nstream_max_retries = {TEST_STREAM_MAX_RETRIES}"),
        1,
    );
    fs::write(profile_path, profile)
        .map_err(|error| format!("failed to write stream retry override: {error}"))
}

fn seed_retry_accounts(
    state_path: &Path,
    secret_root: &Path,
    scenario: RetryScenario,
    now_unix_seconds: u64,
) -> Result<(), String> {
    let state = SqliteStateStore::open(state_path)
        .map_err(|error| format!("failed to open retry integration state: {error}"))?;
    let secrets = FileSecretStore::open(secret_root)
        .map_err(|error| format!("failed to open retry integration secrets: {error}"))?;
    let weekly_exhausted = matches!(scenario, RetryScenario::WeeklyTerminal);
    let account_count = if matches!(
        scenario,
        RetryScenario::ThreeAccountShortQuota | RetryScenario::WeeklyTerminal
    ) {
        3
    } else {
        1
    };
    for (index, token) in TEST_ACCOUNT_TOKENS.iter().take(account_count).enumerate() {
        let account_id = account_id(&format!("acct_retry_integration_{index}"))?;
        let account = AccountRecord::new(
            account_id.clone(),
            format!("retry-integration-{index}"),
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        AccountStateRepository::upsert_account(&state, &account)
            .map_err(|error| format!("failed to seed retry account: {error}"))?;
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(100)
        .with_reset_unix_seconds(now_unix_seconds.saturating_add(5))
        .with_effective(true)
        .with_observed_unix_seconds(now_unix_seconds);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            if weekly_exhausted {
                SelectorQuotaWindowStatus::Ineligible
            } else {
                SelectorQuotaWindowStatus::Eligible
            },
        )
        .with_remaining_headroom(if weekly_exhausted { 0 } else { 100 })
        .with_reset_unix_seconds(now_unix_seconds.saturating_add(604_800))
        .with_observed_unix_seconds(now_unix_seconds);
        SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
            &state,
            &account_id,
            "responses",
            &[short_window, weekly_window],
            now_unix_seconds,
            now_unix_seconds.saturating_add(300),
        )
        .map_err(|error| format!("failed to seed retry quota windows: {error}"))?;
        let credential_key = account_credential_bundle_key(&account_id, 1)
            .map_err(|error| format!("failed to build retry credential key: {error}"))?;
        let credential = AccountCredentialBundle::imported_codex_auth(*token, None)
            .to_secret_string()
            .map_err(|error| format!("failed to serialize retry credential: {error}"))?;
        secrets
            .write_secret(&credential_key, &credential)
            .map_err(|error| format!("failed to write retry credential: {error}"))?;
    }
    Ok(())
}

impl RetryUpstream {
    fn start(scenario: RetryScenario) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("failed to bind retry upstream: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("failed to set retry listener nonblocking: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("failed to read retry upstream address: {error}"))?
            .to_string();
        let state = Arc::new(Mutex::new(RetryUpstreamState::default()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = thread::Builder::new()
            .name("codex-router-retry-integration-upstream".to_owned())
            .spawn(move || run_retry_upstream(listener, scenario, thread_state, thread_shutdown))
            .map_err(|error| format!("failed to spawn retry upstream: {error}"))?;
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

    fn join(mut self) -> Result<RetryUpstreamState, String> {
        self.shutdown.store(true, Ordering::SeqCst);
        wake_mock_upstream_accept(&self.address);
        let handle = self
            .handle
            .take()
            .ok_or_else(|| "retry upstream already joined".to_owned())?;
        join_result(handle, "retry integration upstream")?;
        self.state
            .lock()
            .map_err(|_| "retry upstream state mutex poisoned".to_owned())
            .map(|state| state.clone())
    }
}

impl Drop for RetryUpstream {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.shutdown.store(true, Ordering::SeqCst);
            wake_mock_upstream_accept(&self.address);
            let _ = join_result(handle, "retry upstream cleanup");
        }
    }
}

fn run_retry_upstream(
    listener: TcpListener,
    scenario: RetryScenario,
    state: Arc<Mutex<RetryUpstreamState>>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(70);
    while Instant::now() < deadline && !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).map_err(|error| {
                    format!("failed to restore retry stream blocking mode: {error}")
                })?;
                if !looks_like_websocket_upgrade(&stream)? {
                    respond_to_http_request(stream)?;
                    continue;
                }
                run_retry_websocket(stream, scenario, &state)?;
                if state
                    .lock()
                    .map_err(|_| "retry state poisoned".to_owned())?
                    .completion_sent
                    || matches!(scenario, RetryScenario::CapacityLimit)
                        && state
                            .lock()
                            .map_err(|_| "retry state poisoned".to_owned())?
                            .terminal_original_sent
                {
                    return Ok(());
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(format!("retry upstream accept failed: {error}")),
        }
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn run_retry_websocket(
    stream: std::net::TcpStream,
    scenario: RetryScenario,
    state: &Arc<Mutex<RetryUpstreamState>>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|error| format!("failed to set retry websocket timeout: {error}"))?;
    let headers = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let callback_headers = Arc::clone(&headers);
    let mut websocket = accept_hdr(stream, move |request: &Request, response: Response| {
        if let Ok(mut target) = callback_headers.lock() {
            *target = request
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.as_str().to_owned(), value.to_owned()))
                })
                .collect();
        }
        Ok(response)
    })
    .map_err(|error| format!("retry websocket handshake failed: {error}"))?;
    let captured = headers
        .lock()
        .map_err(|_| "retry header mutex poisoned".to_owned())?;
    let token = bearer_token_from_headers(&captured)
        .unwrap_or_default()
        .to_owned();
    let thread_id = captured
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("thread-id"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    drop(captured);
    state
        .lock()
        .map_err(|_| "retry state poisoned".to_owned())?
        .handshakes += 1;
    loop {
        let frame = match websocket.read() {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Binary(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => continue,
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
            {
                return Ok(());
            }
            Err(error) => return Err(format!("retry websocket read failed: {error}")),
        };
        if is_prewarm_request_frame(&frame) {
            for event in smoke_prewarm_events(0) {
                websocket
                    .send(Message::Text(event.into()))
                    .map_err(|error| error.to_string())?;
            }
            continue;
        }
        let request_index = {
            let mut locked = state
                .lock()
                .map_err(|_| "retry state poisoned".to_owned())?;
            locked.non_prewarm_requests += 1;
            locked.observed_tokens.push(token);
            locked.observed_thread_ids.push(thread_id);
            locked.non_prewarm_requests
        };
        let error_frame = match scenario {
            RetryScenario::ThreeAccountShortQuota if request_index <= 3 => {
                Some(quota_reconnect_usage_limit_frame().to_owned())
            }
            RetryScenario::CapacityThenSuccess(code) if request_index == 1 => {
                Some(capacity_error_frame(code))
            }
            RetryScenario::CapacityLimit if request_index <= 11 => {
                Some(capacity_error_frame("server_is_overloaded"))
            }
            _ => None,
        };
        if let Some(error_frame) = error_frame {
            websocket
                .send(Message::Text(error_frame.into()))
                .map_err(|error| error.to_string())?;
            if matches!(scenario, RetryScenario::CapacityLimit) && request_index == 11 {
                state
                    .lock()
                    .map_err(|_| "retry state poisoned".to_owned())?
                    .terminal_original_sent = true;
            }
        } else {
            for event in smoke_response_events(request_index) {
                websocket
                    .send(Message::Text(event.into()))
                    .map_err(|error| error.to_string())?;
            }
            state
                .lock()
                .map_err(|_| "retry state poisoned".to_owned())?
                .completion_sent = true;
        }
        let _ = websocket.close(None);
        return Ok(());
    }
}

fn capacity_error_frame(code: &str) -> String {
    format!(
        r#"{{"type":"response.failed","response":{{"error":{{"code":"{code}","message":"Selected model is at capacity. Please try a different model"}}}}}}"#
    )
}

fn assert_retry_scenario(
    scenario: RetryScenario,
    output: &Output,
    last_message_path: &Path,
    state: &RetryUpstreamState,
) -> Result<(), String> {
    match scenario {
        RetryScenario::ThreeAccountShortQuota => {
            if !output.status.success() || !state.completion_sent || state.non_prewarm_requests != 4
            {
                return Err(format!(
                    "3-account 5h retry contract failed: status={} state={state:?} saw_retry_after={} saw_usage_limit={} stderr_preview={} markers={}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).contains("Try again in"),
                    String::from_utf8_lossy(&output.stderr).contains("usage limit"),
                    redacted_process_output_preview(&String::from_utf8_lossy(&output.stderr)),
                    process_output_markers(&String::from_utf8_lossy(&output.stderr)),
                ));
            }
            let unique_tokens = state
                .observed_tokens
                .iter()
                .take(3)
                .collect::<BTreeSet<_>>();
            if unique_tokens.len() != 3 {
                return Err(format!("expected three rotated accounts: {state:?}"));
            }
        }
        RetryScenario::WeeklyTerminal => {
            if output.status.success() || state.handshakes != 0 {
                return Err(format!(
                    "weekly exhaustion must stop without upstream retry: status={} state={state:?}",
                    output.status
                ));
            }
        }
        RetryScenario::CapacityThenSuccess(_) => {
            if !output.status.success() || !state.completion_sent || state.non_prewarm_requests != 2
            {
                return Err(format!(
                    "capacity reconnect contract failed: status={} state={state:?}",
                    output.status
                ));
            }
            let stable_thread_id = state
                .observed_thread_ids
                .first()
                .zip(state.observed_thread_ids.get(1))
                .is_some_and(|(first, second)| !first.is_empty() && first == second);
            if state.observed_thread_ids.len() != 2 || !stable_thread_id {
                return Err(format!(
                    "capacity reconnect did not preserve thread-id: {state:?}"
                ));
            }
        }
        RetryScenario::CapacityLimit => {
            if output.status.success()
                || state.non_prewarm_requests != 11
                || !state.terminal_original_sent
            {
                return Err(format!(
                    "capacity retry limit contract failed: status={} state={state:?}",
                    output.status
                ));
            }
        }
    }
    let combined = format!(
        "{}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(last_message_path).unwrap_or_default()
    );
    for token in TEST_ACCOUNT_TOKENS {
        if combined.contains(token) {
            return Err("installed Codex output leaked upstream credential".to_owned());
        }
    }
    Ok(())
}
