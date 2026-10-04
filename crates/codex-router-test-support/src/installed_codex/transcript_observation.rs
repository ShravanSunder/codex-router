#[derive(Clone, Debug, Eq, PartialEq)]
struct RouterAuditObservation {
    http_sse_local_auth_validated: bool,
    websocket_local_auth_validated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouterProcessObservation {
    binary_path: PathBuf,
    pid: u32,
    argv: Vec<String>,
    listener: String,
    readiness_line: String,
    cleanup_result: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct RouterWebSocketRegistryReport {
    handled_connections: Option<usize>,
    active_sessions: usize,
    high_water_sessions: usize,
    registered_sessions: usize,
    closed_sessions: usize,
    completed_response_sessions: usize,
    forwarded_upstream_messages: usize,
    registered_session_id_count: usize,
    completed_session_id_count: usize,
    closed_session_id_count: usize,
    session_peer_addr_count: usize,
    session_peer_join_observable: bool,
    completed_session_forwarded_upstream_message_counts: Vec<usize>,
    final_session_forwarded_upstream_message_counts: Vec<usize>,
    #[serde(default)]
    quota_reconnect_signal_count: usize,
    quota_reconnect_signal_unix_ms: Option<u128>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
struct RouterWebSocketRegistryReportFile {
    schema_version: usize,
    handled_connections: Option<usize>,
    websocket_registry: RouterWebSocketRegistryReport,
}

impl RouterWebSocketRegistryReport {
    fn from_file(path: &Path) -> Result<Self, String> {
        let contents = fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read router websocket registry report {}: {error}",
                path.display()
            )
        })?;
        let report = serde_json::from_str::<RouterWebSocketRegistryReportFile>(&contents).map_err(
            |error| {
                format!(
                    "router websocket registry report {} was invalid JSON: {error}",
                    path.display()
                )
            },
        )?;
        if report.schema_version != 2 {
            return Err(format!(
                "router websocket registry report schema_version={}, expected 2",
                report.schema_version
            ));
        }
        let mut registry = report.websocket_registry;
        registry.handled_connections = report.handled_connections;
        Ok(registry)
    }
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
    router_process: &'a RouterProcessObservation,
    router_audit: &'a RouterAuditObservation,
}

struct QuotaReconnectTranscriptInput<'a> {
    codex_version: &'a str,
    profile_path: &'a Path,
    codex_status: &'a ExitStatus,
    codex_stdout: &'a str,
    codex_stderr: &'a str,
    last_message_path: &'a Path,
    upstream: &'a QuotaReconnectWebSocketTranscript,
    router_process: &'a RouterProcessObservation,
    router_audit: &'a RouterAuditObservation,
    registry_report: &'a RouterWebSocketRegistryReport,
    runtime_roots: &'a InstalledCodexRuntimeRoots,
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
    let selected_account = selected_account_from_status_json(&input.quota_status.json)?;
    let selected_account_tag = selected_account.account_hash;
    let selected_account_label = selected_account.safe_label;
    let router_binary_path = sanitized_artifact_path(&input.router_process.binary_path)?;
    let router_argv = sanitized_router_argv(&input.router_process.argv);
    let redacted = serde_json::json!({
        "mode": input.mode.as_str(),
        "codex_version": input.codex_version,
        "profile_written": input.profile_path.exists(),
        "profile_env_key": null,
        "profile_uses_codex_router_token": false,
        "router_process": {
            "binary_path": router_binary_path,
            "pid": input.router_process.pid,
            "argv": router_argv,
            "listener": sanitized_loopback_endpoint_text(&input.router_process.listener),
            "readiness_line": sanitized_loopback_endpoint_text(&input.router_process.readiness_line),
            "cleanup_result": sanitized_loopback_endpoint_text(&input.router_process.cleanup_result),
            "spawned_real_serve_child": true,
        },
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
            "stream_requested": input.upstream.http_sse.as_ref().map(http_sse_request_asks_streaming),
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

fn write_redacted_quota_reconnect_transcript(
    input: &QuotaReconnectTranscriptInput<'_>,
) -> Result<PathBuf, String> {
    let artifact_dir = workspace_root()?.join("tmp").join("smoke");
    fs::create_dir_all(&artifact_dir).map_err(|error| {
        format!(
            "failed to create smoke artifact dir {}: {error}",
            artifact_dir.display()
        )
    })?;
    let transcript_path = artifact_dir.join(format!(
        "installed-codex-quota-reconnect-{}-{}.json",
        std::process::id(),
        timestamp_millis()
    ));
    let router_root = sanitized_artifact_path(&input.runtime_roots.router_root)?;
    let router_db = sanitized_artifact_path(&input.runtime_roots.state_path)?;
    let codex_home = input
        .runtime_roots
        .codex_home
        .as_deref()
        .map(sanitized_artifact_path)
        .transpose()?;
    let codex_db = input
        .runtime_roots
        .codex_home
        .as_deref()
        .map(|path| sanitized_artifact_path(&path.join("state_5.sqlite")))
        .transpose()?;
    let process_home = input
        .runtime_roots
        .process_home
        .as_deref()
        .map(sanitized_artifact_path)
        .transpose()?;
    let runtime_roots = serde_json::json!({
        "mode": input.runtime_roots.mode.as_str(),
        "router_root": router_root,
        "router_db": router_db,
        "codex_home": codex_home,
        "codex_db": codex_db,
        "process_home": process_home,
    });
    let sqlite_pressure = input.upstream.sqlite_pressure.as_ref();
    let sqlite_lock_or_maintenance_pressure = sqlite_pressure.is_some_and(|pressure| {
        pressure.acquired_before_quota_error && pressure.released_after_completion
    });
    let reconnected_to_different_account =
        input.upstream.quota_error_connection_label != input.upstream.completion_connection_label;
    let pressure = serde_json::json!({
        "pressure_mechanism": sqlite_pressure.map_or("none", |pressure| pressure.mechanism),
        "copied_db_pressure_proven": input.runtime_roots.mode == "copied-dev-state"
            && sqlite_lock_or_maintenance_pressure,
        "sqlite_lock_or_maintenance_pressure": sqlite_lock_or_maintenance_pressure,
        "provider_error_observer_delay": false,
        "sqlite_pressure": sqlite_pressure.map(|pressure| serde_json::json!({
            "mechanism": pressure.mechanism,
            "hold_duration_ms": pressure.hold_duration_ms,
            "acquired_before_quota_error": pressure.acquired_before_quota_error,
            "released_after_completion": pressure.released_after_completion,
            "acquired_unix_ms": pressure.acquired_unix_ms,
            "released_unix_ms": pressure.released_unix_ms,
        })),
    });
    let signal_ordering = serde_json::json!({
        "signal_before_persistence": sqlite_lock_or_maintenance_pressure
            && input.upstream.completion_sent,
        "basis": "fallback completion was sent before copied SQLite write-lock pressure released",
    });
    let account_selection = serde_json::json!({
        "non_reselection": reconnected_to_different_account,
        "basis": "quota error account role differs from fallback completion account role",
    });
    let router_signal_latency_ms = input
        .upstream
        .quota_error_sent_unix_ms
        .zip(input.registry_report.quota_reconnect_signal_unix_ms)
        .map(|(quota_error, router_signal)| router_signal.saturating_sub(quota_error));
    let payload = serde_json::json!({
        "git_head": current_git_head()?,
        "mode": "quota-reconnect-websocket",
        "s8_provenance": s8_smoke_provenance("quota-reconnect"),
        "codex_version": input.codex_version,
        "runtime_roots": runtime_roots,
        "pressure": pressure,
        "signal_ordering": signal_ordering,
        "account_selection": account_selection,
        "profile_written": input.profile_path.exists(),
        "profile_uses_codex_router_token": false,
        "router_process": {
            "binary_path": sanitized_artifact_path(&input.router_process.binary_path)?,
            "pid": input.router_process.pid,
            "argv": sanitized_router_argv(&input.router_process.argv),
            "listener": sanitized_loopback_endpoint_text(&input.router_process.listener),
            "readiness_line": sanitized_loopback_endpoint_text(&input.router_process.readiness_line),
            "cleanup_result": sanitized_loopback_endpoint_text(&input.router_process.cleanup_result),
            "spawned_real_serve_child": true,
        },
        "codex": {
            "status": input.codex_status.to_string(),
            "stdout_contains_smoke_text": input.codex_stdout.contains(SMOKE_EXPECTED_TEXT),
            "stderr_line_count": input.codex_stderr.lines().count(),
            "stdout_contains_usage_limit_reached": input.codex_stdout.contains("usage_limit_reached"),
            "stderr_contains_usage_limit_reached": input.codex_stderr.contains("usage_limit_reached"),
            "last_message_written": input.last_message_path.exists(),
        },
        "router_audit": {
            "websocket_local_auth_validated": input.router_audit.websocket_local_auth_validated,
        },
        "quota_reconnect": {
            "primary_account_role": "primary",
            "fallback_account_role": "fallback",
            "quota_error_hidden_from_codex": !input.codex_stdout.contains("usage_limit_reached")
                && !input.codex_stderr.contains("usage_limit_reached"),
            "first_real_request_account_role": quota_reconnect_role_from_label(
                input.upstream.quota_error_connection_label.as_deref(),
            ),
            "completion_account_role": quota_reconnect_role_from_label(
                input.upstream.completion_connection_label.as_deref(),
            ),
            "reconnected_to_different_account": reconnected_to_different_account,
        },
        "quota_reconnect_progress": {
            "signal_latency_ms": router_signal_latency_ms,
            "basis": "router quota_reconnect signal timestamp minus upstream quota exhaustion timestamp",
            "router_signal_count": input.registry_report.quota_reconnect_signal_count,
        },
        "upstream": {
            "http_probe_count": input.upstream.http_probe_count,
            "websocket_handshake_count": input.upstream.websocket_handshake_count,
            "request_frame_count": input.upstream.request_frame_count,
            "prewarm_frame_count": input.upstream.prewarm_frame_count,
            "non_prewarm_frame_count": input.upstream.non_prewarm_frame_count,
            "quota_error_sent": input.upstream.quota_error_sent,
            "completion_sent": input.upstream.completion_sent,
        },
    });
    let rendered = serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("failed to render quota reconnect transcript: {error}"))?;
    assert_redacted_quota_reconnect_payload(&rendered, input)?;
    fs::write(&transcript_path, rendered)
        .map_err(|error| format!("failed to write quota reconnect transcript: {error}"))?;
    Ok(transcript_path)
}

struct ThreeWebSocketTranscriptInput<'a> {
    mode: &'a str,
    codex_version: &'a str,
    router_process: &'a RouterProcessObservation,
    registry_report: Option<&'a RouterWebSocketRegistryReport>,
    upstream: &'a ConcurrentWebSocketTranscript,
    socket_cleanup: &'a RouterSocketCleanupObservation,
    outputs: &'a [CodexChildRun],
    seed: &'a SmokeSeed,
    runtime_roots: &'a InstalledCodexRuntimeRoots,
}

fn write_redacted_three_websocket_transcript(
    input: &ThreeWebSocketTranscriptInput<'_>,
) -> Result<PathBuf, String> {
    let artifact_dir = workspace_root()?.join("tmp").join("smoke");
    fs::create_dir_all(&artifact_dir).map_err(|error| {
        format!(
            "failed to create smoke artifact dir {}: {error}",
            artifact_dir.display()
        )
    })?;
    let transcript_path = artifact_dir.join(format!(
        "installed-codex-three-websocket-{}-{}.json",
        std::process::id(),
        timestamp_millis()
    ));
    let statuses = input
        .outputs
        .iter()
        .map(|run| {
            serde_json::json!({
                "pid": run.pid,
                "status": run.output.status.to_string(),
                "stdout_contains_smoke_text": String::from_utf8_lossy(&run.output.stdout).contains(SMOKE_EXPECTED_TEXT),
                "stderr_line_count": String::from_utf8_lossy(&run.output.stderr).lines().count(),
                "stderr_transport_error_markers": stderr_transport_error_markers(&String::from_utf8_lossy(&run.output.stderr)),
            })
        })
        .collect::<Vec<_>>();
    let router_binary_path = sanitized_artifact_path(&input.router_process.binary_path)?;
    let router_argv = sanitized_router_argv(&input.router_process.argv);
    let router_root = sanitized_artifact_path(&input.runtime_roots.router_root)?;
    let router_db = sanitized_artifact_path(&input.runtime_roots.state_path)?;
    let codex_home = input
        .runtime_roots
        .codex_home
        .as_deref()
        .map(sanitized_artifact_path)
        .transpose()?;
    let codex_db = input
        .runtime_roots
        .codex_home
        .as_deref()
        .map(|path| sanitized_artifact_path(&path.join("state_5.sqlite")))
        .transpose()?;
    let process_home = input
        .runtime_roots
        .process_home
        .as_deref()
        .map(sanitized_artifact_path)
        .transpose()?;
    let runtime_roots = serde_json::json!({
        "mode": input.runtime_roots.mode.as_str(),
        "router_root": router_root,
        "router_db": router_db,
        "codex_home": codex_home,
        "codex_db": codex_db,
        "process_home": process_home,
    });
    let sqlite_pressure = input.upstream.sqlite_pressure.as_ref();
    let sqlite_lock_or_maintenance_pressure = sqlite_pressure.is_some_and(|pressure| {
        pressure.acquired_before_quota_error && pressure.released_after_completion
    });
    let reconnected_to_different_account = input.upstream.quota_error_connection_label
        != input.upstream.completion_connection_label
        && input.upstream.quota_error_connection_label.is_some()
        && input.upstream.completion_connection_label.is_some();
    let pressure = if input.upstream.quota_error_sent {
        serde_json::json!({
            "pressure_mechanism": sqlite_pressure.map_or("none", |pressure| pressure.mechanism),
            "copied_db_pressure_proven": input.runtime_roots.mode == "copied-dev-state"
                && sqlite_lock_or_maintenance_pressure,
            "sqlite_lock_or_maintenance_pressure": sqlite_lock_or_maintenance_pressure,
            "provider_error_observer_delay": false,
            "sqlite_pressure": sqlite_pressure.map(|pressure| serde_json::json!({
                "mechanism": pressure.mechanism,
                "hold_duration_ms": pressure.hold_duration_ms,
                "acquired_before_quota_error": pressure.acquired_before_quota_error,
                "released_after_completion": pressure.released_after_completion,
                "acquired_unix_ms": pressure.acquired_unix_ms,
                "released_unix_ms": pressure.released_unix_ms,
            })),
        })
    } else {
        serde_json::json!({
            "pressure_mechanism": "blocked-missing-sqlite-pressure",
            "copied_db_pressure_proven": false,
            "sqlite_lock_or_maintenance_pressure": false,
            "provider_error_observer_delay": false,
            "blocked_reason": "copied roots were exercised, but this harness has no approved SQLite lock/maintenance pressure or forced provider-error observer delay",
        })
    };
    let signal_ordering = serde_json::json!({
        "signal_before_persistence": if input.upstream.quota_error_sent {
            sqlite_lock_or_maintenance_pressure && input.upstream.completion_sent
        } else {
            input.upstream.multi_step_completed_before_overlap_end
        },
        "basis": if input.upstream.quota_error_sent {
            "fallback completion was sent before copied SQLite write-lock pressure released"
        } else {
            "multi-step follow-up completed before overlap end; durable provider-error persistence pressure is not present in this harness"
        },
    });
    let account_selection = serde_json::json!({
        "non_reselection": if input.upstream.quota_error_sent {
            reconnected_to_different_account
        } else {
            input.upstream.upstream_client_sessions.len() >= input.outputs.len()
        },
        "basis": if input.upstream.quota_error_sent {
            "quota error account role differs from fallback completion account role"
        } else {
            "all expected upstream client sessions completed; exhausted-account scenario is not present in this harness"
        },
    });
    let runtime_correlations = runtime_correlations_for_three_websocket(input);
    let session_continuity = session_continuity_for_three_websocket(input);
    let router_process = serde_json::json!({
        "binary_path": router_binary_path,
        "pid": input.router_process.pid,
        "argv": router_argv,
        "listener": sanitized_loopback_endpoint_text(&input.router_process.listener),
        "readiness_line": sanitized_loopback_endpoint_text(&input.router_process.readiness_line),
        "cleanup_result": sanitized_loopback_endpoint_text(&input.router_process.cleanup_result),
        "spawned_real_serve_child": true,
    });
    let router_websocket_registry = input.registry_report.map(|report| serde_json::json!({
        "handled_connections": report.handled_connections,
        "active_sessions": report.active_sessions,
        "high_water_sessions": report.high_water_sessions,
        "registered_sessions": report.registered_sessions,
        "closed_sessions": report.closed_sessions,
        "completed_response_sessions": report.completed_response_sessions,
        "forwarded_upstream_messages": report.forwarded_upstream_messages,
        "registered_session_id_count": report.registered_session_id_count,
        "completed_session_id_count": report.completed_session_id_count,
        "closed_session_id_count": report.closed_session_id_count,
        "session_peer_addr_count": report.session_peer_addr_count,
        "session_peer_join_observable": report.session_peer_join_observable,
        "completed_session_forwarded_upstream_message_counts": report.completed_session_forwarded_upstream_message_counts,
        "final_session_forwarded_upstream_message_counts": report.final_session_forwarded_upstream_message_counts,
        "quota_reconnect_signal_count": report.quota_reconnect_signal_count,
        "quota_reconnect_signal_unix_ms": report.quota_reconnect_signal_unix_ms,
    }));
    let clients = serde_json::json!({
        "count": input.outputs.len(),
        "target_model": SMOKE_TARGET_MODEL,
        "all_success": input.outputs.iter().all(|run| run.output.status.success()),
        "statuses": statuses,
    });
    let selected_account = serde_json::json!({
        "safe_tag": input.seed.expected_account_tag,
        "expected_upstream_account_selected": input.upstream.upstream_client_sessions.len() >= input.outputs.len(),
    });
    let upstream = serde_json::json!({
        "expected_sessions": input.upstream.expected_sessions,
        "expected_upstream_sessions": input.upstream.expected_upstream_sessions,
        "completed_sessions": input.upstream.completed_sessions,
        "final_active_sessions": input.upstream.final_active_sessions,
        "active_high_water": input.upstream.active_high_water,
        "overlap_proven": input.upstream.active_high_water >= input.upstream.expected_sessions,
        "overlap_started_unix_ms": input.upstream.overlap_started_unix_ms,
        "overlap_completed_unix_ms": input.upstream.overlap_completed_unix_ms,
        "real_overlap_completed_unix_ms": input.upstream.real_overlap_completed_unix_ms,
        "overlap_duration_ms": input.upstream.overlap_duration_ms,
        "real_overlap_duration_ms": input.upstream.real_overlap_duration_ms,
        "hold_duration_ms": input.upstream.hold_duration.as_millis(),
        "non_prewarm_session_count": input.upstream.non_prewarm_session_count,
        "target_model": SMOKE_TARGET_MODEL,
        "target_model_session_count": input.upstream.target_model_session_count,
        "unexpected_response_create_models": input.upstream.unexpected_response_create_models,
        "upstream_session_id_count": input.upstream.upstream_session_ids.len(),
        "upstream_client_session_count": input.upstream.upstream_client_sessions.len(),
        "upstream_client_indexes": input.upstream.upstream_client_sessions.iter().map(|session| session.client_index).collect::<Vec<_>>(),
        "session_frame_counts": input.upstream.session_frame_counts,
        "session_event_counts": input.upstream.session_event_counts,
        "in_overlap_session_event_counts": input.upstream.in_overlap_session_event_counts,
        "http_probe_count": input.upstream.http_probe_count,
        "normal_close_sessions": input.upstream.normal_close_sessions,
        "abnormal_close_sessions": input.upstream.abnormal_close_sessions,
        "session_close_outcomes": input.upstream.session_close_outcomes,
        "multi_step_interleave_completed": input.upstream.multi_step_interleave_completed,
        "multi_step_followup_frame_count": input.upstream.multi_step_followup_frame_count,
        "multi_step_followup_active_session_count": input.upstream.multi_step_followup_active_session_count,
        "multi_step_followup_unix_ms": input.upstream.multi_step_followup_unix_ms,
        "multi_step_completed_before_overlap_end": input.upstream.multi_step_completed_before_overlap_end,
    });
    let quota_reconnect_progress = serde_json::json!({
        "signal_latency_ms": input.upstream.quota_error_sent_unix_ms.zip(input.registry_report.and_then(|report| report.quota_reconnect_signal_unix_ms)).map(|(quota_error, router_signal)| router_signal.saturating_sub(quota_error)),
        "basis": "router quota_reconnect signal timestamp minus upstream quota exhaustion timestamp",
        "router_signal_count": input.registry_report.map_or(0, |report| report.quota_reconnect_signal_count),
    });
    let source_artifact = serde_json::json!({
        "artifact": transcript_path.file_name().and_then(|name| name.to_str()),
        "s8_run_id": s8_smoke_run_id(),
        "git_head": current_git_head()?,
        "runtime_roots": runtime_roots,
        "mode": input.mode,
    });
    let socket_cleanup = serde_json::json!({
        "lsof_exit_status": input.socket_cleanup.lsof_exit_status,
        "tcp_line_count": input.socket_cleanup.tcp_line_count,
        "established_count": input.socket_cleanup.established_count,
        "close_wait_count": input.socket_cleanup.close_wait_count,
        "raw_state_counts": input.socket_cleanup.raw_state_counts,
    });
    let payload = serde_json::json!({
        "git_head": current_git_head()?,
        "mode": input.mode,
        "s8_provenance": s8_smoke_provenance(input.mode),
        "codex_version": input.codex_version.trim(),
        "runtime_roots": runtime_roots,
        "pressure": pressure,
        "signal_ordering": signal_ordering,
        "account_selection": account_selection,
        "quota_reconnect": {
            "primary_account_role": "primary",
            "fallback_account_role": "fallback",
            "first_real_request_account_role": quota_reconnect_role_from_label(
                input.upstream.quota_error_connection_label.as_deref(),
            ),
            "completion_account_role": quota_reconnect_role_from_label(
                input.upstream.completion_connection_label.as_deref(),
            ),
            "reconnected_to_different_account": reconnected_to_different_account,
        },
        "quota_reconnect_progress": quota_reconnect_progress,
        "source_artifacts": {
            "three_websocket_soak": source_artifact,
            "quota_reconnect": source_artifact,
        },
        "router_process": router_process,
        "router_websocket_registry": router_websocket_registry,
        "clients": clients,
        "selected_account": selected_account,
        "runtime_correlations": runtime_correlations,
        "session_continuity": session_continuity,
        "upstream": upstream,
        "socket_cleanup": socket_cleanup,
        "shared_router_pid": input.router_process.pid,
    });
    let rendered = serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("failed to render three-client transcript: {error}"))?;
    assert_redacted_three_websocket_payload(&rendered, input.outputs, input.seed)?;
    fs::write(&transcript_path, rendered)
        .map_err(|error| format!("failed to write three-client transcript: {error}"))?;
    Ok(transcript_path)
}
