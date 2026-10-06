use super::*;

pub(super) fn s8_smoke_provenance(scenario: &str) -> serde_json::Value {
    serde_json::json!({
        "run_id": s8_smoke_run_id(),
        "scenario": scenario,
    })
}

pub(super) fn s8_smoke_run_id() -> Option<String> {
    std::env::var(S8_RUN_ID_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(super) fn runtime_correlations_for_three_websocket(
    input: &ThreeWebSocketTranscriptInput<'_>,
) -> Vec<Value> {
    let upstream_session_by_client = upstream_session_by_client_index(input.upstream);
    input
        .outputs
        .iter()
        .enumerate()
        .map(|(index, run)| {
            let stderr = String::from_utf8_lossy(&run.output.stderr);
            let markers = stderr_transport_error_markers(&stderr);
            serde_json::json!({
                "client_index": index,
                "client_pid": run.pid,
                "router_pid": input.router_process.pid,
                "router_session_observed": input.registry_report.is_some_and(|report| {
                    report.high_water_sessions >= input.outputs.len()
                        && report.registered_sessions >= input.outputs.len()
                }),
                "upstream_session_observed": upstream_session_by_client.contains_key(&index),
                "transport": if markers.is_empty() { "websocket" } else { "websocket_error" },
                "stderr_transport_error_markers": markers,
                "stdout_contains_smoke_text": String::from_utf8_lossy(&run.output.stdout).contains(SMOKE_EXPECTED_TEXT),
            })
        })
        .collect()
}

pub(super) fn session_continuity_for_three_websocket(
    input: &ThreeWebSocketTranscriptInput<'_>,
) -> Value {
    let upstream_session_by_client = upstream_session_by_client_index(input.upstream);
    let router_registry_observed = input.registry_report.is_some_and(|report| {
        report.high_water_sessions >= input.outputs.len()
            && report.registered_sessions >= input.outputs.len()
            && report.closed_sessions >= input.outputs.len()
    });
    let per_client_join_observations = input
        .outputs
        .iter()
        .enumerate()
        .map(|(client_index, run)| {
            serde_json::json!({
                "client_index": client_index,
                "client_pid": run.pid,
                "router_session_observed": router_registry_observed,
                "upstream_session_observed": upstream_session_by_client.contains_key(&client_index),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "per_client_session_join_key_observed": false,
        "correlation_level": if router_registry_observed {
            "router_registry_counts_and_upstream_marker_join"
        } else {
            "upstream_marker_join"
        },
        "per_client_join_observations": per_client_join_observations,
        "router_registered_unique_session_count": input
            .registry_report
            .map_or(0, |report| report.registered_session_id_count),
        "router_closed_unique_session_count": input
            .registry_report
            .map_or(0, |report| report.closed_session_id_count),
        "upstream_unique_session_count": unique_u64_count(&input.upstream.upstream_session_ids),
        "router_pid": input.router_process.pid,
        "shared_router_pid": input.router_process.pid,
    })
}

pub(super) fn upstream_session_by_client_index(
    upstream: &ConcurrentWebSocketTranscript,
) -> BTreeMap<usize, u64> {
    upstream
        .upstream_client_sessions
        .iter()
        .map(|session| (session.client_index, session.upstream_session_id))
        .collect()
}

pub(super) fn unique_u64_count(values: &[u64]) -> usize {
    values.iter().copied().collect::<BTreeSet<_>>().len()
}

pub(super) fn stderr_transport_error_markers(stderr: &str) -> Vec<&'static str> {
    let stderr = stderr.to_ascii_lowercase();
    [
        ("fallback", "fallback"),
        ("reconnect", "reconnect"),
        ("websocket protocol error", "websocket_protocol_error"),
        ("handshake not finished", "handshake_not_finished"),
        (
            "stream disconnected before completion",
            "stream_disconnected_before_completion",
        ),
        ("closed connection", "closed_connection"),
        ("connection closed", "closed_connection"),
        ("waiting on loopback", "waiting_on_loopback"),
        ("function_call_output", "function_call_output"),
        ("shell_command", "shell_command"),
        ("tool-call", "tool_call"),
        ("request timed out", "request_timed_out"),
    ]
    .into_iter()
    .filter_map(|(needle, marker)| stderr.contains(needle).then_some(marker))
    .collect()
}

pub(super) fn sanitized_artifact_path(path: &Path) -> Result<String, String> {
    let workspace = workspace_root()?;
    if let Ok(relative) = path.strip_prefix(&workspace) {
        return Ok(format!("<repo>/{}", relative.display()));
    }
    Ok(path.file_name().and_then(|name| name.to_str()).map_or_else(
        || "<external-path>".to_owned(),
        |name| format!("<external>/{name}"),
    ))
}

pub(super) fn sanitized_router_argv(argv: &[String]) -> Vec<String> {
    let path_value_flags = [
        "--port",
        "--state-db",
        "--secret-root",
        "--upstream-base-url",
        "--audit-file",
        "--websocket-registry-report-file",
    ];
    let mut sanitized = Vec::with_capacity(argv.len());
    let mut redact_next_value: Option<&str> = None;
    for value in argv {
        if let Some(flag) = redact_next_value.take() {
            sanitized.push(format!("<{flag}-path>"));
            continue;
        }
        sanitized.push(value.clone());
        if path_value_flags.contains(&value.as_str()) {
            redact_next_value = Some(value.trim_start_matches("--"));
        }
    }
    sanitized
}

pub(super) fn sanitized_loopback_endpoint_text(value: &str) -> String {
    let mut sanitized = value.to_owned();
    for prefix in ["127.0.0.1:", "localhost:"] {
        sanitized = replace_port_after_prefix(&sanitized, prefix);
    }
    sanitized
}

pub(super) fn replace_port_after_prefix(value: &str, prefix: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(index) = remaining.find(prefix) {
        let (before, after_before) = remaining.split_at(index);
        output.push_str(before);
        output.push_str(prefix);
        output.push_str("<port>");
        let Some(after_prefix) = after_before.strip_prefix(prefix) else {
            output.push_str(after_before);
            remaining = "";
            break;
        };
        let first_non_digit = after_prefix
            .char_indices()
            .find_map(|(offset, character)| (!character.is_ascii_digit()).then_some(offset))
            .unwrap_or(after_prefix.len());
        remaining = after_prefix.get(first_non_digit..).unwrap_or_default();
    }
    output.push_str(remaining);
    output
}

pub(super) fn current_git_head() -> Result<String, String> {
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

pub(super) fn observe_router_socket_cleanup(
    pid: u32,
) -> Result<RouterSocketCleanupObservation, String> {
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
    pub(super) fn assert_no_leaked_sessions(&self) -> Result<(), String> {
        if self.established_count != 0 || self.close_wait_count != 0 {
            return Err(format!(
                "router socket cleanup found established_count={} close_wait_count={} state_counts={:?}",
                self.established_count, self.close_wait_count, self.raw_state_counts
            ));
        }
        Ok(())
    }
}

pub(super) fn assert_redacted_three_websocket_payload(
    payload: &str,
    outputs: &[CodexChildRun],
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
        Some("/Users/"),
        Some("/var/folders/"),
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
    if contains_loopback_endpoint_with_numeric_port(payload) {
        return Err(
            "three-client transcript leaked loopback endpoint with numeric port".to_owned(),
        );
    }
    let forbidden_structural_keys = [
        "registered_session_ids",
        "completed_session_ids",
        "closed_session_ids",
        "session_peer_addrs",
        "session_id",
        "local_port",
        "router_session_id",
        "upstream_session_id",
        "per_client_join_keys",
        "router_registered_session_ids",
        "router_closed_session_ids",
        "upstream_session_ids",
        "upstream_client_sessions",
    ];
    for forbidden_key in forbidden_structural_keys {
        let quoted_key = format!("\"{forbidden_key}\"");
        if payload.contains(&quoted_key) {
            return Err(format!(
                "three-client transcript leaked forbidden structural key: {forbidden_key}"
            ));
        }
    }
    for run in outputs {
        let stdout = String::from_utf8_lossy(&run.output.stdout);
        let stderr = String::from_utf8_lossy(&run.output.stderr);
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

pub(super) fn contains_loopback_endpoint_with_numeric_port(payload: &str) -> bool {
    ["127.0.0.1:", "localhost:"]
        .into_iter()
        .any(|prefix| contains_prefix_followed_by_digit(payload, prefix))
}

pub(super) fn contains_prefix_followed_by_digit(payload: &str, prefix: &str) -> bool {
    let mut remaining = payload;
    while let Some(index) = remaining.find(prefix) {
        let Some(after_prefix) = remaining.get(index + prefix.len()..) else {
            return false;
        };
        if after_prefix
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
        {
            return true;
        }
        remaining = after_prefix;
    }
    false
}

pub(super) fn assert_redacted_quota_reconnect_payload(
    payload: &str,
    input: &QuotaReconnectTranscriptInput<'_>,
) -> Result<(), String> {
    let forbidden_fragments = [
        QUOTA_RECONNECT_PRIMARY.upstream_token,
        QUOTA_RECONNECT_FALLBACK.upstream_token,
        "installed-quota-primary-token-refresh",
        "installed-quota-fallback-token-refresh",
        QUOTA_RECONNECT_PRIMARY.label,
        QUOTA_RECONNECT_FALLBACK.label,
        quota_reconnect_usage_limit_frame(),
        input.codex_stdout,
        input.codex_stderr,
        "/Users/",
        "/var/folders/",
    ];
    for forbidden in forbidden_fragments
        .into_iter()
        .filter(|fragment| !fragment.is_empty())
    {
        if payload.contains(forbidden) {
            return Err(format!(
                "quota reconnect transcript leaked forbidden fragment: {forbidden}"
            ));
        }
    }
    Ok(())
}

pub(super) fn first_frame_shape_summary(first_frame: &Value) -> Value {
    serde_json::json!({
        "json_object": first_frame.is_object(),
        "non_prewarm_response_create": is_non_prewarm_response_create_frame(first_frame),
    })
}

pub(super) fn response_create_frame_shape_summary(upstream: &MockWebSocketTranscript) -> Value {
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

pub(super) fn assert_redacted_transcript_payload(
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
        Some("/Users/"),
        Some("/var/folders/"),
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
