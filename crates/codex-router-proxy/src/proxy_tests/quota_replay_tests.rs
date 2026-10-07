use super::*;

#[test]
fn assembled_loopback_router_runtime_retries_http_quota_error_on_fallback_account() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime_http_quota_retry");
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
        account_id("acct_primary_quota"),
        "primary-quota",
        AccountStatus::Enabled,
    );
    let fallback = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_fallback_quota"),
        "fallback-quota",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &primary, 90, "primary-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &fallback, 80, "fallback-token");

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
        for connection_index in 0..2 {
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
            if connection_index == 0 {
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
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\nfallback-response")
            {
                panic!("mock upstream should write fallback response: {error}");
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
    let runtime = match LoopbackRouterRuntime::start_for_test(config) {
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

    let handled = match runtime.serve_http_connections(1) {
        Ok(handled) => handled,
        Err(error) => panic!("router runtime should serve one client connection: {error}"),
    };
    assert_eq!(handled, 1);
    let response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };

    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "Codex-side response should be fallback success, got:\n{response}"
    );
    assert!(response.ends_with("\r\nfallback-response"));
    assert!(!response.contains("usage_limit_reached"));
    let first_authorization = match upstream_receiver.recv() {
        Ok(authorization) => authorization,
        Err(error) => panic!("first upstream auth should record: {error}"),
    };
    let second_authorization = match upstream_receiver.recv() {
        Ok(authorization) => authorization,
        Err(error) => panic!("second upstream auth should record: {error}"),
    };
    assert_eq!(first_authorization, "authorization: Bearer primary-token");
    assert_eq!(second_authorization, "authorization: Bearer fallback-token");

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }

    // The retry excludes the primary in process before its queued SQLite
    // exhaustion write completes. Establish the durable row before asking
    // a new repository selector to make the post-retry decision.
    wait_for_durable_quota_exhaustion(&state, &[primary.account_id()]);
    let runtime_state = must_ok(SqliteStateStore::open(&database_path));
    wait_for_repository_selected_account(
        &runtime_state,
        fallback.account_id(),
        "fallback should remain selectable after quota retry",
    );
}

#[test]
fn assembled_loopback_router_runtime_retries_large_http_json_quota_error_on_fallback_account() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime_large_http_quota_retry");
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
        account_id("acct_primary_large_quota"),
        "primary-large-quota",
        AccountStatus::Enabled,
    );
    let fallback = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_fallback_large_quota"),
        "fallback-large-quota",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &primary, 90, "primary-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &fallback, 80, "fallback-token");

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
        for connection_index in 0..2 {
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
            let body_contains_large_padding =
                request.contains("\"large_padding\":\"") && request.len() > 16 * 1024;
            if let Err(error) = upstream_sender.send((authorization, body_contains_large_padding)) {
                panic!("mock upstream authorization should record: {error}");
            }
            if connection_index == 0 {
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
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\nfallback-response")
            {
                panic!("mock upstream should write fallback response: {error}");
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
    let runtime = match LoopbackRouterRuntime::start_for_test(config) {
        Ok(runtime) => runtime,
        Err(error) => panic!("router runtime should start: {error}"),
    };
    let router_address = runtime.local_addr();
    let lock_runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    let lock_state = must_ok(lock_runtime.block_on(AsyncSqliteStateStore::open(&database_path)));
    let mut write_lock = must_ok(lock_runtime.block_on(lock_state.acquire_connection_for_test()));
    must_ok(lock_runtime.block_on(sqlx::query("BEGIN IMMEDIATE").execute(&mut *write_lock)));
    let large_body = format!(
        r#"{{"model":"gpt-5","large_padding":"{}"}}"#,
        "x".repeat(17 * 1024)
    );
    let client_thread = thread::spawn(move || {
        send_loopback_request(
            router_address,
            "POST /v1/responses?retry=true HTTP/1.1\r\n",
            large_body.as_bytes(),
        )
    });

    let shutdown = tokio_util::sync::CancellationToken::new();
    let server_shutdown = shutdown.clone();
    let server_thread = thread::spawn(move || {
        runtime.serve_protocol_connections_until_cancelled(usize::MAX, server_shutdown)
    });
    let response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };

    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "Codex-side response should be fallback success, got:\n{response}"
    );
    assert!(response.ends_with("\r\nfallback-response"));
    assert!(!response.contains("usage_limit_reached"));
    let first = upstream_receiver
        .recv()
        .unwrap_or_else(|error| panic!("first upstream auth should record: {error}"));
    let second = upstream_receiver
        .recv()
        .unwrap_or_else(|error| panic!("second upstream auth should record: {error}"));
    assert_eq!(first.0, "authorization: Bearer primary-token");
    assert_eq!(second.0, "authorization: Bearer fallback-token");
    assert!(first.1);
    assert!(
        second.1,
        "fallback attempt must replay the exact large JSON body, not only the metadata prefix"
    );

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }

    let before_commit =
        must_ok(state.load_quota_snapshot_for_route_band(primary.account_id(), "responses"))
            .expect("initial snapshot should remain readable");
    assert_eq!(before_commit.source(), QuotaSnapshotSource::MockEndpoint);
    assert_eq!(before_commit.remaining_headroom(), 90);
    lock_runtime.block_on(async move {
        must_ok(sqlx::query("COMMIT").execute(&mut *write_lock).await);
        drop(write_lock);
    });
    wait_for_durable_quota_exhaustion(&state, &[primary.account_id()]);
    wait_for_repository_selected_account(
        &state,
        fallback.account_id(),
        "durable quota state should select the fallback while serving",
    );
    shutdown.cancel();
    match server_thread.join() {
        Ok(Ok(handled)) => assert_eq!(handled, 1),
        Ok(Err(error)) => panic!("router shutdown should succeed: {error}"),
        Err(error) => panic!("router server thread panicked: {error:?}"),
    }

    let runtime_state = must_ok(SqliteStateStore::open(&database_path));
    wait_for_repository_selected_account(
        &runtime_state,
        fallback.account_id(),
        "fallback should remain selectable after large quota retry",
    );
}

#[test]
fn selection_diagnostics_unreplayable_http_quota_error_uses_real_handler() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime_unreplayable_http_quota");
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
        account_id("acct_primary_unreplayable_quota"),
        "primary-unreplayable-quota",
        AccountStatus::Enabled,
    );
    let fallback = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_fallback_unreplayable_quota"),
        "fallback-unreplayable-quota",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &primary, 90, "primary-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &fallback, 80, "fallback-token");

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
            Err(error) => panic!("mock upstream should accept first connection: {error}"),
        };
        let request = read_test_http_request(&mut stream);
        let authorization = request
            .lines()
            .find(|line| line.starts_with("authorization: "))
            .unwrap_or("<missing>")
            .to_owned();
        let body_exceeded_replay_limit =
            request.contains("\"unreplayable_padding\":\"") && request.len() > 2 * 1024 * 1024;
        if let Err(error) = upstream_sender.send((authorization, body_exceeded_replay_limit)) {
            panic!("mock upstream authorization should record: {error}");
        }
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
    let mut response = None;
    let captured_logs = crate::test_log_capture::capture_log_output(|| {
        let runtime = match LoopbackRouterRuntime::start_for_test(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let unreplayable_body = format!(
            r#"{{"model":"gpt-5","unreplayable_padding":"{}"}}"#,
            "x".repeat(2 * 1024 * 1024)
        );
        let client_thread = thread::spawn(move || {
            send_loopback_request(
                router_address,
                "POST /v1/responses?retry=true HTTP/1.1\r\n",
                unreplayable_body.as_bytes(),
            )
        });

        let shutdown = tokio_util::sync::CancellationToken::new();
        let server_shutdown = shutdown.clone();
        let server_thread = thread::spawn(move || {
            runtime.serve_protocol_connections_until_cancelled(usize::MAX, server_shutdown)
        });
        let observed_response = match client_thread.join() {
            Ok(response) => response,
            Err(error) => panic!("client thread panicked: {error:?}"),
        };
        let first = upstream_receiver
            .recv()
            .unwrap_or_else(|error| panic!("first upstream auth should record: {error}"));
        assert_eq!(first.0, "authorization: Bearer primary-token");
        assert!(first.1);
        assert!(
            upstream_receiver
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );
        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
        wait_for_durable_quota_exhaustion(&state, &[primary.account_id()]);
        wait_for_repository_selected_account(
            &state,
            fallback.account_id(),
            "durable quota state should select the fallback while serving",
        );
        shutdown.cancel();
        match server_thread.join() {
            Ok(Ok(handled)) => assert_eq!(handled, 1),
            Ok(Err(error)) => panic!("router shutdown should succeed: {error}"),
            Err(error) => panic!("router server thread panicked: {error:?}"),
        }
        response = Some(observed_response);
    });
    let response = response.unwrap_or_else(|| panic!("the real handler should return a response"));

    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    let response_body = response
        .split_once("\r\n\r\n")
        .map(|(_headers, body)| body)
        .unwrap_or_else(|| panic!("the HTTP response should contain its JSON body"));
    assert_eq!(
        response_body,
        r#"{"type":"error","status":503,"error":{"type":"codex_router_quota_state_unavailable","code":"codex_router_quota_state_unavailable","message":"codex-router cannot safely rotate accounts because quota state is unavailable."}}"#
    );
    assert!(captured_logs.contains("codex_router.selection_rejected"));
    assert!(captured_logs.contains("selection.stage=\"http_replay_unavailable\""));
    assert!(captured_logs.contains("error.class=\"quota_replay_unavailable\""));
    assert!(!captured_logs.contains("acct_primary_unreplayable_quota"));
    assert!(!captured_logs.contains("primary-unreplayable-quota"));
    assert!(!captured_logs.contains("unreplayable_padding"));
    assert!(!captured_logs.contains("usage_limit_reached"));
    assert!(!captured_logs.contains("primary-token"));
    assert!(!captured_logs.contains("fallback-token"));

    let runtime_state = must_ok(SqliteStateStore::open(&database_path));
    wait_for_repository_selected_account(
        &runtime_state,
        fallback.account_id(),
        "fallback should remain selectable after unreplayable quota error",
    );
}

#[test]
fn selection_diagnostics_http_attempt_limit_failure_reaches_real_handler_and_503() {
    use sqlx::Connection;

    let temp_dir = ProxyTestTempDir::new("selection_diagnostics_attempt_limit_failure");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock upstream listener should bind: {error}"),
    };
    let upstream_address = upstream_listener
        .local_addr()
        .unwrap_or_else(|error| panic!("mock upstream address should read: {error}"));
    upstream_listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("mock upstream should be nonblocking: {error}"));
    let endpoint = UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
        .unwrap_or_else(|error| panic!("mock upstream endpoint should validate: {error}"));
    let bind_address = LoopbackBindAddress::new("127.0.0.1", 0)
        .unwrap_or_else(|error| panic!("router bind address should validate: {error}"));
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        bind_address,
        endpoint,
        database_path.clone(),
        secret_path,
    );
    let mut response = None;
    let captured_logs = crate::test_log_capture::capture_log_output(|| {
        let runtime = LoopbackRouterRuntime::start_for_test(config)
            .unwrap_or_else(|error| panic!("test router runtime should start: {error}"));
        let sqlite_options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&database_path)
            .create_if_missing(true);
        let fixture_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("fixture SQLite runtime should build: {error}"));
        fixture_runtime.block_on(async {
            let mut connection = sqlx::SqliteConnection::connect_with(&sqlite_options)
                .await
                .unwrap_or_else(|error| panic!("fixture SQLite connection should open: {error}"));
            sqlx::query("DROP TABLE accounts")
                .execute(&mut connection)
                .await
                .unwrap_or_else(|error| panic!("fixture accounts table should drop: {error}"));
            connection
                .close()
                .await
                .unwrap_or_else(|error| panic!("fixture SQLite connection should close: {error}"));
        });

        let router_address = runtime.local_addr();
        let client_thread = thread::spawn(move || {
            send_loopback_request_with_token(
                router_address,
                None,
                br#"{"model":"gpt-5","body_private_marker":"offline-test"}"#,
            )
        });
        let handled = runtime.serve_http_connections(1).unwrap_or_else(|error| {
            panic!("router should serve the read-failure request: {error}")
        });
        assert_eq!(handled, 1);
        response = Some(
            client_thread
                .join()
                .unwrap_or_else(|error| panic!("HTTP client thread should finish: {error:?}")),
        );
        match upstream_listener.accept() {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok((_stream, _peer)) => panic!("attempt-limit failure must stop before upstream"),
            Err(error) => panic!("upstream listener should remain unused: {error}"),
        }
    });
    let response =
        response.unwrap_or_else(|| panic!("real handler should return its 503 response"));

    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    let response_body = response
        .split_once("\r\n\r\n")
        .map(|(_headers, body)| body)
        .unwrap_or_else(|| panic!("the HTTP response should contain its JSON body"));
    assert_eq!(
        response_body,
        r#"{"type":"error","status":503,"error":{"type":"codex_router_quota_state_unavailable","code":"codex_router_quota_state_unavailable","message":"codex-router cannot safely rotate accounts because quota state is unavailable."}}"#
    );
    assert!(captured_logs.contains("codex_router.selection_rejected"));
    assert!(captured_logs.contains("selection.stage=\"http_account_attempt_limit\""));
    assert!(captured_logs.contains("error.class=\"sqlite\""));
    assert!(!captured_logs.contains("body_private_marker"));
    assert!(!captured_logs.contains("no such table"));
    assert!(!captured_logs.contains("current-token"));
}
