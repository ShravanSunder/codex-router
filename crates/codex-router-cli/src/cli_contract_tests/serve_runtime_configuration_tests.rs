use super::*;
use std::ffi::OsString;

#[test]
fn serve_flag_reaches_runtime_configuration() {
    let command = match CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--session-pin-idle-ttl-seconds"),
        OsString::from("1800"),
    ]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(_) => panic!("serve arguments should parse as a serve command"),
        Err(error) => panic!("serve arguments should parse: {error}"),
    };
    let runtime_config = base_serve_runtime_config(&command)
        .unwrap_or_else(|error| panic!("serve runtime config should build: {error}"));
    let expected_config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new(&command.listen_host, command.port)
            .expect("serve bind address should be valid"),
        UpstreamEndpoint::new(command.upstream_base_url.clone())
            .expect("serve upstream endpoint should be valid"),
        command.state_db,
        command.secret_root,
    )
    .with_session_pin_idle_ttl(Duration::from_secs(1_800))
    .with_claude_five_hour_reserve_percent(command.claude_five_hour_reserve_percent);

    assert_eq!(runtime_config, expected_config);
}

#[tokio::test]
async fn serve_quota_refresh_interval_reaches_runtime_configuration() {
    let root = tempfile::tempdir().expect("isolated actual preparation root");
    let gate = codex_router_descriptor_boundary::DescriptorGate::global();
    let listener = codex_router_descriptor_boundary::OwnedListener::bind_tcp(
        "127.0.0.1:0".parse().expect("loopback fixture"),
        gate,
    )
    .await
    .expect("actual listening descriptor");
    let port = listener
        .tcp_address()
        .expect("kernel-assigned positive port")
        .port();
    let command = match CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--quota-refresh-interval-seconds"),
        OsString::from("400"),
        OsString::from("--port"),
        OsString::from(port.to_string()),
        OsString::from("--state-db"),
        root.path().join("state.sqlite").into_os_string(),
        OsString::from("--secret-root"),
        root.path().join("secrets").into_os_string(),
    ]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(_) => panic!("serve arguments should parse as a serve command"),
        Err(error) => panic!("serve arguments should parse: {error}"),
    };
    let base_config = base_serve_runtime_config(&command)
        .unwrap_or_else(|error| panic!("serve runtime config should build: {error}"));
    let config = crate::serve_command::build_serve_role_config(base_config, &command);
    assert_eq!(config.quota_refresh_interval, Duration::from_secs(400));
    let credentials = codex_router_secret_store::test_support::open_encrypted_credential_store(
        &command.secret_root,
    )
    .expect("explicit external Keychain fixture");
    let prepared = agent_proxy_services::test_support::prepare_fresh_with_fixture_credentials(
        config,
        credentials,
        listener,
        gate,
    )
    .await
    .expect("real production role preparation and token/interval propagation");
    let local_token = LocalRouterTokenService::new(
        FileSecretStore::open_read_only(&command.secret_root).expect("actual created token store"),
    )
    .load_current()
    .expect("actual role-created local token");
    let expected_config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new(&command.listen_host, command.port)
            .expect("serve bind address should be valid"),
        UpstreamEndpoint::new(command.upstream_base_url.clone())
            .expect("serve upstream endpoint should be valid"),
        command.state_db,
        command.secret_root,
    )
    .with_session_pin_idle_ttl(Duration::from_secs(command.session_pin_idle_ttl_seconds))
    .with_claude_edge_local_token(local_token, Duration::from_secs(400))
    .with_claude_five_hour_reserve_percent(command.claude_five_hour_reserve_percent);
    assert_eq!(prepared.core_configuration(), &expected_config);
}

#[test]
fn claude_five_hour_reserve_percent_reaches_route_profile_configuration() {
    let command = match CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--claude-five-hour-reserve-percent"),
        OsString::from("90"),
    ]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(_) => panic!("serve arguments should parse as a serve command"),
        Err(error) => panic!("serve arguments should parse: {error}"),
    };

    let runtime_config = base_serve_runtime_config(&command)
        .unwrap_or_else(|error| panic!("serve runtime config should build: {error}"));
    let expected_config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new(&command.listen_host, command.port)
            .expect("serve bind address should be valid"),
        UpstreamEndpoint::new(command.upstream_base_url.clone())
            .expect("serve upstream endpoint should be valid"),
        command.state_db,
        command.secret_root,
    )
    .with_session_pin_idle_ttl(Duration::from_secs(command.session_pin_idle_ttl_seconds))
    .with_claude_five_hour_reserve_percent(command.claude_five_hour_reserve_percent);

    assert_eq!(runtime_config, expected_config);
}

#[cfg(debug_assertions)]
#[test]
fn isolated_debug_claude_override_reaches_runtime_without_changing_codex_upstream() {
    let command = match CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--upstream-base-url"),
        OsString::from("https://codex.example/v1"),
        OsString::from("--require-debug-isolation"),
        OsString::from("--debug-claude-upstream-base-url"),
        OsString::from("http://127.0.0.1:19888"),
    ]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(_) => panic!("serve arguments should parse as a serve command"),
        Err(error) => panic!("serve arguments should parse: {error}"),
    };
    let runtime_config = base_serve_runtime_config(&command)
        .unwrap_or_else(|error| panic!("debug serve runtime config should build: {error}"));
    let expected_config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new(&command.listen_host, command.port)
            .expect("serve bind address should be valid"),
        UpstreamEndpoint::new("https://codex.example/v1")
            .expect("Codex upstream should remain independently configured"),
        command.state_db,
        command.secret_root,
    )
    .with_session_pin_idle_ttl(Duration::from_secs(command.session_pin_idle_ttl_seconds))
    .with_claude_five_hour_reserve_percent(command.claude_five_hour_reserve_percent)
    .with_debug_claude_upstream_endpoint(
        ClaudeUpstreamEndpoint::isolated_debug_override("http://127.0.0.1:19888", true)
            .expect("debug endpoint should be isolated and valid"),
    );

    assert_eq!(runtime_config, expected_config);
}

#[cfg(debug_assertions)]
#[test]
fn invalid_debug_claude_url_is_reported_without_echoing_the_value() {
    let supplied_url = "http://user:token@127.0.0.1:19888?secret=value";
    let command = match CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--require-debug-isolation"),
        OsString::from("--debug-claude-upstream-base-url"),
        OsString::from(supplied_url),
    ]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(_) => panic!("serve arguments should parse as a serve command"),
        Err(error) => panic!("serve arguments should parse: {error}"),
    };
    let error = base_serve_runtime_config(&command)
        .expect_err("query string must not be accepted in a debug Claude base URL");
    let message = error.to_string();

    assert!(message.contains("--debug-claude-upstream-base-url"));
    assert!(message.contains("CODEX_ROUTER_DEBUG_CLAUDE_UPSTREAM_BASE_URL"));
    assert!(message.contains("absolute HTTP(S) base URL"));
    assert!(!message.contains(supplied_url));
}
