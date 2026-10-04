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
