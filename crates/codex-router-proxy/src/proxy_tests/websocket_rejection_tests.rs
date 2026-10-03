use super::*;

#[test]
fn loopback_router_runtime_rejects_websocket_upgrade_without_token() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_missing_token");
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
    let server_thread = thread::spawn(move || match runtime.serve_protocol_connections(1) {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve rejected websocket: {error}"),
    });

    let request = match format!("ws://{router_address}/v1/responses").into_client_request() {
        Ok(request) => request,
        Err(error) => panic!("local websocket request should build: {error}"),
    };
    let connect_succeeded = match connect(request) {
        Ok((client, _response)) => {
            drop(client);
            true
        }
        Err(_error) => false,
    };

    match server_thread.join() {
        Ok(handled) => assert_eq!(handled, 1),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    assert!(
        !connect_succeeded,
        "missing-token websocket upgrade should fail before local accept"
    );
}

#[test]
fn loopback_router_runtime_rejects_websocket_subprotocol_token_smuggling_before_accept() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_subprotocol_auth");
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
    let server_thread = thread::spawn(move || match runtime.serve_protocol_connections(1) {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve rejected websocket: {error}"),
    });

    let mut request = match format!("ws://{router_address}/v1/responses").into_client_request() {
        Ok(request) => request,
        Err(error) => panic!("local websocket request should build: {error}"),
    };
    request.headers_mut().insert(
        "X-Codex-Router-Token",
        HeaderValue::from_static("current-token"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("bearer-current-token"),
    );
    let connect_succeeded = match connect(request) {
        Ok((client, _response)) => {
            drop(client);
            true
        }
        Err(_error) => false,
    };

    match server_thread.join() {
        Ok(handled) => assert_eq!(handled, 1),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    assert!(
        !connect_succeeded,
        "subprotocol token smuggling should fail before local accept"
    );
}

#[test]
fn loopback_router_runtime_rejects_unsupported_websocket_path_before_accept() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_unsupported_path");
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
    let server_thread = thread::spawn(move || match runtime.serve_protocol_connections(1) {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve rejected websocket: {error}"),
    });

    let mut request = match format!("ws://{router_address}/v1/realtime").into_client_request() {
        Ok(request) => request,
        Err(error) => panic!("local websocket request should build: {error}"),
    };
    request.headers_mut().insert(
        "X-Codex-Router-Token",
        HeaderValue::from_static("current-token"),
    );
    let connect_succeeded = match connect(request) {
        Ok((client, _response)) => {
            drop(client);
            true
        }
        Err(_error) => false,
    };

    match server_thread.join() {
        Ok(handled) => assert_eq!(handled, 1),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    assert!(
        !connect_succeeded,
        "unsupported websocket path should fail before local accept"
    );
}

#[test]
fn loopback_router_runtime_keeps_websocket_preconnect_open_until_client_close() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_preconnect");
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
    let (done_sender, done_receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let result = runtime
            .serve_protocol_connections(1)
            .map_err(|error| error.to_string());
        if let Err(error) = done_sender.send(result) {
            panic!("server completion should send: {error}");
        }
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
    match done_receiver.recv_timeout(Duration::from_millis(750)) {
        Ok(result) => panic!("preconnect should remain open without first data: {result:?}"),
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Err(error) => panic!("server result channel should remain open: {error}"),
    }
    drop(client);
    let served_result = match done_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(error) => panic!("server should complete after client close: {error}"),
    };

    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match served_result {
        Ok(handled) => assert_eq!(handled, 1),
        Err(error) => panic!("client close before first data should be clean: {error}"),
    }
}

#[test]
#[allow(clippy::result_large_err)]
fn loopback_router_runtime_drains_affinity_tasks_after_handler_error() {
    let temp_dir = ProxyTestTempDir::new("runtime_error_drains_affinity_tasks");
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
        account_id("acct_ws_error_drain"),
        "ws-error-drain",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &account, 90, "error-drain-token");

    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock websocket upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock websocket upstream address should read: {error}"),
    };
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
        if let Err(error) = websocket.send(Message::text(
            r#"{"type":"response.completed","response":{"id":"resp_error_drain"}}"#,
        )) {
            panic!("mock websocket upstream should send completion: {error}");
        }
    });

    let (recorder_entered_sender, recorder_entered_receiver) = mpsc::channel();
    let (recorder_release_sender, recorder_release_receiver) = mpsc::channel();
    let recorder = Arc::new(BlockingAffinityOwnerRecorder::new(
        recorder_entered_sender,
        recorder_release_receiver,
    ));
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
        Ok(runtime) => runtime.with_affinity_owner_recorder(recorder.clone()),
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let (done_sender, done_receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let result = runtime
            .serve_protocol_connections(2)
            .map_err(|error| error.to_string());
        if let Err(error) = done_sender.send(result) {
            panic!("server completion should send: {error}");
        }
    });

    let mut successful_request =
        match format!("ws://{router_address}/v1/responses").into_client_request() {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
    successful_request.headers_mut().insert(
        "X-Codex-Router-Token",
        HeaderValue::from_static("current-token"),
    );
    let (mut successful_client, _response) = match connect(successful_request) {
        Ok(connection) => connection,
        Err(error) => panic!("successful local websocket should connect: {error}"),
    };
    if let Err(error) = successful_client.send(Message::text(r#"{"type":"response.create"}"#)) {
        panic!("successful local websocket should send first frame: {error}");
    }
    match recorder_entered_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(()) => {}
        Err(error) => panic!("blocking recorder should be entered: {error}"),
    }

    let mut timeout_request =
        match format!("ws://{router_address}/v1/responses").into_client_request() {
            Ok(request) => request,
            Err(error) => panic!("timeout websocket request should build: {error}"),
        };
    timeout_request.headers_mut().insert(
        "X-Codex-Router-Token",
        HeaderValue::from_static("current-token"),
    );
    let (timeout_client, _response) = match connect(timeout_request) {
        Ok(connection) => connection,
        Err(error) => panic!("timeout local websocket should connect: {error}"),
    };

    match done_receiver.recv_timeout(Duration::from_millis(750)) {
        Ok(result) => panic!("serve returned before affinity task drained: {result:?}"),
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Err(error) => panic!("server result channel should remain open: {error}"),
    }
    if let Err(error) = recorder_release_sender.send(()) {
        panic!("blocking recorder release should send: {error}");
    }
    drop(successful_client);
    drop(timeout_client);
    match done_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(handled)) => assert_eq!(handled, 2),
        Ok(Err(error)) => {
            panic!("idle preconnect close should not return stored handler error: {error}")
        }
        Err(error) => panic!("serve should return after recorder release: {error}"),
    }
    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("upstream thread panicked: {error:?}"),
    }
    assert_eq!(recorder.records_snapshot().len(), 1);
}
