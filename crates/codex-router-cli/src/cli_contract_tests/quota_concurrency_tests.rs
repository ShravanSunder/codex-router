use super::*;

#[test]
#[allow(clippy::result_large_err)]
fn served_router_http_uses_persisted_quota_while_background_refresh_is_blocked() {
    let test_root = TestRoot::new("serve-background-refresh-blocked");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let token_service = LocalRouterTokenService::new(secrets.clone());
    let local_token = must_ok(token_service.rotate_with_token("current-token"));
    let account_id = account_id("acct_background_served");
    let account = AccountRecord::new(account_id.clone(), "served", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 91);
    must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
    persist_effective_selector_window(&state, &account_id, "responses", 91);
    let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
    let upstream_credential_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "served-upstream-token",
            Some("served-upstream-refresh-token".to_owned()),
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
            Err(error) => panic!("mock HTTP upstream should accept: {error}"),
        };
        if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
            panic!("mock HTTP upstream should set read timeout: {error}");
        }
        let request = read_http_request_with_body(&mut stream);
        if let Err(error) = upstream_sender.send(("http".to_owned(), request)) {
            panic!("mock HTTP upstream request should record: {error}");
        }
        if let Err(error) =
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
        {
            panic!("mock HTTP upstream should write response: {error}");
        }
        if let Err(error) = stream.flush() {
            panic!("mock HTTP upstream should flush response: {error}");
        }
    });

    let router_port = reserve_loopback_port();
    let bind_address = must_ok(LoopbackBindAddress::new("127.0.0.1", router_port));
    let upstream_endpoint = must_ok(UpstreamEndpoint::new(format!(
        "http://{upstream_address}/v1"
    )));
    let runtime_config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        upstream_endpoint,
        state_path.clone(),
        secret_root.clone(),
        local_token.clone(),
    )
    .with_quota_clock(1_030, 60);
    let runtime = must_ok(LoopbackRouterRuntime::start(runtime_config));
    let runtime_address = runtime.local_addr();
    assert_eq!(runtime_address.port(), router_port);
    let router_thread = thread::spawn(move || {
        if let Err(error) = runtime.serve_protocol_connections(1) {
            panic!("router runtime should serve HTTP: {error}");
        }
    });

    let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
        &state_path,
        &secret_root,
        1_000,
        NoopCredentialRefreshClient,
    ));
    let (refresh_started_sender, refresh_started_receiver) = mpsc::channel();
    let (release_refresh_sender, release_refresh_receiver) = mpsc::channel();
    let provider =
        BlockingQuotaRefreshProvider::new(13, refresh_started_sender, release_refresh_receiver);
    let worker = start_background_quota_refresh_worker_with_dependencies(
        state_path,
        secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        resolver,
        provider,
        Duration::from_secs(0),
    );
    if let Err(error) = refresh_started_receiver.recv_timeout(Duration::from_secs(2)) {
        panic!("background refresh should start and block in provider: {error}");
    }

    let http_response = send_loopback_request_with_retry(
        router_port,
        local_token.token().expose_secret(),
        br#"{"model":"gpt-5","served_http":true}"#,
    );
    if let Err(error) = release_refresh_sender.send(()) {
        panic!("test should release blocked quota refresh: {error}");
    }
    drop(worker);
    let (kind, http_request) = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(recorded) => recorded,
        Err(error) => panic!("HTTP upstream request should be recorded: {error}"),
    };

    assert!(
        http_response.starts_with("HTTP/1.1 200 OK\r\n"),
        "unexpected HTTP response: {http_response:?}"
    );
    assert!(http_response.ends_with("\r\ndata: ok\n\n"));
    assert_eq!(kind, "http");
    assert!(
        http_request.starts_with("POST /v1/responses HTTP/1.1\r\n"),
        "unexpected HTTP upstream request: {http_request:?}"
    );
    assert!(
        http_request.contains("authorization: Bearer served-upstream-token\r\n"),
        "unexpected HTTP upstream request: {http_request:?}"
    );
    assert!(!http_request.contains("current-token"));

    match router_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("router thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}

#[test]
#[allow(clippy::result_large_err)]
fn served_router_websocket_uses_persisted_quota_while_background_refresh_is_blocked() {
    let test_root = TestRoot::new("serve-websocket-background-refresh-blocked");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let token_service = LocalRouterTokenService::new(secrets.clone());
    let local_token = must_ok(token_service.rotate_with_token("current-token"));
    let account_id = account_id("acct_background_served_ws");
    let account = AccountRecord::new(account_id.clone(), "served-ws", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 91);
    must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
    persist_effective_selector_window(&state, &account_id, "responses", 91);
    let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
    let upstream_credential_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "served-ws-upstream-token",
            Some("served-ws-upstream-refresh-token".to_owned()),
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
            Err(error) => panic!("mock WebSocket upstream should accept: {error}"),
        };
        let mut websocket = match accept_hdr(stream, |request: &Request, response: Response| {
            let authorization = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>")
                .to_owned();
            if let Err(error) = upstream_sender.send(("ws-auth".to_owned(), authorization)) {
                panic!("mock WebSocket upstream auth should record: {error}");
            }
            Ok(response)
        }) {
            Ok(websocket) => websocket,
            Err(error) => panic!("mock WebSocket upstream handshake should accept: {error}"),
        };
        let first_frame = match websocket.read() {
            Ok(message) => message,
            Err(error) => panic!("mock WebSocket upstream should read first frame: {error}"),
        };
        if let Err(error) = upstream_sender.send(("ws-frame".to_owned(), first_frame.to_string())) {
            panic!("mock WebSocket upstream first frame should record: {error}");
        }
        if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
            panic!("mock WebSocket upstream should send response: {error}");
        }
    });

    let router_port = reserve_loopback_port();
    let bind_address = must_ok(LoopbackBindAddress::new("127.0.0.1", router_port));
    let upstream_endpoint = must_ok(UpstreamEndpoint::new(format!(
        "http://{upstream_address}/v1"
    )));
    let runtime_config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        upstream_endpoint,
        state_path.clone(),
        secret_root.clone(),
        local_token.clone(),
    )
    .with_quota_clock(1_030, 60);
    let runtime = must_ok(LoopbackRouterRuntime::start(runtime_config));
    assert_eq!(runtime.local_addr().port(), router_port);
    let router_thread = thread::spawn(move || {
        if let Err(error) = runtime.serve_protocol_connections(1) {
            panic!("router runtime should serve WebSocket: {error}");
        }
    });

    let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
        &state_path,
        &secret_root,
        1_000,
        NoopCredentialRefreshClient,
    ));
    let (refresh_started_sender, refresh_started_receiver) = mpsc::channel();
    let (release_refresh_sender, release_refresh_receiver) = mpsc::channel();
    let provider =
        BlockingQuotaRefreshProvider::new(13, refresh_started_sender, release_refresh_receiver);
    let worker = start_background_quota_refresh_worker_with_dependencies(
        state_path,
        secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        resolver,
        provider,
        Duration::from_secs(0),
    );
    if let Err(error) = refresh_started_receiver.recv_timeout(Duration::from_secs(2)) {
        panic!("background refresh should start and block in provider: {error}");
    }

    let mut client = connect_websocket_with_retry(router_port, local_token.token().expose_secret());
    let first_frame = r#"{"type":"response.create","served_ws":true}"#;
    if let Err(error) = client.send(Message::text(first_frame)) {
        panic!("local WebSocket client should send first frame: {error}");
    }
    let websocket_response = match client.read() {
        Ok(message) => message.to_string(),
        Err(error) => panic!("local WebSocket client should read response: {error}"),
    };
    assert_eq!(websocket_response, r#"{"type":"response.completed"}"#);

    if let Err(error) = release_refresh_sender.send(()) {
        panic!("test should release blocked quota refresh: {error}");
    }
    drop(worker);
    assert_eq!(
        upstream_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| {
                panic!("WebSocket upstream auth should be recorded: {error}");
            }),
        (
            "ws-auth".to_owned(),
            "Bearer served-ws-upstream-token".to_owned()
        )
    );
    assert_eq!(
        upstream_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| {
                panic!("WebSocket upstream first frame should be recorded: {error}");
            }),
        ("ws-frame".to_owned(), first_frame.to_owned())
    );

    match router_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("router thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}
