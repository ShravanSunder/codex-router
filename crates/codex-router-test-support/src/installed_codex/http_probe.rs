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
