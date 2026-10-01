use super::*;

#[test]
fn quota_refresh_http_provider_projects_weekly_window_when_primary_is_null() {
    let test_root = TestRoot::new("quota-http-null-primary-window");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_quota_http_null_primary");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "quota-http-null-primary",
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
                    "quota-null-primary-access-token",
                    Some("quota-null-primary-refresh-token".to_owned()),
                )
                .with_expires_unix_seconds(2_000)
                .to_secret_string(),
            ),
        ),
    );

    let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let address = must_ok(listener.local_addr());
    let server_thread = thread::spawn(move || {
        for request_index in 0..4 {
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
            assert!(request.contains("authorization: Bearer quota-null-primary-access-token\r\n"));
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
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                panic!("quota mock should write response {request_index}: {error}");
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
        Err(error) => panic!("quota mock should finish: {error:?}"),
    }
}
