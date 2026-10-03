use super::*;

#[test]
fn assembled_loopback_router_runtime_streams_sse_before_upstream_eof() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime_streams_sse");
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
        account_id("acct_runtime_streaming"),
        "runtime-streaming",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        70,
        "runtime-streaming-token",
    );

    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock upstream should bind: {error}"),
    };
    let upstream_address = match upstream_listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock upstream address should read: {error}"),
    };
    let (release_sender, release_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (mut stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept: {error}"),
        };
        let _request = read_test_http_request(&mut stream);
        if let Err(error) = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: first\n\n")
        {
            panic!("mock upstream should write first event: {error}");
        }
        if let Err(error) = stream.flush() {
            panic!("mock upstream should flush first event: {error}");
        }
        let _ = release_receiver.recv_timeout(Duration::from_secs(2));
        if let Err(error) = stream.write_all(b"data: second\n\n") {
            panic!("mock upstream should write second event: {error}");
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
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_quota_clock(1_030, 60);
    let runtime = match LoopbackRouterRuntime::start_for_test(config) {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let server_thread = thread::spawn(move || match runtime.serve_http_connections(1) {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve one streaming connection: {error}"),
    });

    let mut client = match TcpStream::connect(router_address) {
        Ok(client) => client,
        Err(error) => panic!("client should connect to loopback listener: {error}"),
    };
    if let Err(error) = client.set_read_timeout(Some(Duration::from_millis(750))) {
        panic!("client read timeout should be set: {error}");
    }
    let request = concat!(
        "POST /v1/responses?stream=true HTTP/1.1\r\n",
        "Host: 127.0.0.1\r\n",
        "X-Codex-Router-Token: current-token\r\n",
        "Accept: text/event-stream\r\n",
        "Content-Length: 17\r\n",
        "\r\n",
        "{\"model\":\"gpt-5\"}"
    );
    if let Err(error) = client.write_all(request.as_bytes()) {
        panic!("client request write should succeed: {error}");
    }
    let response_prefix =
        read_until_contains(&mut client, "data: first\n\n", Duration::from_millis(750));
    let _ = release_sender.send(());
    let mut drain = Vec::new();
    let _ = client.read_to_end(&mut drain);
    drop(client);

    match server_thread.join() {
        Ok(handled) => assert_eq!(handled, 1),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
    let response = match response_prefix {
        Ok(response) => response,
        Err(error) => {
            panic!("client should receive first SSE event before upstream EOF: {error}");
        }
    };
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("content-type: text/event-stream\r\n"));
    assert!(response.contains("data: first\n\n"));
}

#[test]
fn assembled_loopback_router_runtime_routes_split_frame_previous_response_affinity() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime_split_affinity");
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
    let alpha = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_alpha"),
        "alpha",
        AccountStatus::Enabled,
    );
    let beta = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_beta"),
        "beta",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &alpha, 90, "alpha-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &beta, 90, "beta-token");
    let affinity_secret = must_ok(load_or_create_router_affinity_hash_secret(&secrets))
        .secret()
        .clone();
    if let Err(error) = persist_previous_response_owner(
        &state,
        "resp_beta_split",
        &affinity_secret,
        beta.account_id(),
    ) {
        panic!("affinity owner should persist: {error}");
    }

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
        let (mut stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept: {error}"),
        };
        let request = read_test_http_request(&mut stream);
        let authorization = request
            .lines()
            .find(|line| line.starts_with("authorization: "))
            .unwrap_or("<missing>")
            .to_owned();
        if let Err(error) = upstream_sender.send(authorization) {
            panic!("mock upstream authorization should record: {error}");
        }
        if let Err(error) = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok") {
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
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_quota_clock(1_030, 60);
    let runtime = match LoopbackRouterRuntime::start_for_test(config) {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let server_thread = thread::spawn(move || match runtime.serve_http_connections(1) {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve split request: {error}"),
    });

    let body = br#"{"model":"gpt-5","previous_response_id":"resp_beta_split"}"#;
    let split_at = br#"{"model":"gpt-5","previous_"#.len();
    let mut client = match TcpStream::connect(router_address) {
        Ok(client) => client,
        Err(error) => panic!("client should connect to loopback listener: {error}"),
    };
    let headers = format!(
        "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nX-Codex-Router-Token: current-token\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    if let Err(error) = client.write_all(headers.as_bytes()) {
        panic!("client headers should write: {error}");
    }
    if let Err(error) = client.write_all(&body[..split_at]) {
        panic!("client first body frame should write: {error}");
    }
    if let Err(error) = client.flush() {
        panic!("client first body frame should flush: {error}");
    }
    thread::sleep(Duration::from_millis(100));
    if let Err(error) = client.write_all(&body[split_at..]) {
        panic!("client second body frame should write: {error}");
    }
    if let Err(error) = client.shutdown(Shutdown::Write) {
        panic!("client write shutdown should succeed: {error}");
    }
    let mut response = String::new();
    if let Err(error) = client.read_to_string(&mut response) {
        panic!("client response should read: {error}");
    }

    match server_thread.join() {
        Ok(handled) => assert_eq!(handled, 1),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    let authorization = match upstream_receiver.recv() {
        Ok(authorization) => authorization,
        Err(error) => panic!("upstream authorization should record: {error}"),
    };
    assert_eq!(authorization, "authorization: Bearer beta-token");
}
