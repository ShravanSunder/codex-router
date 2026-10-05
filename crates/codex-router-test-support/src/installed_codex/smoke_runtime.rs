use super::*;

pub(super) fn run_installed_codex_three_websocket_mock_e2e_inner(
    config: ConcurrentWebSocketHarnessConfig,
) -> Result<InstalledCodexSmokeReport, String> {
    let smoke_root = SmokeTempRoot::new("installed-codex-three-websocket")?;
    let runtime_roots = installed_codex_runtime_roots_for_three_websocket(&smoke_root)?;
    fs::create_dir_all(&runtime_roots.router_root).map_err(|error| {
        format!(
            "failed to create temp router root {}: {error}",
            runtime_roots.router_root.display()
        )
    })?;

    let codex_version = command_output_text(Command::new("codex").arg("--version"))?;
    let sqlite_pressure = (config.quota_reconnect && runtime_roots.mode == "copied-dev-state")
        .then(|| QuotaReconnectSqlitePressureConfig::new(runtime_roots.state_path.clone()));
    let upstream = MockConcurrentWebSocketUpstream::start(config.upstream, sqlite_pressure)?;
    let seed = if config.quota_reconnect {
        seed_s8_overlap_quota_router_state(&runtime_roots.state_path, &runtime_roots.secret_root)?
    } else {
        seed_router_state(&runtime_roots.state_path, &runtime_roots.secret_root)?
    };
    let router_port = reserve_loopback_port()?;
    let audit_path = smoke_root
        .path()
        .join("three-websocket-audit")
        .join("events.jsonl");
    if let Some(audit_dir) = audit_path.parent() {
        fs::create_dir_all(audit_dir).map_err(|error| {
            format!(
                "failed to create three-websocket audit dir {}: {error}",
                audit_dir.display()
            )
        })?;
    }
    let registry_report_path = config.capture_registry_report.then(|| {
        runtime_roots
            .router_root
            .join("websocket-registry-report.json")
    });
    let router_process = start_router_process_with_options(RouterProcessStartOptions {
        now_unix_seconds: Some(1_030),
        router_port,
        state_path: runtime_roots.state_path.clone(),
        secret_root: runtime_roots.secret_root.clone(),
        local_token: None,
        upstream_base_url: format!("http://{}/v1", upstream.address()),
        audit_path,
        max_connections: config.router_max_connections,
        websocket_registry_report_file: registry_report_path.clone(),
    })?;

    let start_barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for client_index in 0..3 {
        let client_root = smoke_root.path().join(format!("client-{client_index}"));
        let codex_home = runtime_roots
            .codex_home
            .clone()
            .unwrap_or_else(|| client_root.join("codex-home"));
        let workdir = client_root.join("workdir");
        let process_home = runtime_roots
            .process_home
            .clone()
            .unwrap_or_else(|| client_root.join("home"));
        let xdg_config_home = client_root.join("xdg-config");
        let xdg_state_home = client_root.join("xdg-state");
        let xdg_cache_home = client_root.join("xdg-cache");
        for path in [
            &codex_home,
            &workdir,
            &process_home,
            &xdg_config_home,
            &xdg_state_home,
            &xdg_cache_home,
        ] {
            fs::create_dir_all(path)
                .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
        }
        let profile_writer = CodexRouterProfileWriter::new(&codex_home);
        let profile = CodexRouterProfile::new(router_port);
        profile_writer.write(&profile, true).map_err(|error| {
            format!("failed to write generated Codex profile for client {client_index}: {error}")
        })?;
        let last_message_path = client_root.join("websocket-last-message.txt");
        let child_environment = CodexChildEnvironment::new(
            &process_home,
            &xdg_config_home,
            &xdg_state_home,
            &xdg_cache_home,
        );
        let barrier = Arc::clone(&start_barrier);
        handles.push(
            thread::Builder::new()
                .name(format!("codex-router-three-client-{client_index}"))
                .spawn(move || {
                    barrier.wait();
                    let prompt = format!(
                        "{SMOKE_PROMPT}\n\nHarness marker: codex-router-client-{client_index}"
                    );
                    let output = run_codex_exec_with_timeout_observed(CodexExecRequest {
                        transport_mode: CodexTransportMode::WebSocket,
                        codex_home: &codex_home,
                        workdir: &workdir,
                        last_message_path: &last_message_path,
                        child_environment,
                        timeout: config.codex_command_timeout,
                        prompt: &prompt,
                        client_index: Some(client_index),
                    })?;
                    assert_codex_visible_output(
                        &format!("WebSocket client {client_index}"),
                        &output.output,
                        &last_message_path,
                    )?;
                    Ok::<CodexChildRun, String>(output)
                })
                .map_err(|error| {
                    format!("failed to spawn installed Codex client {client_index}: {error}")
                })?,
        );
    }

    let quota_probe_handle = if config.quota_reconnect {
        let overlap_state = Arc::clone(&upstream.state);
        let local_token = seed.local_token.clone();
        Some(
            thread::Builder::new()
                .name("codex-router-s8-overlap-quota-probe".to_owned())
                .spawn(move || {
                    wait_for_concurrent_session_barrier(&overlap_state)?;
                    run_s8_overlap_quota_local_probe(router_port, &local_token)
                })
                .map_err(|error| {
                    format!("failed to spawn S8 overlap quota local probe: {error}")
                })?,
        )
    } else {
        None
    };

    let mut outputs = Vec::new();
    let mut output_errors = Vec::new();
    for (client_index, handle) in handles.into_iter().enumerate() {
        match join_result(handle, &format!("installed Codex client {client_index}")) {
            Ok(output) => outputs.push(output),
            Err(error) => output_errors.push(format!("client{client_index}:{error}")),
        }
    }
    if let Some(handle) = quota_probe_handle
        && let Err(error) = join_result(handle, "S8 overlap quota local probe")
    {
        output_errors.push(format!("quota_probe:{error}"));
    }
    if !output_errors.is_empty() {
        return Err(format!(
            "installed Codex client failures: {}",
            output_errors.join(" | ")
        ));
    }
    let upstream_result = upstream.join()?;
    let socket_cleanup = observe_router_socket_cleanup(router_process.observation.pid)?;
    let router_process =
        router_process.wait("three-client router process", ROUTER_REGISTRY_DRAIN_TIMEOUT)?;
    let registry_report = registry_report_path
        .as_deref()
        .map(RouterWebSocketRegistryReport::from_file)
        .transpose()?;
    assert_concurrent_websocket_contract(config, &upstream_result, registry_report.as_ref())?;
    socket_cleanup.assert_no_leaked_sessions()?;
    let transcript_path =
        write_redacted_three_websocket_transcript(&ThreeWebSocketTranscriptInput {
            mode: config.artifact_mode,
            codex_version: &codex_version,
            router_process: &router_process,
            registry_report: registry_report.as_ref(),
            upstream: &upstream_result,
            socket_cleanup: &socket_cleanup,
            outputs: &outputs,
            seed: &seed,
            runtime_roots: &runtime_roots,
        })?;

    Ok(InstalledCodexSmokeReport { transcript_path })
}

pub(super) fn installed_codex_runtime_roots_for_three_websocket(
    smoke_root: &SmokeTempRoot,
) -> Result<InstalledCodexRuntimeRoots, String> {
    installed_codex_runtime_roots(smoke_root)
}

pub(super) fn installed_codex_runtime_roots(
    smoke_root: &SmokeTempRoot,
) -> Result<InstalledCodexRuntimeRoots, String> {
    let mode = std::env::var(INSTALLED_SMOKE_RUNTIME_ROOT_MODE_ENV)
        .unwrap_or_else(|_| "isolated-temp".to_owned());
    match mode.as_str() {
        "isolated-temp" => {
            let router_root = smoke_root.path().join("router");
            Ok(InstalledCodexRuntimeRoots {
                mode,
                state_path: router_root.join("state.sqlite"),
                secret_root: router_root.join("secrets"),
                router_root,
                codex_home: None,
                process_home: None,
            })
        }
        "copied-dev-state" => {
            let router_root = required_env_path(INSTALLED_SMOKE_ROUTER_ROOT_ENV)?;
            let codex_home = required_env_path(INSTALLED_SMOKE_CODEX_HOME_ENV)?;
            let process_home = required_env_path(INSTALLED_SMOKE_PROCESS_HOME_ENV)?;
            validate_copied_dev_state_roots(&router_root, &codex_home, &process_home)?;
            Ok(InstalledCodexRuntimeRoots {
                mode,
                state_path: router_root.join("state.sqlite"),
                secret_root: router_root.join("secrets"),
                router_root,
                codex_home: Some(codex_home),
                process_home: Some(process_home),
            })
        }
        other => Err(format!(
            "{INSTALLED_SMOKE_RUNTIME_ROOT_MODE_ENV} must be isolated-temp or copied-dev-state, got {other}"
        )),
    }
}

pub(super) fn required_env_path(name: &str) -> Result<PathBuf, String> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| format!("{name} is required for copied-dev-state installed Codex smoke"))
}

pub(super) fn validate_copied_dev_state_roots(
    router_root: &Path,
    codex_home: &Path,
    process_home: &Path,
) -> Result<(), String> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "test-support crate should live under workspace crates".to_owned())?;
    let dev_state_root = resolve_copied_dev_state_policy_path(
        "repo tmp/dev-state",
        &workspace_root.join("tmp/dev-state"),
    )?;
    for (label, path) in [
        ("router root", router_root),
        ("Codex home", codex_home),
        ("process HOME", process_home),
        ("router state.sqlite", &router_root.join("state.sqlite")),
        ("Codex state_5.sqlite", &codex_home.join("state_5.sqlite")),
        ("router secrets", &router_root.join("secrets")),
    ] {
        let resolved = resolve_copied_dev_state_policy_path(label, path)?;
        if !resolved.starts_with(&dev_state_root) {
            return Err(format!(
                "copied-dev-state {label} must be under repo-local tmp/dev-state; got {}",
                resolved.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn resolve_copied_dev_state_policy_path(
    label: &str,
    path: &Path,
) -> Result<PathBuf, String> {
    if path_is_symlink(path)? {
        return Err(format!(
            "copied-dev-state {label} must not be a symlink; got {}",
            path.display()
        ));
    }
    if path.exists() {
        return fs::canonicalize(path).map_err(|error| {
            format!(
                "failed to resolve copied-dev-state {label} {}: {error}",
                path.display()
            )
        });
    }
    resolve_future_path_without_following_new_leaf(path)
}

pub(super) fn path_is_symlink(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "failed to inspect copied-dev-state path {}: {error}",
            path.display()
        )),
    }
}

pub(super) fn resolve_future_path_without_following_new_leaf(
    path: &Path,
) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("failed to resolve current directory: {error}"))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    let mut missing_components = Vec::new();
    let mut existing_ancestor = normalized.as_path();
    while !existing_ancestor.exists() {
        let Some(file_name) = existing_ancestor.file_name() else {
            break;
        };
        missing_components.push(file_name.to_os_string());
        let Some(parent) = existing_ancestor.parent() else {
            break;
        };
        existing_ancestor = parent;
    }
    let mut resolved = fs::canonicalize(existing_ancestor).map_err(|error| {
        format!(
            "failed to resolve copied-dev-state ancestor {}: {error}",
            existing_ancestor.display()
        )
    })?;
    for component in missing_components.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

pub(super) fn assert_concurrent_websocket_contract(
    config: ConcurrentWebSocketHarnessConfig,
    upstream: &ConcurrentWebSocketTranscript,
    registry_report: Option<&RouterWebSocketRegistryReport>,
) -> Result<(), String> {
    if upstream.expected_sessions != config.upstream.expected_sessions {
        return Err(format!(
            "concurrent upstream expected_sessions={} did not match config {}",
            upstream.expected_sessions, config.upstream.expected_sessions
        ));
    }
    if upstream.completed_sessions != config.upstream.expected_upstream_sessions {
        return Err(format!(
            "concurrent upstream completed {} sessions, expected {}",
            upstream.completed_sessions, config.upstream.expected_upstream_sessions
        ));
    }
    if upstream.final_active_sessions != 0 {
        return Err(format!(
            "concurrent upstream final active sessions was {}, expected 0",
            upstream.final_active_sessions
        ));
    }
    if upstream.active_high_water < config.upstream.expected_sessions {
        return Err(format!(
            "concurrent upstream high-water was {}, expected at least {}",
            upstream.active_high_water, config.upstream.expected_sessions
        ));
    }
    if upstream.target_model_session_count != config.upstream.expected_upstream_sessions {
        return Err(format!(
            "concurrent upstream observed {} target-model sessions, expected {} with {SMOKE_TARGET_MODEL}",
            upstream.target_model_session_count, config.upstream.expected_upstream_sessions
        ));
    }
    if !upstream.unexpected_response_create_models.is_empty() {
        return Err(format!(
            "concurrent upstream observed unexpected response.create models: {:?}",
            upstream.unexpected_response_create_models
        ));
    }
    if config.upstream.hold_duration > Duration::ZERO {
        if upstream.real_overlap_duration_ms < config.upstream.hold_duration.as_millis() {
            return Err(format!(
                "soak real overlap duration was {}ms, expected at least {}ms",
                upstream.real_overlap_duration_ms,
                config.upstream.hold_duration.as_millis()
            ));
        }
        if !config.quota_reconnect
            && upstream
                .in_overlap_session_event_counts
                .iter()
                .any(|event_count| *event_count < 3)
        {
            return Err(format!(
                "soak in-overlap session event counts were {:?}, expected at least 3 each",
                upstream.in_overlap_session_event_counts
            ));
        }
        if upstream.in_overlap_session_event_counts.len() < config.upstream.expected_sessions {
            return Err(format!(
                "soak in-overlap session event counts {:?} had fewer entries than expected sessions {}",
                upstream.in_overlap_session_event_counts, config.upstream.expected_sessions
            ));
        }
        if upstream.upstream_session_ids.len() < config.upstream.expected_sessions {
            return Err(format!(
                "soak upstream session ids {:?} had fewer entries than expected sessions {}",
                upstream.upstream_session_ids, config.upstream.expected_sessions
            ));
        }
        if upstream.normal_close_sessions < config.upstream.expected_upstream_sessions
            || upstream.abnormal_close_sessions != 0
        {
            return Err(format!(
                "soak close outcomes were {:?}; normal={} abnormal={} expected all {} normal",
                upstream.session_close_outcomes,
                upstream.normal_close_sessions,
                upstream.abnormal_close_sessions,
                config.upstream.expected_upstream_sessions
            ));
        }
        if !config.quota_reconnect && !upstream.multi_step_interleave_completed {
            return Err("soak did not complete a multi-step WebSocket interleave".to_owned());
        }
        if !config.quota_reconnect && upstream.multi_step_followup_frame_count == 0 {
            return Err(
                "soak did not observe a follow-up local frame before completion".to_owned(),
            );
        }
        if !config.quota_reconnect
            && upstream.multi_step_followup_active_session_count < config.upstream.expected_sessions
        {
            return Err(format!(
                "multi-step follow-up saw {} active sessions, expected at least {}",
                upstream.multi_step_followup_active_session_count,
                config.upstream.expected_sessions
            ));
        }
        if !config.quota_reconnect && !upstream.multi_step_completed_before_overlap_end {
            return Err(
                "multi-step WebSocket interleave did not complete before true 3-way overlap ended"
                    .to_owned(),
            );
        }
    }
    if config.quota_reconnect {
        if !upstream.quota_error_sent || !upstream.completion_sent {
            return Err(format!(
                "S8 overlap quota did not send both quota error and completion: {upstream:?}"
            ));
        }
        if upstream.quota_error_connection_label.as_deref() != Some(QUOTA_RECONNECT_PRIMARY.label) {
            return Err(format!(
                "S8 overlap quota first real request used {:?}, expected {}",
                upstream.quota_error_connection_label, QUOTA_RECONNECT_PRIMARY.label
            ));
        }
        if upstream.completion_connection_label.as_deref() != Some(QUOTA_RECONNECT_FALLBACK.label) {
            return Err(format!(
                "S8 overlap quota completion used {:?}, expected {}",
                upstream.completion_connection_label, QUOTA_RECONNECT_FALLBACK.label
            ));
        }
        let sqlite_pressure = upstream
            .sqlite_pressure
            .as_ref()
            .ok_or_else(|| "S8 overlap quota did not record copied SQLite pressure".to_owned())?;
        if !sqlite_pressure.acquired_before_quota_error
            || !sqlite_pressure.released_after_completion
        {
            return Err(format!(
                "S8 overlap quota SQLite pressure did not cover quota reconnect: {sqlite_pressure:?}"
            ));
        }
    }
    if config.capture_registry_report {
        let registry_report =
            registry_report.ok_or_else(|| "router registry report was not captured".to_owned())?;
        if registry_report.handled_connections != Some(config.router_max_connections) {
            return Err(format!(
                "router registry handled_connections={:?}, expected final CLI report with {}",
                registry_report.handled_connections, config.router_max_connections
            ));
        }
        if registry_report.active_sessions != 0 {
            return Err(format!(
                "router registry active_sessions={} after soak; expected 0",
                registry_report.active_sessions
            ));
        }
        if registry_report.high_water_sessions < config.upstream.expected_sessions {
            return Err(format!(
                "router registry high_water_sessions={} did not prove all {} sessions overlapped",
                registry_report.high_water_sessions, config.upstream.expected_sessions
            ));
        }
        if registry_report.registered_sessions < config.upstream.expected_sessions {
            return Err(format!(
                "router registry registered_sessions={} was less than expected sessions {}",
                registry_report.registered_sessions, config.upstream.expected_sessions
            ));
        }
        if registry_report.closed_sessions < config.upstream.expected_sessions {
            return Err(format!(
                "router registry closed_sessions={} was less than expected sessions {}",
                registry_report.closed_sessions, config.upstream.expected_sessions
            ));
        }
        if registry_report.completed_response_sessions < config.upstream.expected_sessions {
            return Err(format!(
                "router registry completed_response_sessions={} was less than expected sessions {}",
                registry_report.completed_response_sessions, config.upstream.expected_sessions
            ));
        }
        if registry_report
            .final_session_forwarded_upstream_message_counts
            .len()
            < config.upstream.expected_sessions
        {
            return Err(format!(
                "router registry final-session forwarded counts {:?} had fewer entries than expected sessions {}",
                registry_report.final_session_forwarded_upstream_message_counts,
                config.upstream.expected_sessions
            ));
        }
        let mut sorted_forwarded_counts = registry_report
            .final_session_forwarded_upstream_message_counts
            .clone();
        sorted_forwarded_counts.sort_unstable_by(|left, right| right.cmp(left));
        if sorted_forwarded_counts
            .iter()
            .take(config.upstream.expected_sessions)
            .any(|count| *count < 3)
        {
            return Err(format!(
                "router registry final-session forwarded counts {:?} did not prove three unique sessions with at least three local writes",
                registry_report.final_session_forwarded_upstream_message_counts
            ));
        }
        if registry_report.forwarded_upstream_messages
            < config.upstream.expected_sessions.saturating_mul(3)
        {
            return Err(format!(
                "router registry forwarded_upstream_messages={} was less than expected minimum {}",
                registry_report.forwarded_upstream_messages,
                config.upstream.expected_sessions.saturating_mul(3)
            ));
        }
    }

    Ok(())
}
