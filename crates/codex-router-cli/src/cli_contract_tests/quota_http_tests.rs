use super::*;

#[test]
fn quota_refresh_http_provider_fetches_usage_and_persists_sqlite_state() {
    let test_root = TestRoot::new("quota-refresh-http-provider");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_quota_http");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "quota-http",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &bundle_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "quota-http-access-token",
                    Some("quota-http-refresh-token".to_owned()),
                )
                .with_expires_unix_seconds(2_000)
                .to_secret_string(),
            ),
        ),
    );

    let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let address = must_ok(listener.local_addr());
    let server_thread = thread::spawn(move || {
        for _request_index in 0..4 {
            let (mut stream, _peer_address) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept: {error}"),
            };
            if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
                panic!("quota mock should set read timeout: {error}");
            }
            let mut buffer = [0_u8; 4096];
            let bytes_read = match stream.read(&mut buffer) {
                Ok(bytes_read) => bytes_read,
                Err(error) => panic!("quota mock should read request: {error}"),
            };
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            let is_usage_request = request.starts_with("GET /api/codex/usage HTTP/1.1\r\n");
            let is_reset_credits_request =
                request.starts_with("GET /api/codex/rate-limit-reset-credits HTTP/1.1\r\n");
            assert!(
                is_usage_request || is_reset_credits_request,
                "unexpected quota mock request: {request}"
            );
            assert!(request.contains("authorization: Bearer quota-http-access-token\r\n"));
            assert!(!request.to_ascii_lowercase().contains("openai-beta:"));
            assert!(!request.to_ascii_lowercase().contains("originator:"));
            let body = if is_usage_request {
                r#"{
                        "rate_limit": {
                            "primary_window": null,
                            "secondary_window": {
                                "used_percent": 80,
                                "reset_at": 9000,
                                "limit_window_seconds": 604800
                            }
                        },
                        "additional_rate_limits": [{
                            "limit_name": "codex_other",
                            "metered_feature": "codex_other",
                            "rate_limit": {
                                "primary_window": {
                                    "used_percent": 25,
                                    "reset_at": 2000,
                                    "limit_window_seconds": 18000
                                },
                                "secondary_window": null
                            }
                        }],
                        "reset_credits": {"available": 1}
                    }"#
            } else {
                r#"{"reset_credits":{"available":1}}"#
            };
            // The mock serves one request per connection, so it must tell the
            // client not to pool it; otherwise the follow-up request can race
            // the close and fail with "error sending request".
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                panic!("quota mock should write response: {error}");
            }
        }
    });
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    let provider = must_ok(HttpQuotaRefreshProvider::new());
    let mut stdout = Vec::new();

    must_ok(refresh_quota_with_dependencies(
        &mut stdout,
        router_root,
        format!("http://{address}"),
        &resolver,
        &provider,
        1_100,
    ));

    for route_band in ["responses", "models"] {
        let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &account_id,
            route_band,
        ))
        .unwrap_or_else(|| panic!("{route_band} quota snapshot should be persisted"));
        assert_eq!(snapshot.remaining_headroom(), 20);
        assert_eq!(snapshot.reset_unix_seconds(), Some(9_000));
        assert_eq!(snapshot.reset_credits_available(), Some(1));
        assert_eq!(snapshot.source(), QuotaSnapshotSource::OpenAiEndpoint);
    }
    let selector_inputs = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
        &state,
        "responses",
        1_100,
    ));
    assert_eq!(selector_inputs.len(), 1);
    let windows = selector_inputs[0].windows();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].limit_window_seconds(), 604_800);
    assert_eq!(windows[0].remaining_headroom(), 20);
    assert_eq!(windows[0].reset_unix_seconds(), Some(9_000));
    assert!(windows[0].effective());
    assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");

    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("quota mock thread panicked: {error:?}"),
    }
}

#[test]
fn loopback_quota_401_retries_with_a_concurrently_committed_generation() {
    let test_root = TestRoot::new("quota-http-401-generation-race");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("quota-http-401-race-account");
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "quota-http-race",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1),
    ));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let active = AccountCredentialBundle::imported_codex_auth(
        "rejected-access-canary",
        Some("original-refresh-canary".to_owned()),
    )
    .with_expires_unix_seconds(4_000_000_000);
    must_ok(secrets.write_secret(&active_key, &must_ok(active.to_secret_string())));

    let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    must_ok(listener.set_nonblocking(true));
    let address = must_ok(listener.local_addr());
    let server_state_path = state_path.clone();
    let server_secret_root = secret_root.clone();
    let server_account_id = account_id.clone();
    let server_thread = thread::spawn(move || {
        let expected_paths = [
            "/api/codex/usage",
            "/api/codex/usage",
            "/api/codex/rate-limit-reset-credits",
            "/api/codex/usage",
            "/api/codex/rate-limit-reset-credits",
        ];
        for (request_index, path) in expected_paths.iter().enumerate() {
            let deadline = Instant::now() + Duration::from_secs(3);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::yield_now();
                    }
                    Err(error) => panic!("fixture quota request {request_index} missing: {error}"),
                }
            };
            must_ok(stream.set_nonblocking(false));
            must_ok(stream.set_read_timeout(Some(Duration::from_secs(2))));
            let mut request_bytes = Vec::new();
            while !request_bytes.ends_with(b"\r\n\r\n") {
                let mut buffer = [0_u8; 1024];
                let bytes_read = must_ok(stream.read(&mut buffer));
                assert!(bytes_read > 0, "fixture quota request ended early");
                request_bytes.extend_from_slice(&buffer[..bytes_read]);
                assert!(
                    request_bytes.len() <= 4096,
                    "fixture quota headers too large"
                );
            }
            let request = String::from_utf8_lossy(&request_bytes);
            assert!(request.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
            let expected_access = if request_index == 0 {
                "rejected-access-canary"
            } else {
                "concurrent-access-canary"
            };
            assert!(request.contains(&format!("authorization: Bearer {expected_access}\r\n")));
            if request_index == 0 {
                let successor_key =
                    must_ok(openai_account_credential_bundle_key(&server_account_id, 2));
                let successor = AccountCredentialBundle::imported_codex_auth(
                    "concurrent-access-canary",
                    Some("concurrent-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(5_000_000_000);
                let secrets = must_ok(
                    codex_router_secret_store::test_support::open_encrypted_credential_store(
                        &server_secret_root,
                    ),
                );
                must_ok(
                    secrets.write_secret(&successor_key, &must_ok(successor.to_secret_string())),
                );
                let state = must_ok(SqliteStateStore::open(&server_state_path));
                must_ok(AccountStateRepository::upsert_account(
                    &state,
                    &AccountRecord::new(
                        codex_router_core::provider::Provider::Openai,
                        server_account_id.clone(),
                        "quota-http-race",
                        AccountStatus::Enabled,
                    )
                    .with_active_credential_generation(2),
                ));
                must_ok(stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                ));
                continue;
            }
            let body = if *path == "/api/codex/usage" {
                r#"{"rate_limit":{"primary_window":null,"secondary_window":{"used_percent":58,"reset_at":9000,"limit_window_seconds":604800}}}"#
            } else {
                r#"{"reset_credits":{"available":1}}"#
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            must_ok(stream.write_all(response.as_bytes()));
        }
    });

    let refresh_client = RecordingRefreshClient::new(
        "quota-http-401-race-account",
        "original-refresh-canary",
        AccountCredentialBundle::imported_codex_auth("unexpected-refresh", None),
    );
    let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
        &state_path,
        &secret_root,
        refresh_client.clone(),
    ));
    let provider = must_ok(HttpQuotaRefreshProvider::new());
    let mut output = Vec::new();
    let result = refresh_quota_store_paths_with_dependencies(
        &mut output,
        &state_path,
        &secret_root,
        format!("http://{address}"),
        &resolver,
        &provider,
        1_100,
    );
    must_ok(
        server_thread
            .join()
            .map_err(|_| "fixture quota server failed"),
    );
    must_ok(result);
    assert_eq!(refresh_client.calls(), 0);
    let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .expect("account should remain");
    assert_eq!(account.active_credential_generation(), Some(2));
    assert_eq!(account.status(), AccountStatus::Enabled);
    for route_band in ["responses", "models"] {
        let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &account_id,
            route_band,
        ))
        .expect("quota snapshot should be saved");
        assert_eq!(snapshot.remaining_headroom(), 42);
    }
    assert_eq!(must_ok(String::from_utf8(output)), "refreshed: 2\n");
}

#[test]
fn quota_refresh_http_provider_times_out_hanging_usage_endpoint() {
    let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let address = must_ok(listener.local_addr());
    let server_thread = thread::spawn(move || {
        let (mut stream, _peer_address) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("quota mock should accept: {error}"),
        };
        let mut buffer = [0_u8; 1024];
        let _bytes_read = stream.read(&mut buffer);
        thread::sleep(Duration::from_millis(200));
    });
    let provider = must_ok(HttpQuotaRefreshProvider::new_with_timeout(
        Duration::from_millis(10),
    ));

    let error =
        match test_async_runtime().block_on(provider.fetch_quota(QuotaRefreshProviderRequest::new(
            account_id("acct_timeout"),
            "timeout",
            "responses",
            format!("http://{address}"),
            SecretString::new("timeout-token-canary"),
            None,
        ))) {
            Ok(response) => panic!("hanging quota endpoint should time out: {response:?}"),
            Err(error) => error,
        };

    assert!(error.to_string().contains("quota refresh request failed"));
    assert!(!error.to_string().contains("timeout-token-canary"));
    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("quota mock thread panicked: {error:?}"),
    }
}
