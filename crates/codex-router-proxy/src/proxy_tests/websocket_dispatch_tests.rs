use super::*;

#[test]
#[allow(clippy::result_large_err)]
fn loopback_router_runtime_passes_large_malformed_websocket_first_frame_unchanged() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_large_first_frame");
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
        account_id("acct_ws_runtime_large"),
        "ws-large",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        90,
        "runtime-ws-large-token-canary",
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
        let mut websocket = match accept_hdr_with_config(
            stream,
            |request: &Request, response: Response| {
                let authorization = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<missing>")
                    .to_owned();
                if let Err(error) = upstream_sender.send((authorization, String::new())) {
                    panic!("mock websocket upstream headers should record: {error}");
                }
                Ok(response)
            },
            Some(unbounded_test_websocket_config()),
        ) {
            Ok(websocket) => websocket,
            Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
        };
        let first_frame = match websocket.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
        };
        if let Err(error) = upstream_sender.send((String::new(), first_frame)) {
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
    let client_first_frame = {
        let mut frame = String::from(r#"{"type":"response.create","input":""#);
        frame.extend(std::iter::repeat_n('x', (16 * 1024 * 1024) + 1));
        frame
    };
    let expected_first_frame = client_first_frame.clone();
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
        let (mut client, _response) =
            match connect_with_config(request, Some(unbounded_test_websocket_config()), 0) {
                Ok(connection) => connection,
                Err(error) => panic!("local websocket client should connect: {error}"),
            };
        if let Err(error) = client.send(Message::text(client_first_frame)) {
            panic!("local websocket client should send first frame: {error}");
        }
        let response = match client.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("local websocket client should read response: {error}"),
        };
        if let Err(error) = client.close(None) {
            panic!("local websocket client should close cleanly: {error}");
        }
        response
    });

    let handled = match runtime.serve_protocol_connections(1) {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve websocket connection: {error}"),
    };
    assert_eq!(handled, 1);
    let client_response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };
    assert_eq!(client_response, r#"{"type":"response.completed"}"#);
    let (authorization, _) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream handshake should record: {error}"),
    };
    assert_eq!(authorization, "Bearer runtime-ws-large-token-canary");
    let (_, recorded_first_frame) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream first frame should record: {error}"),
    };
    assert_eq!(recorded_first_frame, expected_first_frame);

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }
}

#[test]
#[allow(clippy::result_large_err)]
fn loopback_router_runtime_reloads_local_auth_and_closes_old_token_websocket() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_token_rotation");
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
        account_id("acct_ws_rotation"),
        "ws-rotation",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        90,
        "runtime-ws-rotation-upstream-token",
    );

    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock websocket upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock websocket upstream address should read: {error}"),
    };
    let (first_frame_sender, first_frame_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
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
        let first_frame = match websocket.read() {
            Ok(message) => message,
            Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
        };
        if let Err(error) = first_frame_sender.send(first_frame.to_string()) {
            panic!("mock websocket upstream first frame should record: {error}");
        }
        let _released = release_receiver.recv_timeout(Duration::from_secs(2));
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
        LocalRouterTokenRecord::new(SecretString::new("token-a"), TokenGeneration::new(1)),
    )
    .with_quota_clock(1_030, 60);
    let runtime = match LoopbackRouterRuntime::start_for_test(config) {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let reloader = runtime.local_auth_reloader();
    let server_thread = thread::spawn(move || match runtime.serve_protocol_connections(1) {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve websocket connection: {error}"),
    });

    let mut request = match format!("ws://{router_address}/v1/responses").into_client_request() {
        Ok(request) => request,
        Err(error) => panic!("local websocket request should build: {error}"),
    };
    request
        .headers_mut()
        .insert("X-Codex-Router-Token", HeaderValue::from_static("token-a"));
    let (mut client, _response) = match connect(request) {
        Ok(connection) => connection,
        Err(error) => panic!("local websocket client should connect: {error}"),
    };
    if let Err(error) = client.send(Message::text(r#"{"type":"response.create"}"#)) {
        panic!("local websocket client should send first frame: {error}");
    }
    let recorded_first_frame = match first_frame_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(frame) => frame,
        Err(error) => panic!("upstream should receive first frame before rotation: {error}"),
    };
    assert_eq!(recorded_first_frame, r#"{"type":"response.create"}"#);

    reloader.reload_local_auth(
        LocalRouterTokenRecord::new(SecretString::new("token-b"), TokenGeneration::new(2)),
        vec![LocalRouterTokenRecord::new(
            SecretString::new("token-a"),
            TokenGeneration::new(1),
        )],
    );
    match client.read() {
        Ok(Message::Close(_)) => {}
        Ok(message) => panic!("old-token websocket should close, got message: {message}"),
        Err(_error) => {}
    }
    if let Err(error) = release_sender.send(()) {
        panic!("upstream release should send: {error}");
    }

    match server_thread.join() {
        Ok(handled) => assert_eq!(handled, 1),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }
}
