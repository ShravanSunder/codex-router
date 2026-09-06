use super::*;

#[test]
fn serve_command_starts_runtime_and_forwards_one_loopback_request() {
    let test_root = TestRoot::new("serve-command");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let account_id = account_id("acct_cli_serve");
    let account = AccountRecord::new(account_id.clone(), "cli-serve", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 100);
    must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
    persist_effective_selector_window(&state, &account_id, "responses", 100);
    let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
    let upstream_credential_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "cli-upstream-token",
            Some("cli-upstream-refresh-token".to_owned()),
        )
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

    let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let upstream_address = must_ok(upstream_listener.local_addr());
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (mut stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept: {error}"),
        };
        let request = read_http_request_with_body(&mut stream);
        if let Err(error) = upstream_sender.send(request) {
            panic!("mock upstream request should record: {error}");
        }
        if let Err(error) =
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
        {
            panic!("mock upstream should write response: {error}");
        }
    });
    let router_port = reserve_loopback_port();
    let router_port_text = router_port.to_string();
    let upstream_base_url = format!("http://{upstream_address}/v1");
    let client_thread = thread::spawn(move || {
        send_tokenless_loopback_request_with_retry(
            router_port,
            br#"{"model":"gpt-5","serve":true}"#,
        )
    });

    let output = run_cli(
        [
            "codex-router",
            "serve",
            "--listen-host",
            "127.0.0.1",
            "--port",
            router_port_text.as_str(),
            "--state-db",
            path_to_str(&state_path),
            "--secret-root",
            path_to_str(&secret_root),
            "--upstream-base-url",
            upstream_base_url.as_str(),
            "--now-unix-seconds",
            "1030",
            "--max-snapshot-age-seconds",
            "60",
            "--disable-background-quota-refresh",
            "--max-connections",
            "1",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(
        output
            .stdout
            .contains(format!("listening: 127.0.0.1:{router_port}\n").as_str())
    );
    assert!(output.stderr.is_empty());
    let client_response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };
    assert!(client_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(client_response.ends_with("\r\ndata: ok\n\n"));

    let upstream_request = match upstream_receiver.recv() {
        Ok(request) => request,
        Err(error) => panic!("mock upstream request should be recorded: {error}"),
    };
    assert!(upstream_request.starts_with("POST /v1/responses HTTP/1.1\r\n"));
    assert!(upstream_request.contains("authorization: Bearer cli-upstream-token\r\n"));
    assert!(!upstream_request.contains("X-Codex-Router-Token"));
    assert!(!upstream_request.contains("current-token"));

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}

#[test]
fn serve_command_defaults_to_live_runtime_clock_with_quota_freshness_margin() {
    let command = match CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--state-db"),
        OsString::from("/tmp/codex-router-state.sqlite"),
        OsString::from("--secret-root"),
        OsString::from("/tmp/codex-router-secrets"),
        OsString::from("--upstream-base-url"),
        OsString::from("http://127.0.0.1:1/v1"),
    ]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(other) => panic!("serve command should parse, got {other:?}"),
        Err(error) => panic!("serve command should parse: {error}"),
    };

    assert_eq!(command.now_unix_seconds, None);
    assert!(
        command.quota_refresh_interval_seconds < command.max_snapshot_age_seconds,
        "background refresh must run before live selector evidence expires"
    );
    assert!(
        command.quota_refresh_interval_seconds
            < crate::quota::DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS,
        "background refresh must run before persisted selector evidence becomes stale"
    );
}

#[test]
fn serve_command_accepts_explicit_fixed_quota_clock() {
    let command = match CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--now-unix-seconds"),
        OsString::from("12345"),
    ]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(other) => panic!("serve command should parse, got {other:?}"),
        Err(error) => panic!("serve command should parse: {error}"),
    };

    assert_eq!(command.now_unix_seconds, Some(12_345));
}

#[test]
fn serve_command_defaults_to_home_router_paths_and_provider_upstream() {
    let command = match CliCommand::parse([OsString::from("serve")]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(other) => panic!("serve command should parse, got {other:?}"),
        Err(error) => panic!("serve command should parse: {error}"),
    };

    let router_root = default_router_root_for_test();
    assert_eq!(command.state_db, router_root.join("state.sqlite"));
    assert_eq!(command.secret_root, router_root.join("secrets"));
    assert_eq!(
        command.upstream_base_url,
        codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL
    );
}

#[test]
#[allow(clippy::result_large_err)]
fn serve_command_dispatches_websocket_upgrade_through_runtime() {
    let test_root = TestRoot::new("serve-command-websocket");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let account_id = account_id("acct_cli_ws");
    let account = AccountRecord::new(account_id.clone(), "cli-ws", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 100);
    must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
    persist_effective_selector_window(&state, &account_id, "responses", 100);
    let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
    let upstream_credential_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "cli-ws-upstream-token",
            Some("cli-ws-upstream-refresh-token".to_owned()),
        )
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

    let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let upstream_address = must_ok(upstream_listener.local_addr());
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock websocket upstream should accept: {error}"),
        };
        let mut websocket = match accept_hdr(stream, |request: &Request, response: Response| {
            let authorization = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>")
                .to_owned();
            let local_token = request
                .headers()
                .get("x-codex-router-token")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if let Err(error) = upstream_sender.send((authorization, local_token)) {
                panic!("mock websocket upstream headers should record: {error}");
            }
            Ok(response)
        }) {
            Ok(websocket) => websocket,
            Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
        };
        let first_frame = match websocket.read() {
            Ok(message) => message,
            Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
        };
        if let Err(error) = upstream_sender.send((first_frame.to_string(), None)) {
            panic!("mock websocket upstream first frame should record: {error}");
        }
        if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
            panic!("mock websocket upstream should send response: {error}");
        }
    });
    let router_port = reserve_loopback_port();
    let router_port_text = router_port.to_string();
    let upstream_base_url = format!("http://{upstream_address}/v1");
    let client_thread = thread::spawn(move || {
        let mut client = connect_tokenless_websocket_with_retry(router_port);
        let first_frame = r#"{"type":"response.create","cli":true}"#;
        if let Err(error) = client.send(Message::text(first_frame)) {
            panic!("local websocket client should send first frame: {error}");
        }
        match client.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("local websocket client should read response: {error}"),
        }
    });

    let output = run_cli(
        [
            "codex-router",
            "serve",
            "--listen-host",
            "127.0.0.1",
            "--port",
            router_port_text.as_str(),
            "--state-db",
            path_to_str(&state_path),
            "--secret-root",
            path_to_str(&secret_root),
            "--upstream-base-url",
            upstream_base_url.as_str(),
            "--now-unix-seconds",
            "1030",
            "--max-snapshot-age-seconds",
            "60",
            "--disable-background-quota-refresh",
            "--max-connections",
            "1",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(
        output
            .stdout
            .contains(format!("listening: 127.0.0.1:{router_port}\n").as_str())
    );
    assert!(output.stderr.is_empty());
    let client_response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };
    assert_eq!(client_response, r#"{"type":"response.completed"}"#);
    let (authorization, local_token) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream handshake should be recorded: {error}"),
    };
    assert_eq!(authorization, "Bearer cli-ws-upstream-token");
    assert_eq!(local_token, None);
    let (recorded_first_frame, _) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream first frame should be recorded: {error}"),
    };
    assert_eq!(
        recorded_first_frame,
        r#"{"type":"response.create","cli":true}"#
    );

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }
}
