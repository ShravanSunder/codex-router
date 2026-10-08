use super::*;

#[test]
#[allow(clippy::result_large_err)]
fn legacy_single_lane_accept_loop_reproducer_blocks_second_client_until_first_finishes() {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("legacy reproducer listener should bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("legacy reproducer listener address should read: {error}"),
    };
    let (first_accepted_sender, first_accepted_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let (mut first_stream, _peer) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("legacy reproducer should accept first client: {error}"),
        };
        if let Err(error) = first_stream.write_all(b"first accepted\n") {
            panic!("legacy reproducer should write first response: {error}");
        }
        if let Err(error) = first_accepted_sender.send(()) {
            panic!("legacy reproducer should signal first accept: {error}");
        }
        match release_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(()) => {}
            Err(error) => panic!("legacy reproducer release should arrive: {error}"),
        }
        let (mut second_stream, _peer) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("legacy reproducer should accept second client: {error}"),
        };
        if let Err(error) = second_stream.write_all(b"second accepted\n") {
            panic!("legacy reproducer should write second response: {error}");
        }
    });

    let mut first_client = match TcpStream::connect(address) {
        Ok(stream) => stream,
        Err(error) => panic!("first reproducer client should connect: {error}"),
    };
    let mut first_response = [0_u8; 15];
    if let Err(error) = first_client.read_exact(&mut first_response) {
        panic!("first reproducer client should read response: {error}");
    }
    assert_eq!(&first_response, b"first accepted\n");
    match first_accepted_receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(()) => {}
        Err(error) => panic!("legacy reproducer should signal first accept: {error}"),
    }

    let mut second_client = match TcpStream::connect(address) {
        Ok(stream) => stream,
        Err(error) => panic!("second reproducer client should connect: {error}"),
    };
    if let Err(error) = second_client.set_read_timeout(Some(Duration::from_millis(100))) {
        panic!("second reproducer client should set read timeout: {error}");
    }
    let mut blocked_probe = [0_u8; 1];
    match second_client.read(&mut blocked_probe) {
        Ok(bytes_read) => panic!(
            "legacy single-lane accept loop unexpectedly handled second client before first finished; bytes_read={bytes_read}"
        ),
        Err(error)
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::TimedOut => {}
        Err(error) => panic!("second reproducer read failed for unexpected reason: {error}"),
    }

    if let Err(error) = release_sender.send(()) {
        panic!("legacy reproducer release should send: {error}");
    }
    if let Err(error) = second_client.set_read_timeout(Some(Duration::from_secs(1))) {
        panic!("second reproducer client should reset read timeout: {error}");
    }
    let mut second_response = Vec::new();
    if let Err(error) = second_client.read_to_end(&mut second_response) {
        panic!("second reproducer client should read after release: {error}");
    }
    assert_eq!(second_response, b"second accepted\n");
    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("legacy reproducer server thread panicked: {error:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::result_large_err)]
async fn loopback_router_runtime_accepts_second_websocket_while_first_is_blocked() {
    let temp_dir = ProxyTestTempDir::new("runtime_websocket_concurrent_websockets");
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
        account_id("acct_ws_pair"),
        "ws-pair",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        90,
        "paired-websocket-upstream-token",
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
        let (first_stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept first websocket: {error}"),
        };
        let mut first_websocket =
            match accept_hdr(first_stream, |_request: &Request, response: Response| {
                Ok(response)
            }) {
                Ok(websocket) => websocket,
                Err(error) => panic!("mock upstream first handshake should accept: {error}"),
            };
        let first_frame = match first_websocket.read() {
            Ok(message) => message,
            Err(error) => panic!("mock upstream should read first websocket frame: {error}"),
        };
        if let Err(error) = upstream_sender.send(format!("ws1-frame:{first_frame}")) {
            panic!("mock upstream first frame should record: {error}");
        }

        if let Err(error) = upstream_listener.set_nonblocking(true) {
            let _ = first_websocket.send(Message::text(r#"{"type":"response.completed","id":1}"#));
            panic!("mock upstream listener should become nonblocking: {error}");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let (second_stream, _peer_address) = loop {
            match upstream_listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        let _ = first_websocket
                            .send(Message::text(r#"{"type":"response.completed","id":1}"#));
                        panic!("router should accept second websocket while first is blocked");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    let _ = first_websocket
                        .send(Message::text(r#"{"type":"response.completed","id":1}"#));
                    panic!("mock upstream should accept second websocket: {error}");
                }
            }
        };
        if let Err(error) = second_stream.set_nonblocking(false) {
            let _ = first_websocket.send(Message::text(r#"{"type":"response.completed","id":1}"#));
            panic!("mock upstream second websocket should become blocking: {error}");
        }
        let mut second_websocket =
            match accept_hdr(second_stream, |_request: &Request, response: Response| {
                Ok(response)
            }) {
                Ok(websocket) => websocket,
                Err(error) => panic!("mock upstream second handshake should accept: {error}"),
            };
        let second_frame = match second_websocket.read() {
            Ok(message) => message,
            Err(error) => panic!("mock upstream should read second websocket frame: {error}"),
        };
        if let Err(error) = upstream_sender.send(format!("ws2-frame:{second_frame}")) {
            panic!("mock upstream second frame should record: {error}");
        }
        if let Err(error) =
            second_websocket.send(Message::text(r#"{"type":"response.completed","id":2}"#))
        {
            panic!("mock upstream should respond to second websocket: {error}");
        }
        if let Err(error) =
            first_websocket.send(Message::text(r#"{"type":"response.completed","id":1}"#))
        {
            panic!("mock upstream should release first websocket: {error}");
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
            Err(error) => panic!("router runtime should serve concurrent websockets: {error}"),
        }
    });
    let first_client_thread = thread::spawn(move || {
        let mut websocket =
            connect_local_websocket_with_timeout(router_address, Duration::from_secs(2));
        if let Err(error) = websocket.send(Message::text(r#"{"type":"response.create","id":1}"#)) {
            panic!("first websocket client should send frame: {error}");
        }
        match websocket.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("first websocket client should read response: {error}"),
        }
    });

    let recorded_first_frame = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream should receive first websocket frame: {error}"),
    };
    assert_eq!(
        recorded_first_frame,
        r#"ws1-frame:{"type":"response.create","id":1}"#
    );

    let mut second_websocket =
        connect_local_websocket_with_timeout(router_address, Duration::from_secs(2));
    if let Err(error) = second_websocket.send(Message::text(r#"{"type":"response.create","id":2}"#))
    {
        panic!("second websocket client should send frame: {error}");
    }
    let second_response = match second_websocket.read() {
        Ok(message) => message.to_string(),
        Err(error) => panic!("second websocket client should read response: {error}"),
    };
    assert_eq!(second_response, r#"{"type":"response.completed","id":2}"#);

    let recorded_second_frame = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream should receive second websocket frame: {error}"),
    };
    assert_eq!(
        recorded_second_frame,
        r#"ws2-frame:{"type":"response.create","id":2}"#
    );
    let first_response = match first_client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("first websocket client thread panicked: {error:?}"),
    };
    assert_eq!(first_response, r#"{"type":"response.completed","id":1}"#);
    match server_thread.await {
        Ok(handled) => assert_eq!(handled, 2),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}
