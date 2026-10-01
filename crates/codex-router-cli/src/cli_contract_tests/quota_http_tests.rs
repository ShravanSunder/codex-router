use super::*;

struct ReversedResponsesRefreshProvider {
    older_responses_read_started: tokio::sync::Notify,
    allow_older_responses_failure: tokio::sync::Notify,
    newer_responses_commit_reached_models: tokio::sync::Notify,
}

impl ReversedResponsesRefreshProvider {
    fn quota_response(route_band: &str) -> QuotaRefreshProviderResponse {
        let windows = if route_band == "responses" {
            vec![
                QuotaRefreshProviderWindow {
                    limit_window_seconds: 18_000,
                    remaining_headroom: 35,
                    reset_unix_seconds: Some(18_000),
                    effective: true,
                },
                QuotaRefreshProviderWindow {
                    limit_window_seconds: 604_800,
                    remaining_headroom: 20,
                    reset_unix_seconds: Some(604_800),
                    effective: false,
                },
            ]
        } else {
            vec![QuotaRefreshProviderWindow {
                limit_window_seconds: 18_000,
                remaining_headroom: 70,
                reset_unix_seconds: Some(18_000),
                effective: true,
            }]
        };
        let credit_provider_observation = if route_band == "responses" {
            codex_router_core::credit_usage::CreditProviderObservation::new(
                codex_router_core::credit_usage::CreditAvailability::Available {
                    balance: Some(
                        codex_router_core::credit_usage::CreditBalance::new("0.75")
                            .expect("fixed loopback balance should validate"),
                    ),
                },
                codex_router_core::credit_usage::CreditSpendControl::Clear,
                Some(codex_router_core::credit_usage::CreditProviderLimitReason::RateLimitReached),
            )
        } else {
            codex_router_core::credit_usage::CreditProviderObservation::missing()
        };
        QuotaRefreshProviderResponse {
            windows,
            reset_credits_available: None,
            credit_provider_observation,
        }
    }
}

impl QuotaRefreshProvider for ReversedResponsesRefreshProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
        match (request.base_url(), request.route_band()) {
            ("older-run", "responses") => {
                self.older_responses_read_started.notify_one();
                self.allow_older_responses_failure.notified().await;
                Err(QuotaCommandError::ProviderStatus { status: 429 })
            }
            ("newer-run", "responses") => Ok(Self::quota_response("responses")),
            ("newer-run", "models") => {
                self.newer_responses_commit_reached_models.notify_one();
                Ok(Self::quota_response("models"))
            }
            (_, route_band) => Ok(Self::quota_response(route_band)),
        }
    }
}

#[tokio::test]
async fn older_responses_provider_failure_cannot_replace_newer_service_commit() {
    let test_root = TestRoot::new("quota-refresh-reversed-service-completion");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("acct_reversed_service_completion");
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "reversed",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1),
    ));
    let secret_root = router_root.join("secrets");
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let credential_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &credential_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "reversed-refresh-access-canary",
                    Some("reversed-refresh-token-canary".to_owned()),
                )
                .with_expires_unix_seconds(4_000_000_000)
                .to_secret_string(),
            ),
        ),
    );
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    let policy_state = must_ok(AsyncSqliteStateStore::open(&state_path).await);
    must_ok(
        policy_state
            .save_account_credit_usage_policy(
                &account_id,
                codex_router_core::credit_usage::CreditUsagePolicy::Allow,
            )
            .await,
    );
    must_ok(policy_state.close().await);

    let provider = ReversedResponsesRefreshProvider {
        older_responses_read_started: tokio::sync::Notify::new(),
        allow_older_responses_failure: tokio::sync::Notify::new(),
        newer_responses_commit_reached_models: tokio::sync::Notify::new(),
    };
    let mut older_output = Vec::new();
    let mut newer_output = Vec::new();
    let older_refresh = refresh_quota_with_dependencies_async(
        &mut older_output,
        router_root.clone(),
        "older-run".to_owned(),
        &resolver,
        &provider,
        100,
    );
    tokio::pin!(older_refresh);
    let older_started = provider.older_responses_read_started.notified();
    tokio::pin!(older_started);
    tokio::select! {
        () = &mut older_started => {}
        result = &mut older_refresh => panic!("older refresh completed before its Responses read blocked: {result:?}"),
    }

    let newer_refresh = refresh_quota_with_dependencies_async(
        &mut newer_output,
        router_root,
        "newer-run".to_owned(),
        &resolver,
        &provider,
        200,
    );
    tokio::pin!(newer_refresh);
    let newer_committed = provider.newer_responses_commit_reached_models.notified();
    tokio::pin!(newer_committed);
    tokio::select! {
        () = &mut newer_committed => {}
        result = &mut newer_refresh => panic!("newer refresh completed before its Responses commit reached Models: {result:?}"),
    }
    provider.allow_older_responses_failure.notify_one();
    let (older_result, newer_result) = tokio::join!(older_refresh, newer_refresh);
    must_ok(newer_result);
    must_ok(older_result);

    let async_state = must_ok(AsyncSqliteStateStore::open_read_only(&state_path).await);
    let observation = async_state
        .load_account_credit_observation(&account_id)
        .await
        .expect("latest credit observation should read")
        .expect("newer provider observation should persist");
    assert_eq!(observation.latest_started_attempt(), 2);
    assert_eq!(observation.committed_attempt(), Some(2));
    assert_eq!(observation.observed_unix_seconds(), Some(200));
    assert_eq!(
        observation.provider_observation().availability(),
        &codex_router_core::credit_usage::CreditAvailability::Available {
            balance: Some(
                codex_router_core::credit_usage::CreditBalance::new("0.75")
                    .expect("fixed loopback balance should validate"),
            ),
        }
    );
    let windows = async_state
        .selector_inputs_for_route_band("responses", 201)
        .await
        .expect("newer selector windows should read");
    let input = windows
        .iter()
        .find(|input| input.account_id() == &account_id)
        .expect("account should remain in selector snapshot");
    assert_eq!(input.windows().len(), 2);
    assert!(input.has_current_credit_authority(201));
    let status = async_state
        .quota_refresh_statuses_for_route_band("responses")
        .await
        .expect("refresh status should read")
        .into_iter()
        .find(|status| status.account_id() == &account_id)
        .expect("newer success status should remain");
    assert_eq!(status.last_success_unix_seconds(), Some(200));
    assert_eq!(status.last_attempt_unix_seconds(), Some(200));
    assert_eq!(status.last_error_class(), None);
    let response_history = async_state
        .quota_history_observations_for_window(&account_id, "responses", 604_800, 0, 250)
        .await
        .expect("Responses history should read");
    assert_eq!(response_history.len(), 1);
    assert_eq!(response_history[0].observed_unix_seconds(), 200);
    assert_eq!(response_history[0].remaining_headroom(), 20);
    let snapshot = async_state
        .load_quota_snapshot_for_route_band(&account_id, "responses")
        .await
        .expect("Responses snapshot should read")
        .expect("newer scalar snapshot should persist");
    assert_eq!(snapshot.observed_unix_seconds(), 200);
    assert_eq!(snapshot.remaining_headroom(), 35);
    must_ok(async_state.close().await);
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
    let policy_state = must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    )));
    must_ok(
        test_async_runtime().block_on(policy_state.save_account_credit_usage_policy(
            &account_id,
            codex_router_core::credit_usage::CreditUsagePolicy::Allow,
        )),
    );
    must_ok(test_async_runtime().block_on(policy_state.close()));
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
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
        for request_index in 0..8 {
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
                if request_index < 4 {
                    r#"{
                        "rate_limit": {
                            "primary_window": {
                                "used_percent": 100,
                                "reset_at": 8000,
                                "limit_window_seconds": 18000
                            },
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
                        "credits": {
                            "has_credits": true,
                            "unlimited": false,
                            "balance": "0.25"
                        },
                        "rate_limit_reached_type": {"type": "rate_limit_reached"},
                        "reset_credits": {"available": 1}
                    }"#
                } else {
                    r#"{
                        "rate_limit": {
                            "primary_window": {
                                "used_percent": 60,
                                "reset_at": 8000,
                                "limit_window_seconds": 18000
                            },
                            "secondary_window": {
                                "used_percent": 100,
                                "reset_at": 9000,
                                "limit_window_seconds": 604800
                            }
                        },
                        "additional_rate_limits": [],
                        "credits": {
                            "has_credits": true,
                            "unlimited": false,
                            "balance": "0.25"
                        },
                        "rate_limit_reached_type": {"type": "rate_limit_reached"},
                        "reset_credits": {"available": 1}
                    }"#
                }
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
        router_root.clone(),
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
        assert_eq!(snapshot.remaining_headroom(), 0);
        assert_eq!(snapshot.reset_unix_seconds(), Some(8_000));
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
    assert_eq!(windows.len(), 2);
    let five_hour_window = windows
        .iter()
        .find(|window| window.limit_window_seconds() == 18_000)
        .expect("provider's exhausted five-hour window should be projected");
    assert_eq!(
        five_hour_window.status(),
        SelectorQuotaWindowStatus::Ineligible
    );
    assert_eq!(five_hour_window.remaining_headroom(), 0);
    assert_eq!(five_hour_window.reset_unix_seconds(), Some(8_000));
    assert!(five_hour_window.effective());
    let weekly_window = windows
        .iter()
        .find(|window| window.limit_window_seconds() == 604_800)
        .expect("provider's healthy weekly window should be projected");
    assert_eq!(weekly_window.status(), SelectorQuotaWindowStatus::Eligible);
    assert_eq!(weekly_window.remaining_headroom(), 20);
    assert_eq!(weekly_window.reset_unix_seconds(), Some(9_000));
    assert!(!weekly_window.effective());
    let async_state = must_ok(test_async_runtime().block_on(
        AsyncSqliteStateStore::open_read_only(&router_root.join("state.sqlite")),
    ));
    let selector_inputs = must_ok(
        test_async_runtime()
            .block_on(async_state.selector_inputs_for_route_band("responses", 1_101)),
    );
    let selector_input = selector_inputs
        .iter()
        .find(|input| input.account_id() == &account_id)
        .expect("Responses selector input should remain paired with credit facts");
    assert_eq!(
        selector_input.credit_usage_policy(),
        codex_router_core::credit_usage::CreditUsagePolicy::Allow
    );
    assert!(selector_input.has_current_credit_authority(1_101));
    let credit_observation = selector_input
        .credit_observation()
        .expect("loopback credit facts should be available in selector projection");
    assert_eq!(credit_observation.latest_started_attempt(), 1);
    assert_eq!(credit_observation.committed_attempt(), Some(1));
    assert_eq!(credit_observation.observed_unix_seconds(), Some(1_100));
    assert_eq!(
        credit_observation.provider_observation().availability(),
        &codex_router_core::credit_usage::CreditAvailability::Available {
            balance: Some(
                codex_router_core::credit_usage::CreditBalance::new("0.25")
                    .expect("valid provider balance"),
            ),
        }
    );
    let route_projection = must_ok(test_async_runtime().block_on(
        codex_router_state::selection_projection::project_route_band_selection_inputs_read_only(
            &async_state,
            "responses",
            1_101,
            300,
        ),
    ));
    let projected_account = route_projection
        .accounts()
        .iter()
        .find(|input| input.account_id() == &account_id)
        .expect("real state projection should include the refreshed account");
    let projected_five_hour = projected_account
        .windows()
        .iter()
        .find(|window| window.window_seconds() == 18_000)
        .expect("read-only projection should include the persisted zero window");
    assert_eq!(
        projected_five_hour.status(),
        codex_router_selection::burn_down::QuotaWindowStatus::Ineligible
    );
    assert_eq!(projected_five_hour.remaining_headroom(), 0);
    let assessment = codex_router_selection::burn_down::assess_route_band(
        codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput::new(
            codex_router_core::routes::RouteBand::Responses,
            1_101,
            codex_router_core::route_profile::RESPONSES_HTTP.clone(),
            route_projection.accounts().to_vec(),
        ),
    );
    assert_eq!(
        assessment.selected_pool(),
        codex_router_selection::burn_down::SelectedPool::Reserve
    );
    let credit_routed_account = assessment
        .accounts()
        .iter()
        .find(|account| account.account_id() == &account_id)
        .expect("selected account should remain in the burn-down report");
    assert_eq!(
        credit_routed_account.quota_evidence_reason(),
        codex_router_selection::burn_down::QuotaEvidenceReason::CreditBacked
    );
    assert_eq!(
        credit_routed_account.routing_reason(),
        codex_router_selection::burn_down::RoutingReason::CreditBacked
    );
    must_ok(test_async_runtime().block_on(async_state.close()));

    must_ok(refresh_quota_with_dependencies(
        &mut stdout,
        router_root.clone(),
        format!("http://{address}"),
        &resolver,
        &provider,
        1_102,
    ));

    let weekly_zero_state = must_ok(test_async_runtime().block_on(
        AsyncSqliteStateStore::open_read_only(&router_root.join("state.sqlite")),
    ));
    let weekly_zero_projection = must_ok(test_async_runtime().block_on(
        codex_router_state::selection_projection::project_route_band_selection_inputs_read_only(
            &weekly_zero_state,
            "responses",
            1_103,
            300,
        ),
    ));
    let weekly_zero_account = weekly_zero_projection
        .accounts()
        .iter()
        .find(|input| input.account_id() == &account_id)
        .expect("second real refresh should preserve the account");
    let weekly_zero_five_hour = weekly_zero_account
        .windows()
        .iter()
        .find(|window| window.window_seconds() == 18_000)
        .expect("second projection should include the five-hour window");
    assert_eq!(
        weekly_zero_five_hour.status(),
        codex_router_selection::burn_down::QuotaWindowStatus::Eligible
    );
    assert_eq!(weekly_zero_five_hour.remaining_headroom(), 40);
    let weekly_zero_window = weekly_zero_account
        .windows()
        .iter()
        .find(|window| window.window_seconds() == 604_800)
        .expect("second projection should include the weekly window");
    assert_eq!(
        weekly_zero_window.status(),
        codex_router_selection::burn_down::QuotaWindowStatus::Ineligible
    );
    assert_eq!(weekly_zero_window.remaining_headroom(), 0);
    let weekly_zero_assessment = codex_router_selection::burn_down::assess_route_band(
        codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput::new(
            codex_router_core::routes::RouteBand::Responses,
            1_103,
            codex_router_core::route_profile::RESPONSES_HTTP.clone(),
            weekly_zero_projection.accounts().to_vec(),
        ),
    );
    let weekly_zero_routed_account = weekly_zero_assessment
        .accounts()
        .iter()
        .find(|account| account.account_id() == &account_id)
        .expect("weekly-zero account should remain in the selection report");
    assert_eq!(
        weekly_zero_routed_account.quota_evidence_reason(),
        codex_router_selection::burn_down::QuotaEvidenceReason::CreditBacked
    );
    assert_eq!(
        weekly_zero_routed_account.routing_reason(),
        codex_router_selection::burn_down::RoutingReason::CreditBacked
    );
    let weekly_zero_selector_inputs = must_ok(
        test_async_runtime()
            .block_on(weekly_zero_state.selector_inputs_for_route_band("responses", 1_103)),
    );
    let latest_credit_observation = weekly_zero_selector_inputs
        .iter()
        .find(|input| input.account_id() == &account_id)
        .and_then(|input| input.credit_observation())
        .expect("second refresh should persist paired credit facts");
    assert_eq!(latest_credit_observation.latest_started_attempt(), 2);
    assert_eq!(latest_credit_observation.committed_attempt(), Some(2));
    assert_eq!(
        latest_credit_observation.observed_unix_seconds(),
        Some(1_102)
    );
    must_ok(test_async_runtime().block_on(weekly_zero_state.close()));

    let guard_state = must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    )));
    for status in [
        SelectorQuotaWindowStatus::Ineligible,
        SelectorQuotaWindowStatus::Unknown,
        SelectorQuotaWindowStatus::Stale,
    ] {
        must_ok(
            test_async_runtime().block_on(
                guard_state.upsert_selector_quota_window(
                    &PersistedSelectorQuotaWindow::new(
                        account_id.clone(),
                        "responses",
                        3_600,
                        status,
                    )
                    .with_remaining_headroom(0)
                    .with_reset_unix_seconds(2_500)
                    .with_observed_unix_seconds(1_102)
                    .with_effective(true),
                ),
            ),
        );
        let guarded_projection = must_ok(test_async_runtime().block_on(
            codex_router_state::selection_projection::project_route_band_selection_inputs_read_only(
                &guard_state,
                "responses",
                1_103,
                300,
            ),
        ));
        let guarded_assessment = codex_router_selection::burn_down::assess_route_band(
            codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput::new(
                codex_router_core::routes::RouteBand::Responses,
                1_103,
                codex_router_core::route_profile::RESPONSES_HTTP.clone(),
                guarded_projection.accounts().to_vec(),
            ),
        );
        let guarded_account = guarded_assessment
            .accounts()
            .iter()
            .find(|account| account.account_id() == &account_id)
            .expect("guarded account should remain in the selection report");
        assert_ne!(
            guarded_account.routing_reason(),
            codex_router_selection::burn_down::RoutingReason::CreditBacked,
            "unrelated persisted window status {status:?} must deny credit backing"
        );
    }
    must_ok(test_async_runtime().block_on(guard_state.close()));
    assert_eq!(
        must_ok(String::from_utf8(stdout)),
        "refreshed: 2\nrefreshed: 2\n"
    );

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
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
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
                let successor_key = must_ok(account_credential_bundle_key(&server_account_id, 2));
                let successor = AccountCredentialBundle::imported_codex_auth(
                    "concurrent-access-canary",
                    Some("concurrent-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(5_000_000_000);
                let secrets = must_ok(FileSecretStore::open(&server_secret_root));
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
