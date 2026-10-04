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
