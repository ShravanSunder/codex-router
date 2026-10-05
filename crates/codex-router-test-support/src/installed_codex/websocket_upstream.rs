use super::*;

pub(super) fn run_mock_upstream(
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

pub(super) fn run_quota_reconnect_mock_upstream(
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
pub(super) fn run_quota_reconnect_mock_websocket_session(
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

pub(super) fn start_quota_reconnect_sqlite_pressure(
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

pub(super) fn start_s8_overlap_quota_sqlite_pressure(
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

pub(super) fn quota_reconnect_completion_sent(
    state: &Arc<Mutex<QuotaReconnectUpstreamState>>,
) -> Result<bool, String> {
    state
        .lock()
        .map(|state| state.completion_sent)
        .map_err(|_| "quota reconnect upstream state mutex poisoned".to_owned())
}
