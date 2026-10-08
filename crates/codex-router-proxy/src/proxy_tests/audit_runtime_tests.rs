use super::*;

async fn assembled_loopback_router_runtime_writes_redacted_private_audit_events() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime_audit");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let audit_path = temp_dir.path().join("audit").join("events.jsonl");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let secrets = match codex_router_secret_store::test_support::open_encrypted_credential_store(
        &secret_path,
    ) {
        Ok(secrets) => secrets,
        Err(error) => panic!("secret store should open: {error}"),
    };
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_audit_raw_id_canary"),
        "raw-account-email-canary@example.com",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        70,
        "audit-upstream-token-canary",
    );

    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock upstream address should read: {error}"),
    };
    let upstream_thread = thread::spawn(move || {
        let (mut stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => {
                panic!("mock upstream should accept only authorized request: {error}")
            }
        };
        let _request = read_test_http_request(&mut stream);
        if let Err(error) =
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
        {
            panic!("mock upstream should write response: {error}");
        }
    });
    let endpoint = match UpstreamEndpoint::new(format!("http://{upstream_address}/v1")) {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("mock upstream endpoint should validate: {error}"),
    };
    let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
        Ok(address) => address,
        Err(error) => panic!("router bind address should validate: {error}"),
    };
    let config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        endpoint,
        database_path,
        secret_path,
        LocalRouterTokenRecord::new(
            SecretString::new("audit-local-token-canary"),
            TokenGeneration::new(1),
        ),
    )
    .with_quota_clock(1_030, 60)
    .with_audit_file(audit_path.clone());
    let runtime = match LoopbackRouterRuntime::start_for_test(config).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let server_thread = tokio::spawn(async move {
        match runtime.serve_http_connections(2).await {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve audit connections: {error}"),
        }
    });

    let unauthorized_response = send_loopback_request_with_token(
        router_address,
        None,
        br#"{"prompt":"prompt-body-canary","unauthorized":true}"#,
    );
    assert!(unauthorized_response.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
    let authorized_response = send_loopback_request_with_token(
        router_address,
        Some("audit-local-token-canary"),
        br#"{"prompt":"prompt-body-canary","authorized":true}"#,
    );
    assert!(authorized_response.starts_with("HTTP/1.1 200 OK\r\n"));

    match server_thread.await {
        Ok(handled) => assert_eq!(handled, 2),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }

    let audit_contents = match fs::read_to_string(&audit_path) {
        Ok(contents) => contents,
        Err(error) => panic!("audit file should exist: {error}"),
    };
    assert_eq!(audit_contents.lines().count(), 2);
    assert!(audit_contents.contains("\"transport_kind\":\"http\""));
    assert!(audit_contents.contains("\"route_kind\":\"responses\""));
    assert!(audit_contents.contains("\"local_auth_result\":\"missing\""));
    assert!(audit_contents.contains("\"local_auth_result\":\"valid\""));
    assert!(audit_contents.contains("\"response_commit_state\":\"not_committed\""));
    assert!(audit_contents.contains("\"response_commit_state\":\"committed\""));
    assert!(audit_contents.contains("\"account_hash\""));
    assert!(!audit_contents.contains("audit-local-token-canary"));
    assert!(!audit_contents.contains("audit-upstream-token-canary"));
    assert!(!audit_contents.contains("prompt-body-canary"));
    assert!(!audit_contents.contains("raw-account-email-canary@example.com"));
    assert!(!audit_contents.contains("acct_audit_raw_id_canary"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = match fs::metadata(&audit_path) {
            Ok(metadata) => metadata.permissions().mode() & 0o777,
            Err(error) => panic!("audit metadata should read: {error}"),
        };
        assert_eq!(mode, 0o600);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assembled_loopback_router_runtime_redacts_http_and_websocket_audit_events() {
    assembled_loopback_router_runtime_writes_redacted_private_audit_events().await;
    loopback_router_runtime_dispatches_websocket_upgrade_to_tunnel().await;
}

#[allow(clippy::result_large_err)]
async fn loopback_router_runtime_dispatches_websocket_upgrade_to_tunnel() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let audit_path = temp_dir.path().join("audit").join("events.jsonl");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let secrets = match codex_router_secret_store::test_support::open_encrypted_credential_store(
        &secret_path,
    ) {
        Ok(secrets) => secrets,
        Err(error) => panic!("secret store should open: {error}"),
    };
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_ws_runtime"),
        "ws",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        90,
        "runtime-ws-upstream-token-canary",
    );

    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock websocket upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock websocket upstream address should read: {error}"),
    };
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
        if let Err(error) = websocket.send(Message::text(r#"{"type":"session.ready"}"#)) {
            panic!("mock websocket upstream should send ready response: {error}");
        }
        let established_frame = match websocket.read() {
            Ok(message) => message,
            Err(error) => {
                panic!("mock websocket upstream should read established frame: {error}")
            }
        };
        if let Err(error) = upstream_sender.send((established_frame.to_string(), None)) {
            panic!("mock websocket upstream established frame should record: {error}");
        }
        if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
            panic!("mock websocket upstream should send completed response: {error}");
        }
    });

    let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
        Ok(address) => address,
        Err(error) => panic!("router bind address should validate: {error}"),
    };
    let endpoint = match UpstreamEndpoint::new(format!("http://{upstream_address}/v1")) {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("mock endpoint should validate: {error}"),
    };
    let config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        endpoint,
        database_path.clone(),
        secret_path,
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_audit_file(audit_path.clone());
    let runtime = match LoopbackRouterRuntime::start_for_test(config).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let (ready_sender, ready_receiver) = mpsc::channel();
    let (continue_sender, continue_receiver) = mpsc::channel();
    let client_thread = thread::spawn(move || {
        let mut request = match format!("ws://{router_address}/v1/responses").into_client_request()
        {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        request.headers_mut().insert(
            "Authorization",
            HeaderValue::from_static("Bearer current-token"),
        );
        request.headers_mut().insert(
            "session-id",
            HeaderValue::from_static("assembled-websocket-session"),
        );
        let (mut client, _response) = match connect(request) {
            Ok(connection) => connection,
            Err(error) => panic!("local websocket client should connect: {error}"),
        };
        let first_frame = r#"{"type":"response.create","runtime":true}"#;
        if let Err(error) = client.send(Message::text(first_frame)) {
            panic!("local websocket client should send first frame: {error}");
        }
        let ready_response = match client.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("local websocket client should read ready response: {error}"),
        };
        assert_eq!(ready_response, r#"{"type":"session.ready"}"#);
        ready_sender
            .send(())
            .expect("client should announce established tunnel");
        continue_receiver
            .recv()
            .expect("client should await later activity time");
        let established_frame = r#"{"type":"response.create","established":true}"#;
        client
            .send(Message::text(established_frame))
            .expect("local websocket client should send established frame");
        let response = client
            .read()
            .expect("local websocket client should read completed response")
            .to_string();
        if let Err(error) = client.close(None) {
            panic!("local websocket client should close cleanly: {error}");
        }
        response
    });

    let runtime_thread = tokio::spawn(async move { runtime.serve_protocol_connections(1).await });
    ready_receiver
        .recv()
        .expect("established websocket should report ready");
    let initial_affinity =
        wait_for_session_affinity(&database_path, "assembled-websocket-session", |affinity| {
            affinity
                .account_id()
                .is_some_and(|account_id| account_id.as_str() == "acct_ws_runtime")
        })
        .await;
    let advance_deadline = std::time::Instant::now() + Duration::from_secs(2);
    while test_unix_seconds() <= initial_affinity.last_seen_unix_seconds() {
        assert!(
            std::time::Instant::now() < advance_deadline,
            "wall clock should advance for established activity proof"
        );
        thread::yield_now();
    }
    continue_sender
        .send(())
        .expect("established websocket should continue");
    let handled = match runtime_thread.await {
        Ok(Ok(handled)) => handled,
        Ok(Err(error)) => panic!("router runtime should serve websocket connection: {error}"),
        Err(error) => panic!("router runtime thread panicked: {error:?}"),
    };
    assert_eq!(handled, 1);
    let client_response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };
    assert_eq!(client_response, r#"{"type":"response.completed"}"#);
    let (authorization, local_token) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream handshake should record: {error}"),
    };
    assert_eq!(authorization, "Bearer runtime-ws-upstream-token-canary");
    assert_eq!(local_token, None);
    let (recorded_first_frame, _) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream first frame should record: {error}"),
    };
    assert_eq!(
        recorded_first_frame,
        r#"{"type":"response.create","runtime":true}"#
    );
    let (recorded_established_frame, _) = upstream_receiver
        .recv()
        .expect("upstream established frame should record");
    assert_eq!(
        recorded_established_frame,
        r#"{"type":"response.create","established":true}"#
    );

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }

    let renewed_affinity =
        wait_for_session_affinity(&database_path, "assembled-websocket-session", |affinity| {
            affinity.last_seen_unix_seconds() > initial_affinity.last_seen_unix_seconds()
        })
        .await;
    assert_eq!(
        renewed_affinity
            .account_id()
            .map(|account_id| account_id.as_str()),
        Some("acct_ws_runtime")
    );

    let audit_contents = match fs::read_to_string(&audit_path) {
        Ok(contents) => contents,
        Err(error) => panic!("websocket audit file should exist: {error}"),
    };
    assert_eq!(audit_contents.lines().count(), 1);
    assert!(audit_contents.contains("\"transport_kind\":\"web_socket\""));
    assert!(audit_contents.contains("\"route_kind\":\"responses_web_socket\""));
    assert!(audit_contents.contains("\"local_auth_result\":\"valid\""));
    assert!(audit_contents.contains("\"response_commit_state\":\"committed\""));
    assert!(audit_contents.contains("\"account_hash\""));
    assert!(!audit_contents.contains("current-token"));
    assert!(!audit_contents.contains("runtime-ws-upstream-token-canary"));
    assert!(!audit_contents.contains(r#"{"type":"response.create","runtime":true}"#));
    assert!(!audit_contents.contains("acct_ws_runtime"));
}
