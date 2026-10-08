use super::*;

async fn wait_for_upkeep_generation_async(
    state_path: &Path,
    account_id: &AccountId,
    expected_generation: u64,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let state = loop {
        match codex_router_state::sqlite::AsyncSqliteStateStore::open_read_only(state_path).await {
            Ok(state) => break state,
            Err(error)
                if format!("{error}").contains("locked")
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::task::yield_now().await;
            }
            Err(error) => panic!("read-only upkeep observation failed: {error}"),
        }
    };

    loop {
        match state.load_account(account_id).await {
            Ok(Some(account))
                if account.active_credential_generation() == Some(expected_generation) =>
            {
                state.close().await.unwrap_or_else(|error| {
                    panic!("upkeep observation state should close: {error}")
                });
                return;
            }
            Ok(_) => {}
            Err(error) if format!("{error}").contains("locked") => {}
            Err(error) => panic!("upkeep account observation failed: {error}"),
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "upkeep account should reach credential generation {expected_generation}"
        );
        tokio::task::yield_now().await;
    }
}

#[test]
fn synchronous_serve_dispatch_returns_async_dispatch_error_before_opening_storage() {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = run_with_io(
        [OsString::from("serve")],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    )
    .expect_err("Serve must use the caller's async runtime");

    assert!(matches!(error, CliError::ServeRequiresAsyncDispatch));
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn serve_provisions_local_token_and_keeps_codex_optional() {
    let test_root = TestRoot::new("serve-command");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("acct_cli_serve");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "cli-serve",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 100);
    must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
    persist_effective_selector_window(&state, &account_id, "responses", 100);
    let upstream_token_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let upstream_credential_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "cli-upstream-token",
            Some("cli-upstream-refresh-token".to_owned()),
        )
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

    let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let upstream_address = must_ok(upstream_listener.local_addr());
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (mut stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept: {error}"),
        };
        let request = read_http_request_with_body(&mut stream);
        if let Err(error) = upstream_sender.send(request) {
            panic!("mock upstream request should record: {error}");
        }
        if let Err(error) =
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
        {
            panic!("mock upstream should write response: {error}");
        }
    });
    let router_port = reserve_loopback_port();
    let router_port_text = router_port.to_string();
    let upstream_base_url = format!("http://{upstream_address}/v1");
    let client_thread = thread::spawn(move || {
        send_tokenless_loopback_request_with_retry(
            router_port,
            br#"{"model":"gpt-5","serve":true}"#,
        )
    });

    let output = run_cli(
        [
            "codex-router",
            "serve",
            "--listen-host",
            "127.0.0.1",
            "--port",
            router_port_text.as_str(),
            "--state-db",
            path_to_str(&state_path),
            "--secret-root",
            path_to_str(&secret_root),
            "--upstream-base-url",
            upstream_base_url.as_str(),
            "--now-unix-seconds",
            "1030",
            "--max-snapshot-age-seconds",
            "60",
            "--disable-background-quota-refresh",
            "--max-connections",
            "1",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(
        output
            .stdout
            .contains(format!("listening: 127.0.0.1:{router_port}\n").as_str())
    );
    assert!(output.stderr.is_empty());
    let token_store = must_ok(FileSecretStore::open(&secret_root));
    let local_token = must_ok(LocalRouterTokenService::new(token_store).load_current());
    assert_eq!(local_token.generation().as_u64(), 1);
    let client_response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };
    assert!(client_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(client_response.ends_with("\r\ndata: ok\n\n"));

    let upstream_request = match upstream_receiver.recv() {
        Ok(request) => request,
        Err(error) => panic!("mock upstream request should be recorded: {error}"),
    };
    assert!(upstream_request.starts_with("POST /v1/responses HTTP/1.1\r\n"));
    assert!(upstream_request.contains("authorization: Bearer cli-upstream-token\r\n"));
    assert!(!upstream_request.contains("X-Codex-Router-Token"));
    assert!(!upstream_request.contains("current-token"));

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}

#[test]
fn serve_with_background_quota_refresh_enabled_uses_async_dispatch() {
    let test_root = TestRoot::new("serve-background-quota-enabled");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let router_port = reserve_loopback_port();
    let router_port_text = router_port.to_string();
    let health_client_thread =
        thread::spawn(move || {
            let mut client = connect_with_retry(router_port);
            must_ok(client.write_all(
                b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            ));
            must_ok(client.shutdown(Shutdown::Write));
            let mut response = String::new();
            must_ok(client.read_to_string(&mut response));
            response
        });

    let output = run_cli(
        [
            "codex-router",
            "serve",
            "--listen-host",
            "127.0.0.1",
            "--port",
            router_port_text.as_str(),
            "--state-db",
            path_to_str(&state_path),
            "--secret-root",
            path_to_str(&secret_root),
            "--upstream-base-url",
            "http://127.0.0.1:1/v1",
            "--max-connections",
            "1",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(output.stderr.is_empty());
    assert!(
        output
            .stdout
            .contains(format!("listening: 127.0.0.1:{router_port}\n").as_str())
    );
    let health_response = health_client_thread
        .join()
        .unwrap_or_else(|error| panic!("health client should join: {error:?}"));
    assert!(health_response.starts_with("HTTP/1.1 200 OK\r\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_starts_and_codex_route_fails_closed_for_unavailable_pooled_stores() {
    #[derive(Clone, Copy)]
    enum UnavailableStoreKind {
        KeyUnavailable,
        MigrationIncomplete,
    }

    for (suffix, store_kind) in [
        ("key-unavailable", UnavailableStoreKind::KeyUnavailable),
        (
            "migration-incomplete",
            UnavailableStoreKind::MigrationIncomplete,
        ),
    ] {
        let test_root = TestRoot::new(&format!("serve-{suffix}"));
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let account_id = account_id(&format!("acct_serve_{suffix}"));
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                suffix,
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        ));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 100);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        persist_effective_selector_window(&state, &account_id, "responses", 100);
        let file_store = must_ok(FileSecretStore::open(&secret_root));
        let credential_store = match store_kind {
            UnavailableStoreKind::KeyUnavailable => EncryptedCredentialStore::key_unavailable(file_store),
            UnavailableStoreKind::MigrationIncomplete => EncryptedCredentialStore::migration_incomplete(
                file_store,
                vec![account_id.as_str().to_owned()],
                codex_router_secret_store::credential_migration::CredentialMigrationFailure::MigrationNotComplete,
            ),
        };

        let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        must_ok(upstream_listener.set_nonblocking(true));
        let upstream_address = must_ok(upstream_listener.local_addr());
        let router_port = reserve_loopback_port();
        let command = must_ok(CliCommand::parse([
            OsString::from("serve"),
            OsString::from("--listen-host"),
            OsString::from("127.0.0.1"),
            OsString::from("--port"),
            OsString::from(router_port.to_string()),
            OsString::from("--state-db"),
            state_path.as_os_str().to_os_string(),
            OsString::from("--secret-root"),
            secret_root.as_os_str().to_os_string(),
            OsString::from("--upstream-base-url"),
            OsString::from(format!("http://{upstream_address}/v1")),
            OsString::from("--now-unix-seconds"),
            OsString::from("1030"),
            OsString::from("--max-snapshot-age-seconds"),
            OsString::from("60"),
            OsString::from("--disable-background-quota-refresh"),
            OsString::from("--max-connections"),
            OsString::from("1"),
        ]));
        let CliCommand::Serve(command) = command else {
            panic!("serve command should parse");
        };
        let client_thread = thread::spawn(move || {
            send_tokenless_loopback_request_with_retry(
                router_port,
                br#"{"model":"gpt-5","serve":true}"#,
            )
        });
        let serve_task = tokio::spawn(async move {
            let mut stdout = Vec::new();
            let result = run_serve_command_with_upkeep_start(
                &mut stdout,
                command,
                credential_store,
                credential_upkeep_worker::start_background_credential_upkeep_worker,
            )
            .await;
            (result, stdout)
        });

        let response = must_ok(client_thread.join().map_err(|_| "client thread failed"));
        let (serve_result, stdout) = serve_task
            .await
            .unwrap_or_else(|error| panic!("serve task should join: {error}"));
        must_ok(serve_result);

        assert!(String::from_utf8_lossy(&stdout).contains("listening: 127.0.0.1:"));
        assert!(
            response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
            "{suffix}: {response}"
        );
        assert!(matches!(
            upstream_listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }
}

#[test]
fn serve_command_defaults_to_live_runtime_clock_with_quota_freshness_margin() {
    let command = match CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--state-db"),
        OsString::from("/tmp/codex-router-state.sqlite"),
        OsString::from("--secret-root"),
        OsString::from("/tmp/codex-router-secrets"),
        OsString::from("--upstream-base-url"),
        OsString::from("http://127.0.0.1:1/v1"),
    ]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(other) => panic!("serve command should parse, got {other:?}"),
        Err(error) => panic!("serve command should parse: {error}"),
    };

    assert_eq!(command.now_unix_seconds, None);
    assert_eq!(command.quota_refresh_interval_seconds, 180);
    assert!(
        command.quota_refresh_interval_seconds < command.max_snapshot_age_seconds,
        "background refresh must run before live selector evidence expires"
    );
    assert!(
        command.quota_refresh_interval_seconds
            < crate::quota::DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS,
        "background refresh must run before persisted selector evidence becomes stale"
    );
}

#[test]
fn serve_command_accepts_explicit_fixed_quota_clock() {
    let command = match CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--now-unix-seconds"),
        OsString::from("12345"),
    ]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(other) => panic!("serve command should parse, got {other:?}"),
        Err(error) => panic!("serve command should parse: {error}"),
    };

    assert_eq!(command.now_unix_seconds, Some(12_345));
}

#[test]
fn serve_command_defaults_to_home_router_paths_and_provider_upstream() {
    let command = match CliCommand::parse([OsString::from("serve")]) {
        Ok(CliCommand::Serve(command)) => command,
        Ok(other) => panic!("serve command should parse, got {other:?}"),
        Err(error) => panic!("serve command should parse: {error}"),
    };

    let router_root = default_router_root_for_test();
    assert_eq!(command.state_db, router_root.join("state.sqlite"));
    assert_eq!(command.secret_root, router_root.join("secrets"));
    assert_eq!(
        command.upstream_base_url,
        codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL
    );
}

#[test]
#[allow(clippy::result_large_err)]
fn serve_command_dispatches_websocket_upgrade_through_runtime() {
    let test_root = TestRoot::new("serve-command-websocket");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("acct_cli_ws");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "cli-ws",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 100);
    must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
    persist_effective_selector_window(&state, &account_id, "responses", 100);
    let upstream_token_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let upstream_credential_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "cli-ws-upstream-token",
            Some("cli-ws-upstream-refresh-token".to_owned()),
        )
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

    let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let upstream_address = must_ok(upstream_listener.local_addr());
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
        if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
            panic!("mock websocket upstream should send response: {error}");
        }
    });
    let router_port = reserve_loopback_port();
    let router_port_text = router_port.to_string();
    let upstream_base_url = format!("http://{upstream_address}/v1");
    let client_thread = thread::spawn(move || {
        let mut client = connect_tokenless_websocket_with_retry(router_port);
        let first_frame = r#"{"type":"response.create","cli":true}"#;
        if let Err(error) = client.send(Message::text(first_frame)) {
            panic!("local websocket client should send first frame: {error}");
        }
        match client.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("local websocket client should read response: {error}"),
        }
    });

    let output = run_cli(
        [
            "codex-router",
            "serve",
            "--listen-host",
            "127.0.0.1",
            "--port",
            router_port_text.as_str(),
            "--state-db",
            path_to_str(&state_path),
            "--secret-root",
            path_to_str(&secret_root),
            "--upstream-base-url",
            upstream_base_url.as_str(),
            "--now-unix-seconds",
            "1030",
            "--max-snapshot-age-seconds",
            "60",
            "--disable-background-quota-refresh",
            "--max-connections",
            "1",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(
        output
            .stdout
            .contains(format!("listening: 127.0.0.1:{router_port}\n").as_str())
    );
    assert!(output.stderr.is_empty());
    let client_response = match client_thread.join() {
        Ok(response) => response,
        Err(error) => panic!("client thread panicked: {error:?}"),
    };
    assert_eq!(client_response, r#"{"type":"response.completed"}"#);
    let (authorization, local_token) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream handshake should be recorded: {error}"),
    };
    assert_eq!(authorization, "Bearer cli-ws-upstream-token");
    assert_eq!(local_token, None);
    let (recorded_first_frame, _) = match upstream_receiver.recv() {
        Ok(recorded) => recorded,
        Err(error) => panic!("upstream first frame should be recorded: {error}"),
    };
    assert_eq!(
        recorded_first_frame,
        r#"{"type":"response.create","cli":true}"#
    );

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
    }
}

#[derive(Clone)]
struct LoopbackUpkeepOAuthClient {
    token_endpoint: String,
}

impl CredentialRefreshClient for LoopbackUpkeepOAuthClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("fixture OAuth client")
            .post(&self.token_endpoint)
            .header("content-type", "application/json")
            .body(
                serde_json::to_string(&serde_json::json!({
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token.expose_secret(),
                }))
                .expect("fixture OAuth request"),
            )
            .send()
            .expect("fixture OAuth response");
        assert!(response.status().is_success());
        let payload: serde_json::Value =
            serde_json::from_str(&response.text().expect("fixture OAuth body"))
                .expect("fixture OAuth payload");
        Ok(AccountCredentialBundle::imported_codex_auth(
            payload["access_token"]
                .as_str()
                .expect("access token")
                .to_owned(),
            Some(
                payload["refresh_token"]
                    .as_str()
                    .expect("refresh token")
                    .to_owned(),
            ),
        )
        .with_expires_unix_seconds(10_000_000))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_startup_maintains_idle_enabled_oauth_account_across_simulated_days() {
    use crate::credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock;

    let test_root = TestRoot::new("serve-idle-oauth-upkeep");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let enabled_id = account_id("serve-upkeep-enabled");
    let disabled_id = account_id("serve-upkeep-disabled");
    for (account_id, status, refresh_token) in [
        (
            &enabled_id,
            AccountStatus::Enabled,
            "initial-refresh-canary",
        ),
        (
            &disabled_id,
            AccountStatus::Disabled,
            "disabled-refresh-canary",
        ),
    ] {
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                "upkeep",
                status,
            )
            .with_active_credential_generation(1),
        ));
        let key = must_ok(openai_account_credential_bundle_key(account_id, 1));
        must_ok(
            secrets.write_secret(
                &key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "initial-access-canary",
                        Some(refresh_token.to_owned()),
                    )
                    .with_expires_unix_seconds(10_000_000)
                    .to_secret_string(),
                ),
            ),
        );
    }
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(enabled_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 0),
    ));
    drop(state);

    let oauth_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    must_ok(oauth_listener.set_nonblocking(true));
    let oauth_address = must_ok(oauth_listener.local_addr());
    let (oauth_call_sender, mut oauth_call_receiver) = tokio::sync::mpsc::unbounded_channel();
    let oauth_thread = thread::spawn(move || {
        for (call, expected_refresh) in
            [(1, "initial-refresh-canary"), (2, "rotated-refresh-canary")]
        {
            let deadline = Instant::now() + Duration::from_secs(3);
            let (mut stream, _) = loop {
                match oauth_listener.accept() {
                    Ok(connection) => break connection,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::yield_now()
                    }
                    Err(error) => panic!("fixture OAuth accept failed: {error}"),
                }
            };
            must_ok(stream.set_nonblocking(false));
            must_ok(stream.set_read_timeout(Some(Duration::from_secs(2))));
            let mut request = Vec::new();
            let expected_fragment = format!("\"refresh_token\":\"{expected_refresh}\"");
            loop {
                let mut buffer = [0_u8; 2048];
                let bytes_read = must_ok(stream.read(&mut buffer));
                assert!(bytes_read > 0, "fixture OAuth request ended early");
                request.extend_from_slice(&buffer[..bytes_read]);
                if String::from_utf8_lossy(&request).contains(&expected_fragment) {
                    break;
                }
            }
            assert!(String::from_utf8_lossy(&request).starts_with("POST /oauth/token HTTP/1.1"));
            let response_body = format!(
                r#"{{"access_token":"upkeep-access-{call}","refresh_token":"rotated-refresh-canary"}}"#,
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len(),
            );
            must_ok(stream.write_all(response.as_bytes()));
            must_ok(oauth_call_sender.send(call));
        }
    });

    let router_port = reserve_loopback_port();
    let command = must_ok(CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--listen-host"),
        OsString::from("127.0.0.1"),
        OsString::from("--port"),
        OsString::from(router_port.to_string()),
        OsString::from("--state-db"),
        state_path.as_os_str().to_os_string(),
        OsString::from("--secret-root"),
        secret_root.as_os_str().to_os_string(),
        OsString::from("--upstream-base-url"),
        OsString::from("http://127.0.0.1:1/v1"),
        OsString::from("--now-unix-seconds"),
        OsString::from("1000"),
        OsString::from("--disable-background-quota-refresh"),
        OsString::from("--max-connections"),
        OsString::from("1"),
    ]));
    let CliCommand::Serve(command) = command else {
        panic!("fixture serve command should parse");
    };
    let clock = Arc::new(AtomicU64::new(1_000));
    let worker_clock = Arc::clone(&clock);
    let (wake_sender, mut wake_receiver) = tokio::sync::mpsc::unbounded_channel();
    let oauth_client = LoopbackUpkeepOAuthClient {
        token_endpoint: format!("http://{oauth_address}/oauth/token"),
    };
    let credential_store = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let serve_task = tokio::spawn(async move {
        let mut stdout = Vec::new();
        let result = run_serve_command_with_upkeep_start(
            &mut stdout,
            command,
            credential_store,
            move |state_path, credential_store| async move {
                let clock = Arc::clone(&worker_clock);
                let worker = start_background_credential_upkeep_worker_with_client_and_clock(
                    state_path,
                    credential_store,
                    oauth_client,
                    move || clock.load(Ordering::SeqCst),
                )
                .await?;
                must_ok(wake_sender.send(worker.wake_handle_for_test()));
                Ok(worker)
            },
        )
        .await;
        (result, stdout)
    });
    let wake_handle = tokio::time::timeout(Duration::from_secs(2), wake_receiver.recv())
        .await
        .unwrap_or_else(|error| panic!("upkeep worker wake handle should be sent: {error}"))
        .expect("upkeep worker wake handle should arrive");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), oauth_call_receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("first OAuth call should arrive: {error}"))
            .expect("first OAuth call should send"),
        1
    );
    wait_for_upkeep_generation_async(&state_path, &enabled_id, 2).await;
    clock.store(1_000 + 2 * 86_400, Ordering::SeqCst);
    wake_handle.wake();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), oauth_call_receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("second OAuth call should arrive: {error}"))
            .expect("second OAuth call should send"),
        2
    );
    wait_for_upkeep_generation_async(&state_path, &enabled_id, 3).await;

    let mut health_client = must_ok(TcpStream::connect(("127.0.0.1", router_port)));
    must_ok(
        health_client
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
    );
    let mut health_response = String::new();
    must_ok(health_client.read_to_string(&mut health_response));
    assert!(health_response.starts_with("HTTP/1.1 200 OK"));
    let (serve_result, stdout) = serve_task
        .await
        .unwrap_or_else(|error| panic!("serve task should join: {error}"));
    must_ok(serve_result);
    assert!(String::from_utf8_lossy(&stdout).contains("listening: 127.0.0.1:"));
    must_ok(oauth_thread.join().map_err(|_| "OAuth thread failed"));
    let state = must_ok(SqliteStateStore::open(&state_path));
    let disabled = must_ok(AccountStateRepository::load_account(&state, &disabled_id))
        .expect("disabled account should remain");
    assert_eq!(disabled.active_credential_generation(), Some(1));
    assert_eq!(disabled.status(), AccountStatus::Disabled);
}
