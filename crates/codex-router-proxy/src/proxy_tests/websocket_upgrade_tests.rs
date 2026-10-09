use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::result_large_err)]
async fn loopback_router_runtime_accepts_fragmented_websocket_upgrade() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_fragmented_upgrade");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
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
        account_id("acct_ws_fragmented"),
        "ws-fragmented",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        90,
        "fragmented-ws-upstream-token",
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
            if let Err(error) = upstream_sender.send(authorization) {
                panic!("mock websocket upstream auth should record: {error}");
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
        if let Err(error) = upstream_sender.send(first_frame.to_string()) {
            panic!("mock websocket upstream first frame should record: {error}");
        }
        if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
            panic!("mock websocket upstream should send response: {error}");
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
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        bind_address,
        endpoint,
        database_path,
        secret_path,
    )
    .with_quota_clock(1_030, 60);
    let runtime = match LoopbackRouterRuntime::start_for_test(config).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let client_thread = thread::spawn(move || {
        let mut client = match TcpStream::connect(router_address) {
            Ok(client) => client,
            Err(error) => panic!("fragmented client should connect: {error}"),
        };
        let request = format!(
            "GET /v1/responses HTTP/1.1\r\nHost: {router_address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        let split_at = request
            .find("Upgrade: websocket")
            .unwrap_or_else(|| panic!("test request should contain upgrade header"));
        if let Err(error) = client.write_all(&request.as_bytes()[..split_at]) {
            panic!("fragmented client should write first header fragment: {error}");
        }
        thread::sleep(Duration::from_millis(50));
        if let Err(error) = client.write_all(&request.as_bytes()[split_at..]) {
            panic!("fragmented client should write second header fragment: {error}");
        }

        let handshake_response = read_http_response_headers(&mut client);
        assert!(
            handshake_response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
            "fragmented websocket handshake should complete, got:\n{handshake_response}"
        );
        let mut websocket = WebSocket::from_raw_socket(client, Role::Client, None);
        let first_frame = r#"{"type":"response.create","fragmented":true}"#;
        if let Err(error) = websocket.send(Message::text(first_frame)) {
            panic!("fragmented websocket client should send first frame: {error}");
        }
        match websocket.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("fragmented websocket client should read response: {error}"),
        }
    });

    let handled = match runtime.serve_protocol_connections(1).await {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve fragmented websocket: {error}"),
    };
    assert_eq!(handled, 1);
    let client_response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("fragmented websocket client thread panicked: {error:?}"),
    };
    assert_eq!(client_response, r#"{"type":"response.completed"}"#);
    let authorization = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream auth should be recorded: {error}"),
    };
    assert_eq!(authorization, "Bearer fragmented-ws-upstream-token");
    let recorded_first_frame = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream first frame should be recorded: {error}"),
    };
    assert_eq!(
        recorded_first_frame,
        r#"{"type":"response.create","fragmented":true}"#
    );

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::result_large_err)]
async fn served_new_websocket_weekly_floor_routes_only_to_eligible_peer() {
    const NOW: u64 = 1_030;
    let temp_dir = ProxyTestTempDir::new("served_new_websocket_weekly_floor");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let state = SqliteStateStore::open(&database_path).expect("state should open");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .expect("secrets should open");
    let protected = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_served_ws_protected"),
        "protected",
        AccountStatus::Enabled,
    );
    let peer = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_served_ws_peer"),
        "peer",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &protected, 5, "protected-ws-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &peer, 80, "peer-ws-token");
    refresh_served_floor_windows_for_test(&state, &protected, NOW, 5);
    refresh_served_floor_windows_for_test(&state, &peer, NOW, 80);
    drop(state);
    set_weekly_floor_for_test(&database_path, protected.label(), 500).await;

    let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("upstream should bind");
    let upstream_address = upstream_listener.local_addr().expect("address should read");
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (stream, _) = upstream_listener
            .accept()
            .expect("peer websocket should arrive");
        let mut websocket = accept_hdr(stream, |request: &Request, response: Response| {
            let authorization = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>")
                .to_owned();
            upstream_sender
                .send(authorization)
                .expect("authorization should record");
            Ok(response)
        })
        .expect("upstream handshake should accept");
        let first_frame = websocket.read().expect("response.create should arrive");
        upstream_sender
            .send(first_frame.to_string())
            .expect("first frame should record");
        websocket
            .send(Message::text(r#"{"type":"response.completed"}"#))
            .expect("completed response should send");
    });

    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new("127.0.0.1", 0).expect("bind should validate"),
        UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
            .expect("endpoint should validate"),
        database_path,
        secret_path,
    )
    .with_quota_clock(NOW, 60);
    let runtime = LoopbackRouterRuntime::start_for_test(config)
        .await
        .expect("runtime should start");
    let router_address = runtime.local_addr();
    let client_thread = thread::spawn(move || {
        let mut websocket =
            connect_local_websocket_with_timeout(router_address, Duration::from_secs(2));
        websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .expect("client should send response.create");
        websocket
            .read()
            .expect("client should read response.completed")
            .to_string()
    });

    assert_eq!(
        runtime
            .serve_protocol_connections(1)
            .await
            .expect("runtime should serve websocket"),
        1
    );
    assert_eq!(
        client_thread.join().expect("client should join"),
        r#"{"type":"response.completed"}"#
    );
    let authorization = upstream_receiver
        .recv()
        .expect("upstream authorization should record");
    assert_eq!(authorization, "Bearer peer-ws-token");
    assert_eq!(authorization.matches("protected-ws-token").count(), 0);
    assert_eq!(
        upstream_receiver.recv().expect("first frame should record"),
        r#"{"type":"response.create"}"#
    );
    upstream_thread.join().expect("upstream should join");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::result_large_err)]
async fn loopback_router_runtime_accepts_http_while_websocket_is_blocked() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_concurrent_accept");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
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
        account_id("acct_ws_concurrent"),
        "ws-concurrent",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        90,
        "concurrent-upstream-token",
    );

    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock upstream address should read: {error}"),
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
            if let Err(error) = upstream_sender.send(format!("ws-auth:{authorization}")) {
                panic!("mock websocket upstream auth should record: {error}");
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
        if let Err(error) = upstream_sender.send(format!("ws-frame:{first_frame}")) {
            panic!("mock websocket upstream first frame should record: {error}");
        }

        if let Err(error) = upstream_listener.set_nonblocking(true) {
            let _ = websocket.send(Message::text(r#"{"type":"response.completed"}"#));
            panic!("mock upstream listener should become nonblocking: {error}");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let (mut http_stream, _peer_address) = loop {
            match upstream_listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        let _ = websocket.send(Message::text(r#"{"type":"response.completed"}"#));
                        panic!("router should accept HTTP while websocket handler is blocked");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    let _ = websocket.send(Message::text(r#"{"type":"response.completed"}"#));
                    panic!("mock upstream should accept concurrent HTTP request: {error}");
                }
            }
        };
        if let Err(error) = http_stream.set_nonblocking(false) {
            let _ = websocket.send(Message::text(r#"{"type":"response.completed"}"#));
            panic!("mock upstream HTTP stream should become blocking: {error}");
        }
        let http_request = read_test_http_request(&mut http_stream);
        if let Err(error) = upstream_sender.send(format!("http-request:{http_request}")) {
            panic!("mock upstream HTTP request should record: {error}");
        }
        if let Err(error) = http_stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
        {
            panic!("mock upstream should write HTTP response: {error}");
        }
        if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
            panic!("mock websocket upstream should release first tunnel: {error}");
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
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        bind_address,
        endpoint,
        database_path,
        secret_path,
    )
    .with_quota_clock(1_030, 60);
    let runtime = match LoopbackRouterRuntime::start_for_test(config).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let server_thread = tokio::spawn(async move {
        match runtime.serve_protocol_connections(2).await {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve concurrent connections: {error}"),
        }
    });
    let websocket_client_thread = thread::spawn(move || {
        let request = match format!("ws://{router_address}/v1/responses").into_client_request() {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        let (mut client, _response) = match connect(request) {
            Ok(connection) => connection,
            Err(error) => panic!("local websocket client should connect: {error}"),
        };
        if let Err(error) = client.send(Message::text(r#"{"type":"response.create"}"#)) {
            panic!("local websocket client should send first frame: {error}");
        }
        match client.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("local websocket client should read response: {error}"),
        }
    });

    let recorded_first_frame = loop {
        let recorded = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(recorded) => recorded,
            Err(error) => panic!("upstream should observe first websocket frame: {error}"),
        };
        if recorded.starts_with("ws-frame:") {
            break recorded;
        }
    };
    assert_eq!(
        recorded_first_frame,
        r#"ws-frame:{"type":"response.create"}"#
    );

    let http_response = send_loopback_request_with_read_timeout(
        router_address,
        "POST /v1/responses HTTP/1.1\r\n",
        br#"{"model":"gpt-5","concurrent":true}"#,
        Duration::from_secs(2),
    );
    assert!(http_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(http_response.ends_with("\r\nok"));

    let recorded_http = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream should record concurrent HTTP request: {error}"),
    };
    assert!(recorded_http.starts_with("http-request:POST /v1/responses HTTP/1.1\r\n"));
    assert!(recorded_http.contains("authorization: Bearer concurrent-upstream-token\r\n"));

    let websocket_response = match websocket_client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("websocket client thread panicked: {error:?}"),
    };
    assert_eq!(websocket_response, r#"{"type":"response.completed"}"#);
    match server_thread.await {
        Ok(handled) => assert_eq!(handled, 2),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}
