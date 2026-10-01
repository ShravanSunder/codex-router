use super::*;

struct FixedClaudeQuotaCredentialResolver;

impl crate::credential_runtime::AsyncProviderCredentialResolver
    for FixedClaudeQuotaCredentialResolver
{
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            SecretString::new("claude-quota-http-access-token"),
            1,
        ))
    }
}

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
fn claude_usage_http_response_is_persisted_as_started_window_observations() {
    use codex_router_core::provider::Provider;
    use codex_router_core::route_profile::WindowKind;
    use codex_router_state::sqlite::AsyncSqliteStateStore;

    let test_root = TestRoot::new("claude-quota-http-window-observations");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    ensure_async_state_schema(&router_root);
    let state_path = router_root.join("state.sqlite");
    let account_id = account_id("acct_claude_usage_persisted");
    let state = must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(
        test_async_runtime().block_on(
            state.upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "claude-http",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            ),
        ),
    );
    must_ok(test_async_runtime().block_on(state.close()));

    let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let address = must_ok(listener.local_addr());
    let (request_sender, request_receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("Claude usage mock should accept: {error}"),
        };
        if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
            panic!("Claude usage mock should set read timeout: {error}");
        }
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let count = match stream.read(&mut buffer) {
                Ok(count) => count,
                Err(error) => panic!("Claude usage mock should read request: {error}"),
            };
            request.extend_from_slice(&buffer[..count]);
            if let Some(position) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break position + 4;
            }
        };
        request_sender
            .send(String::from_utf8_lossy(&request[..header_end]).into_owned())
            .expect("test should receive Claude request headers");
        let body = r#"{"five_hour":{"utilization":25,"resets_at":"2030-01-01T00:00:00Z"},"seven_day":{"utilization":80,"resets_at":"2030-01-07T00:00:00Z"},"oauth_app":{}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        if let Err(error) = stream.write_all(response.as_bytes()) {
            panic!("Claude usage mock should write response: {error}");
        }
    });

    let resolver = FixedClaudeQuotaCredentialResolver;
    let provider = must_ok(
        HttpQuotaRefreshProvider::new_with_claude_usage_endpoint_for_test(
            Duration::from_secs(2),
            format!("http://{address}/api/oauth/usage"),
        ),
    );
    let started_before_refresh = must_ok(codex_router_auth::resolver::current_unix_seconds());
    let mut stdout = Vec::new();
    must_ok(refresh_quota_with_dependencies(
        &mut stdout,
        router_root,
        "unused-openai-base-url".to_owned(),
        &resolver,
        &provider,
        1_100,
    ));
    let started_after_refresh = must_ok(codex_router_auth::resolver::current_unix_seconds());

    let request =
        must_ok(request_receiver.recv_timeout(Duration::from_secs(2))).to_ascii_lowercase();
    assert!(request.starts_with("get /api/oauth/usage http/1.1"));
    assert!(request.contains("authorization: bearer claude-quota-http-access-token"));
    assert!(request.contains("anthropic-beta: oauth-2025-04-20"));
    assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 1\n");
    let observations_state =
        must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open_read_only(&state_path)));
    let observations = must_ok(
        test_async_runtime()
            .block_on(observations_state.window_observations_for_account(&account_id)),
    );
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0].window_kind(), WindowKind::FiveHour);
    assert_eq!(observations[0].remaining_basis_points(), 7_500);
    assert_eq!(observations[0].reset_unix_seconds(), Some(1_893_456_000));
    assert_eq!(observations[1].window_kind(), WindowKind::Weekly);
    assert_eq!(observations[1].remaining_basis_points(), 2_000);
    assert_eq!(observations[1].reset_unix_seconds(), Some(1_893_974_400));
    for observation in observations {
        assert!(observation.observation_started_at() >= started_before_refresh);
        assert!(observation.observation_started_at() <= started_after_refresh);
    }
    must_ok(test_async_runtime().block_on(observations_state.close()));
    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("Claude usage mock thread panicked: {error:?}"),
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

struct RecoveringClaudeQuotaCredentialResolver {
    recovery_calls: std::sync::atomic::AtomicUsize,
}

impl crate::credential_runtime::AsyncProviderCredentialResolver
    for RecoveringClaudeQuotaCredentialResolver
{
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        if expected_provider != codex_router_core::provider::Provider::Claude {
            return Err(CredentialResolverError::AccountProviderMismatch);
        }
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            SecretString::new("claude-quota-old-access"),
            1,
        ))
    }

    async fn recover_unauthorized_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
        rejected_generation: u64,
    ) -> Result<(ResolvedProviderCredential, bool), CredentialResolverError> {
        if expected_provider != codex_router_core::provider::Provider::Claude
            || rejected_generation != 1
        {
            return Err(CredentialResolverError::AccountIneligible);
        }
        self.recovery_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok((
            ResolvedProviderCredential::new(
                account_id.clone(),
                SecretString::new("claude-quota-recovered-access"),
                2,
            ),
            true,
        ))
    }
}

struct RecordingClaudeQuotaProvider {
    unauthorized_once: bool,
    requests: std::sync::Mutex<Vec<(String, String)>>,
}

impl RecordingClaudeQuotaProvider {
    fn new(unauthorized_once: bool) -> Self {
        Self {
            unauthorized_once,
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn recorded_requests(&self) -> Vec<(String, String)> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl QuotaRefreshProvider for RecordingClaudeQuotaProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        let call_number = {
            let mut requests = self
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            requests.push((
                request.account_id().as_str().to_owned(),
                request.access_token().expose_secret().to_owned(),
            ));
            requests.len()
        };
        if self.unauthorized_once && call_number == 1 {
            return Err(crate::quota::QuotaCommandError::ProviderStatus { status: 401 });
        }
        Ok(QuotaRefreshProviderResponse {
            windows: vec![QuotaRefreshProviderWindow {
                limit_window_seconds: 18_000,
                headroom: crate::quota::QuotaWindowHeadroom::BasisPoints(7_500),
                reset_unix_seconds: Some(20_000),
                effective: true,
            }],
            reset_credits_available: None,
        })
    }
}

#[test]
fn claude_quota_401_recovers_credentials_and_retries_the_usage_request() {
    use codex_router_core::provider::Provider;
    use codex_router_state::sqlite::AsyncSqliteStateStore;
    use std::sync::atomic::Ordering;

    let test_root = TestRoot::new("claude-quota-401-recovery");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let account_id = account_id("acct_claude_401_recovery");
    let state = must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(
        test_async_runtime().block_on(
            state.upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "claude-401",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            ),
        ),
    );
    must_ok(test_async_runtime().block_on(state.close()));

    let resolver = RecoveringClaudeQuotaCredentialResolver {
        recovery_calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let provider = RecordingClaudeQuotaProvider::new(true);
    let mut stdout = Vec::new();
    must_ok(
        test_async_runtime().block_on(refresh_quota_store_paths_with_dependencies_async(
            &mut stdout,
            &state_path,
            &router_root.join("secrets"),
            "unused-openai-base-url".to_owned(),
            &resolver,
            &provider,
            2_000,
        )),
    );

    assert_eq!(resolver.recovery_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider.recorded_requests(),
        vec![
            (
                account_id.as_str().to_owned(),
                "claude-quota-old-access".to_owned(),
            ),
            (
                account_id.as_str().to_owned(),
                "claude-quota-recovered-access".to_owned(),
            ),
        ]
    );
    assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 1\n");
}

#[test]
fn background_quota_refresh_polls_only_claude_accounts_idle_longer_than_interval() {
    use codex_router_core::ids::ReservationId;
    use codex_router_core::provider::Provider;
    use codex_router_state::sqlite::AsyncSqliteStateStore;

    let test_root = TestRoot::new("claude-background-quota-idle-only");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let state = must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(&state_path)));
    let labels = ["busy", "recent", "idle", "never-used"];
    let accounts = labels.map(|label| {
        AccountRecord::new(
            Provider::Claude,
            account_id(&format!("acct_claude_{label}")),
            label,
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1)
    });
    for account in &accounts {
        must_ok(test_async_runtime().block_on(state.upsert_account(account)));
    }
    let busy_reservation = ReservationId::new("busy-reservation");
    must_ok(
        test_async_runtime().block_on(state.record_active_client_acquired(
            "claude_messages",
            "busy-process",
            &busy_reservation,
            accounts[0].account_id(),
            1_000,
            0,
        )),
    );
    for (account_index, acquired_at, released_at, process_id) in
        [(1, 1_600, 1_800, "recent"), (2, 1_000, 1_100, "idle")]
    {
        let reservation = ReservationId::new(format!("{process_id}-reservation"));
        must_ok(
            test_async_runtime().block_on(state.record_active_client_acquired(
                "claude_messages",
                process_id,
                &reservation,
                accounts[account_index].account_id(),
                acquired_at,
                0,
            )),
        );
        must_ok(
            test_async_runtime().block_on(state.record_active_client_released(
                "claude_messages",
                process_id,
                &reservation,
                released_at,
            )),
        );
    }
    must_ok(test_async_runtime().block_on(state.close()));

    let provider = RecordingClaudeQuotaProvider::new(false);
    let resolver = FixedClaudeQuotaCredentialResolver;
    let mut stdout = Vec::new();
    must_ok(test_async_runtime().block_on(
        crate::quota::refresh_quota_store_paths_with_dependencies_and_floor_notifier(
            &mut stdout,
            &state_path,
            &router_root.join("secrets"),
            "unused-openai-base-url".to_owned(),
            &resolver,
            &provider,
            crate::quota::QuotaRefreshObservationContext {
                observed_unix_seconds: 2_000,
                schedule: crate::quota::QuotaRefreshSchedule::Background {
                    interval_seconds: 400,
                },
                weekly_floor_observer: None,
            },
        ),
    ));

    let polled_account_ids = provider
        .recorded_requests()
        .into_iter()
        .map(|(account_id, _token)| account_id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        polled_account_ids,
        std::collections::HashSet::from([
            accounts[2].account_id().as_str().to_owned(),
            accounts[3].account_id().as_str().to_owned(),
        ])
    );
    assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");
}
