use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assembled_loopback_router_runtime_forwards_with_repository_state_and_secrets() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime");
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
        account_id("acct_runtime"),
        "runtime",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &account,
        70,
        "runtime-upstream-token",
    );
    let affinity_secret = must_ok(load_or_create_router_affinity_hash_secret(&secrets))
        .secret()
        .clone();

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
        if let Err(error) = upstream_sender.send(request) {
            panic!("mock upstream request should record: {error}");
        }
        let response_body = b"data: {\"id\":\"resp_runtime\"}\n\n";
        if let Err(error) = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
            response_body.len()
        ) {
            panic!("mock upstream should write response headers: {error}");
        }
        if let Err(error) = stream.write_all(response_body) {
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
        database_path.clone(),
        secret_path,
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_quota_clock(1_030, 60);
    let runtime = match LoopbackRouterRuntime::start_for_test(config).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let client_thread = thread::spawn(move || {
        send_loopback_request(
            router_address,
            "POST /v1/responses?runtime=true HTTP/1.1\r\n",
            br#"{"model":"gpt-5","runtime":true}"#,
        )
    });

    let handled = match runtime.serve_http_connections(1).await {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve one connection: {error}"),
    };
    assert_eq!(handled, 1);
    tokio::task::yield_now().await;
    assert!(tokio::runtime::Handle::try_current().is_ok());

    let response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("\r\ndata: {\"id\":\"resp_runtime\"}\n\n"));

    let upstream_request = match upstream_receiver.recv() {
        Ok(request) => request,
        Err(error) => panic!("mock upstream request should be recorded: {error}"),
    };
    assert!(upstream_request.starts_with("POST /v1/responses?runtime=true HTTP/1.1\r\n"));
    assert!(upstream_request.contains("authorization: Bearer runtime-upstream-token\r\n"));
    assert!(!upstream_request.contains("X-Codex-Router-Token"));
    assert!(!upstream_request.contains("current-token"));

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }

    let owner_hash = must_ok(hash_previous_response_id(
        &affinity_secret,
        &must_ok(PreviousResponseId::new("resp_runtime")),
    ));
    let runtime_state = must_ok(SqliteStateStore::open(&database_path));
    let owner_lookup = wait_for_previous_response_owner(&runtime_state, &owner_hash);
    let PreviousResponseAffinityOwnerLookup::Found(owner) = owner_lookup else {
        panic!("runtime should persist response owner row: {owner_lookup:?}");
    };
    assert_eq!(owner.account_id(), account.account_id());
    assert_eq!(owner.credential_generation(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_http_weekly_floor_at_threshold_routes_only_to_eligible_peer() {
    const NOW: u64 = 1_030;
    let temp_dir = ProxyTestTempDir::new("served_http_weekly_floor");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let state = SqliteStateStore::open(&database_path).expect("state should open");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .expect("secrets should open");
    let protected = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_served_http_protected"),
        "protected",
        AccountStatus::Enabled,
    );
    let peer = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_served_http_peer"),
        "peer",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &protected,
        8,
        "protected-http-token",
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &peer, 80, "peer-http-token");
    refresh_served_floor_windows_for_test(&state, &protected, NOW, 8);
    refresh_served_floor_windows_for_test(&state, &peer, NOW, 80);
    let selector_inputs =
        SelectorQuotaRepository::selector_inputs_for_route_band(&state, "responses", NOW)
            .expect("served floor quota rows should load");
    assert!(
        selector_inputs.iter().all(|input| input
            .windows()
            .iter()
            .all(|window| { window.status() == SelectorQuotaWindowStatus::Eligible })),
        "served floor fixture must have fresh eligible quota windows"
    );
    drop(state);
    set_weekly_floor_for_test(&database_path, protected.label(), 500).await;

    let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("upstream should bind");
    let upstream_address = upstream_listener.local_addr().expect("address should read");
    let (request_sender, request_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (mut stream, _) = upstream_listener
            .accept()
            .expect("peer request should arrive");
        let request = read_test_http_request(&mut stream);
        request_sender.send(request).expect("request should record");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .expect("response should write");
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
        send_loopback_request(
            router_address,
            "POST /v1/responses HTTP/1.1\r\n",
            br#"{"model":"gpt-5"}"#,
        )
    });
    assert_eq!(
        runtime
            .serve_http_connections(1)
            .await
            .expect("runtime should serve HTTP"),
        1
    );
    let response = client_thread.join().expect("client should join");
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    let request = request_receiver.recv().expect("peer request should record");
    assert!(request.contains("authorization: Bearer peer-http-token\r\n"));
    assert_eq!(request.matches("protected-http-token").count(), 0);
    upstream_thread.join().expect("upstream should join");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_http_weekly_floor_ignores_bad_forecast_above_current_threshold() {
    const NOW: u64 = 1_030;
    let temp_dir = ProxyTestTempDir::new("served_http_weekly_floor_bad_forecast");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let state = SqliteStateStore::open(&database_path).expect("state should open");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .expect("secrets should open");
    let protected = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_served_http_floor_forecast"),
        "protected-forecast",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &protected,
        48,
        "protected-forecast-http-token",
    );
    drop(state);
    seed_computable_weekly_margin_for_test(
        &database_path,
        protected.account_id(),
        NOW,
        5,
        100,
        48,
        5 * 86_400,
    )
    .await;
    set_weekly_floor_for_test(&database_path, protected.label(), 1_000).await;
    assert_bad_projected_margin_does_not_floor_block_for_test(
        &database_path,
        protected.account_id(),
        NOW,
        1_000,
    )
    .await;

    let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("upstream should bind");
    let upstream_address = upstream_listener.local_addr().expect("address should read");
    let (request_sender, request_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (mut stream, _) = upstream_listener
            .accept()
            .expect("above-floor request should arrive");
        let request = read_test_http_request(&mut stream);
        request_sender.send(request).expect("request should record");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .expect("response should write");
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
        send_loopback_request(
            router_address,
            "POST /v1/responses HTTP/1.1\r\n",
            br#"{"model":"gpt-5"}"#,
        )
    });
    assert_eq!(
        runtime
            .serve_http_connections(1)
            .await
            .expect("runtime should serve HTTP"),
        1
    );
    let response = client_thread.join().expect("client should join");
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "current quota above the floor should remain eligible, got:\n{response}"
    );
    let request = request_receiver
        .recv()
        .expect("above-floor request should record");
    assert!(request.contains("authorization: Bearer protected-forecast-http-token\r\n"));
    upstream_thread.join().expect("upstream should join");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_http_all_weekly_floor_blocked_is_scrubbed_and_sends_zero_upstream_requests() {
    let temp_dir = ProxyTestTempDir::new("served_http_all_weekly_floor_blocked");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let state = SqliteStateStore::open(&database_path).expect("state should open");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .expect("secrets should open");
    let first = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_all_floor_first"),
        "sensitive-first-label",
        AccountStatus::Enabled,
    );
    let second = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_all_floor_second"),
        "sensitive-second-label",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &first, 100, "first-secret-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &second, 4, "second-secret-token");
    persist_fresh_account_with_selector_window_specs(
        &state,
        &first,
        "responses",
        &[(18_000, 5, true), (604_800, 5, false)],
    );
    persist_fresh_account_with_selector_window_specs(
        &state,
        &second,
        "responses",
        &[(18_000, 4, true), (604_800, 4, false)],
    );
    drop(state);
    set_weekly_floor_for_test(&database_path, first.label(), 500).await;
    set_weekly_floor_for_test(&database_path, second.label(), 500).await;

    let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("upstream should bind");
    upstream_listener
        .set_nonblocking(true)
        .expect("upstream should become nonblocking");
    let upstream_address = upstream_listener.local_addr().expect("address should read");
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new("127.0.0.1", 0).expect("bind should validate"),
        UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
            .expect("endpoint should validate"),
        database_path,
        secret_path,
    )
    .with_quota_clock(1_030, 60);
    let runtime = LoopbackRouterRuntime::start_for_test(config)
        .await
        .expect("runtime should start");
    let router_address = runtime.local_addr();
    let client_thread = thread::spawn(move || {
        send_loopback_request(
            router_address,
            "POST /v1/responses HTTP/1.1\r\n",
            br#"{"model":"gpt-5"}"#,
        )
    });
    assert_eq!(
        runtime
            .serve_http_connections(1)
            .await
            .expect("runtime should serve HTTP"),
        1
    );
    let response = client_thread.join().expect("client should join");
    assert!(response.contains("codex_router_all_accounts_exhausted"));
    for canary in [
        "sensitive-first-label",
        "sensitive-second-label",
        "first-secret-token",
        "second-secret-token",
    ] {
        assert!(!response.contains(canary));
    }
    assert!(matches!(
        upstream_listener.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}
