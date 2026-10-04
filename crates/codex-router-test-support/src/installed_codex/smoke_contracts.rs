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
