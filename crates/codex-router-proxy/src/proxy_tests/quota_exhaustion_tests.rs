use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assembled_loopback_router_runtime_retries_http_quota_errors_until_account_can_serve() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime_http_quota_retry_chain");
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
    let primary = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_primary_quota_chain"),
        "primary-quota-chain",
        AccountStatus::Enabled,
    );
    let secondary = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_secondary_quota_chain"),
        "secondary-quota-chain",
        AccountStatus::Enabled,
    );
    let tertiary = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_tertiary_quota_chain"),
        "tertiary-quota-chain",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &primary, 90, "primary-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &secondary, 80, "secondary-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &tertiary, 70, "tertiary-token");

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
        for connection_index in 0..3 {
            let (mut stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock upstream should accept retry connection: {error}"),
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
            if connection_index < 2 {
                let quota_body = br#"{"type":"error","status":429,"error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#;
                if let Err(error) = write!(
                    stream,
                    "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    quota_body.len()
                ) {
                    panic!("mock upstream should write quota response headers: {error}");
                }
                if let Err(error) = stream.write_all(quota_body) {
                    panic!("mock upstream should write quota response body: {error}");
                }
            } else if let Err(error) =
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\ntertiary-response")
            {
                panic!("mock upstream should write tertiary response: {error}");
            }
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
            "POST /v1/responses?retry=true HTTP/1.1\r\n",
            br#"{"model":"gpt-5","retry":true}"#,
        )
    });

    let shutdown = tokio_util::sync::CancellationToken::new();
    let server_shutdown = shutdown.clone();
    let server_thread = tokio::spawn(async move {
        runtime
            .serve_protocol_connections_until_cancelled(usize::MAX, server_shutdown)
            .await
    });
    let response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };

    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "Codex-side response should be tertiary success, got:\n{response}"
    );
    assert!(response.ends_with("\r\ntertiary-response"));
    assert!(!response.contains("usage_limit_reached"));
    let authorizations: Vec<String> = (0..3)
        .map(|_| match upstream_receiver.recv() {
            Ok(authorization) => authorization,
            Err(error) => panic!("upstream auth should record: {error}"),
        })
        .collect();
    assert_eq!(
        authorizations,
        vec![
            "authorization: Bearer primary-token".to_owned(),
            "authorization: Bearer secondary-token".to_owned(),
            "authorization: Bearer tertiary-token".to_owned(),
        ]
    );

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }

    wait_for_durable_quota_exhaustion(&state, &[primary.account_id(), secondary.account_id()]);
    wait_for_repository_selected_account(
        &state,
        tertiary.account_id(),
        "durable quota state should select the fallback while serving",
    );
    shutdown.cancel();
    match server_thread.await {
        Ok(Ok(handled)) => assert_eq!(handled, 1),
        Ok(Err(error)) => panic!("router shutdown should succeed: {error}"),
        Err(error) => panic!("router server thread panicked: {error:?}"),
    }

    let runtime_state = must_ok(SqliteStateStore::open(&database_path));
    wait_for_repository_selected_account(
        &runtime_state,
        tertiary.account_id(),
        "tertiary should remain selectable after chained quota retries",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assembled_loopback_router_runtime_hides_http_quota_errors_when_all_accounts_exhausted() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime_http_all_quota_exhausted");
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
    let primary = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_primary_all_exhausted"),
        "primary-all-exhausted",
        AccountStatus::Enabled,
    );
    let secondary = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_secondary_all_exhausted"),
        "secondary-all-exhausted",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &primary, 90, "primary-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &secondary, 80, "secondary-token");

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
        for _connection_index in 0..2 {
            let (mut stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock upstream should accept retry connection: {error}"),
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
            let quota_body = br#"{"type":"error","status":429,"error":{"type":"usage_limit_reached","code":"usage_limit_reached","message":"acct_primary_all_exhausted is out"}}"#;
            if let Err(error) = write!(
                stream,
                "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                quota_body.len()
            ) {
                panic!("mock upstream should write quota response headers: {error}");
            }
            if let Err(error) = stream.write_all(quota_body) {
                panic!("mock upstream should write quota response body: {error}");
            }
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
    let runtime = match LoopbackRouterRuntime::start_for_test(config).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let client_thread = thread::spawn(move || {
        send_loopback_request(
            router_address,
            "POST /v1/responses?all-exhausted=true HTTP/1.1\r\n",
            br#"{"model":"gpt-5","all_exhausted":true}"#,
        )
    });

    let handled = match runtime.serve_http_connections(1).await {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve one client connection: {error}"),
    };
    assert_eq!(handled, 1);
    let response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };

    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "Codex-side response should be router-level exhausted response, got:\n{response}"
    );
    assert!(response.contains("codex_router_all_accounts_exhausted"));
    assert!(response.contains("All configured codex-router accounts are out of usable quota"));
    assert!(response.contains("usage_limit_reached"));
    assert!(!response.contains("acct_primary_all_exhausted"));
    assert_eq!(
        vec![
            upstream_receiver
                .recv()
                .unwrap_or_else(|error| panic!("first upstream auth should record: {error}")),
            upstream_receiver
                .recv()
                .unwrap_or_else(|error| panic!("second upstream auth should record: {error}")),
        ],
        vec![
            "authorization: Bearer primary-token".to_owned(),
            "authorization: Bearer secondary-token".to_owned(),
        ]
    );

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_router_runtime_balances_active_account_inside_hold_cooldown() {
    let temp_dir = ProxyTestTempDir::new("runtime_cross_connection_balance");
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
    persist_account_with_snapshot_and_token(&state, &secrets, &alpha, 50, "alpha-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &beta, 50, "beta-token");

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
        let (mut first_stream, _first_peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept first connection: {error}"),
        };
        let first_request = read_test_http_request(&mut first_stream);
        let first_authorization = first_request
            .lines()
            .find(|line| line.starts_with("authorization: "))
            .unwrap_or("<missing>")
            .to_owned();
        if let Err(error) = upstream_sender.send(first_authorization) {
            panic!("mock upstream first authorization should record: {error}");
        }

        let (mut second_stream, _second_peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept second connection: {error}"),
        };
        let second_request = read_test_http_request(&mut second_stream);
        let second_authorization = second_request
            .lines()
            .find(|line| line.starts_with("authorization: "))
            .unwrap_or("<missing>")
            .to_owned();
        if let Err(error) = upstream_sender.send(second_authorization) {
            panic!("mock upstream second authorization should record: {error}");
        }

        for stream in [&mut second_stream, &mut first_stream] {
            if let Err(error) = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            {
                panic!("mock upstream should write response: {error}");
            }
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
    let runtime = match LoopbackRouterRuntime::start_for_test(config).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let runtime_thread = tokio::spawn(async move { runtime.serve_protocol_connections(2).await });
    let first_client_thread = thread::spawn(move || {
        send_loopback_request(
            router_address,
            "POST /v1/responses HTTP/1.1\r\n",
            br#"{"model":"gpt-5","turn":1}"#,
        )
    });
    let first_authorization = match upstream_receiver.recv() {
        Ok(authorization) => authorization,
        Err(error) => panic!("first upstream auth should record: {error}"),
    };
    let second_client_thread = thread::spawn(move || {
        send_loopback_request(
            router_address,
            "POST /v1/responses HTTP/1.1\r\n",
            br#"{"model":"gpt-5","turn":2}"#,
        )
    });

    let handled = match runtime_thread.await {
        Ok(Ok(handled)) => handled,
        Ok(Err(error)) => panic!("router runtime should serve two connections: {error}"),
        Err(error) => panic!("router runtime thread panicked: {error:?}"),
    };
    assert_eq!(handled, 2);
    for client_thread in [first_client_thread, second_client_thread] {
        let response = match client_thread.join() {
            Ok(response) => response,
            Err(error) => panic!("client thread panicked: {error:?}"),
        };
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "client should receive 200 OK, got:\n{response}"
        );
    }

    let second_authorization = match upstream_receiver.recv() {
        Ok(authorization) => authorization,
        Err(error) => panic!("second upstream auth should record: {error}"),
    };
    assert_eq!(first_authorization, "authorization: Bearer alpha-token");
    assert_eq!(second_authorization, "authorization: Bearer beta-token");

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}
