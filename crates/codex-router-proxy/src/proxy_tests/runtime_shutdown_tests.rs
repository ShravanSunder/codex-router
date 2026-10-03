use super::*;

#[test]
fn loopback_router_runtime_cancels_unbounded_websocket_before_first_frame_timeout() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_unbounded_error_report");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
        Ok(address) => address,
        Err(error) => panic!("router bind address should validate: {error}"),
    };
    let endpoint = match UpstreamEndpoint::new("http://127.0.0.1:1/v1") {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("mock endpoint should validate: {error}"),
    };
    let config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        endpoint,
        database_path,
        secret_path,
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    );
    let runtime = match LoopbackRouterRuntime::start_for_test(config) {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown_for_thread = shutdown.clone();
    let server_thread = thread::spawn(move || {
        runtime.serve_protocol_connections_until_cancelled(usize::MAX, shutdown_for_thread)
    });

    let mut request = match format!("ws://{router_address}/v1/responses").into_client_request() {
        Ok(request) => request,
        Err(error) => panic!("local websocket request should build: {error}"),
    };
    request.headers_mut().insert(
        "X-Codex-Router-Token",
        HeaderValue::from_static("current-token"),
    );
    let (client, _response) = match connect(request) {
        Ok(connection) => connection,
        Err(error) => panic!("local websocket client should connect: {error}"),
    };

    thread::sleep(Duration::from_millis(100));
    shutdown.cancel();
    drop(client);
    match server_thread.join() {
        Ok(Ok(handled)) => assert!(handled >= 1, "server should accept websocket"),
        Ok(Err(error)) => panic!("shutdown should cancel first-frame wait cleanly: {error}"),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
}

#[test]
#[allow(clippy::result_large_err)]
fn loopback_router_runtime_shutdown_drains_active_websocket_sessions() {
    let temp_dir = ProxyTestTempDir::new("runtime_shutdown_drains_websocket");
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
        account_id("acct_ws_shutdown"),
        "ws-shutdown",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        90,
        "runtime-shutdown-token",
    );
    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock websocket upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock websocket upstream address should read: {error}"),
    };
    let (upstream_ready_sender, upstream_ready_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock websocket upstream should accept: {error}"),
        };
        let mut websocket = match accept_hdr(stream, |_request: &Request, response: Response| {
            Ok(response)
        }) {
            Ok(websocket) => websocket,
            Err(error) => {
                panic!("mock websocket upstream handshake should accept: {error}")
            }
        };
        match websocket.read() {
            Ok(_message) => {}
            Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
        }
        if let Err(error) = upstream_ready_sender.send(()) {
            panic!("mock websocket upstream readiness should send: {error}");
        }
        let _close_or_error = websocket.read();
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
        database_path,
        secret_path,
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_quota_clock(1_030, 60);
    let runtime = match LoopbackRouterRuntime::start_for_test(config) {
        Ok(runtime) => Arc::new(runtime),
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown_for_thread = shutdown.clone();
    let runtime_for_thread = Arc::clone(&runtime);
    let server_thread = thread::spawn(move || {
        runtime_for_thread
            .serve_protocol_connections_until_cancelled(usize::MAX, shutdown_for_thread)
    });
    let mut request = match format!("ws://{router_address}/v1/responses").into_client_request() {
        Ok(request) => request,
        Err(error) => panic!("local websocket request should build: {error}"),
    };
    request.headers_mut().insert(
        "X-Codex-Router-Token",
        HeaderValue::from_static("current-token"),
    );
    let (mut client, _response) = match connect(request) {
        Ok(connection) => connection,
        Err(error) => panic!("local websocket client should connect: {error}"),
    };
    if let Err(error) = client.send(Message::text(r#"{"type":"response.create"}"#)) {
        panic!("local websocket client should send first frame: {error}");
    }
    match upstream_ready_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(()) => {}
        Err(error) => panic!("upstream should receive first frame: {error}"),
    }
    assert_eq!(runtime.websocket_registry_snapshot().active_sessions, 1);

    shutdown.cancel();
    drop(client);
    match server_thread.join() {
        Ok(Ok(handled)) => assert!(handled >= 1, "server should accept websocket"),
        Ok(Err(error)) => panic!("shutdown should drain active websocket cleanly: {error}"),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("upstream thread panicked: {error:?}"),
    }
    let snapshot = runtime.websocket_registry_snapshot();
    assert_eq!(snapshot.active_sessions, 0);
    assert_eq!(snapshot.high_water_sessions, 1);
    assert_eq!(snapshot.closed_sessions, 1);
}

#[test]
#[allow(clippy::result_large_err)]
fn loopback_router_runtime_continues_after_rejected_connection() {
    let temp_dir = ProxyTestTempDir::new("runtime_rejected_then_websocket");
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
        account_id("acct_ws_after_reject"),
        "ws-after-reject",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        90,
        "runtime-after-reject-token",
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
    let config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        endpoint,
        database_path,
        secret_path,
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_quota_clock(1_030, 60);
    let runtime = match LoopbackRouterRuntime::start_for_test(config) {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let rejected_client_thread = thread::spawn(move || {
        let mut client = match TcpStream::connect(router_address) {
            Ok(client) => client,
            Err(error) => panic!("rejected client should connect: {error}"),
        };
        if let Err(error) = client.write_all(
            b"GET /v1/unsupported HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n",
        ) {
            panic!("rejected client request should write: {error}");
        }
        if let Err(error) = client.shutdown(Shutdown::Write) {
            panic!("rejected client should shutdown write side: {error}");
        }
        let mut ignored_response = String::new();
        let _ = client.read_to_string(&mut ignored_response);
    });
    let websocket_client_thread = thread::spawn(move || {
        match rejected_client_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("rejected client thread panicked: {error:?}"),
        }
        let mut request = match format!("ws://{router_address}/v1/responses").into_client_request()
        {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        request.headers_mut().insert(
            "X-Codex-Router-Token",
            HeaderValue::from_static("current-token"),
        );
        let (mut client, _response) = match connect(request) {
            Ok(connection) => connection,
            Err(error) => panic!("local websocket client should connect after reject: {error}"),
        };
        if let Err(error) = client.send(Message::text(
            r#"{"type":"response.create","after_reject":true}"#,
        )) {
            panic!("local websocket client should send first frame: {error}");
        }
        match client.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("local websocket client should read response: {error}"),
        }
    });

    let handled = match runtime.serve_protocol_connections(2) {
        Ok(handled) => handled,
        Err(error) => {
            panic!("router runtime should continue after rejected connection: {error}")
        }
    };
    assert_eq!(handled, 2);
    let client_response = match websocket_client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("websocket client thread panicked: {error:?}"),
    };
    assert_eq!(client_response, r#"{"type":"response.completed"}"#);
    let authorization = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream handshake should record: {error}"),
    };
    assert_eq!(authorization, "Bearer runtime-after-reject-token");
    let recorded_first_frame = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream first frame should record: {error}"),
    };
    assert_eq!(
        recorded_first_frame,
        r#"{"type":"response.create","after_reject":true}"#
    );

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }
}
