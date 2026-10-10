use super::*;
use codex_router_core::ids::TokenGeneration;
use codex_router_proxy::account_selection::AsyncAccountDecisionSelector;
use codex_router_proxy::account_selection::AsyncRepositoryBackedAccountSelector;
use codex_router_proxy::http_sse::HttpProxyRequest;
use codex_router_proxy::routes::Method;

struct GatedInitialFailureCredentialResolver {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl AsyncProviderCredentialResolver for GatedInitialFailureCredentialResolver {
    async fn resolve_provider_credentials_async(
        &self,
        _account_id: &AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        self.entered.notify_one();
        self.release.notified().await;
        Err(CredentialResolverError::AccountUnavailable)
    }
}

async fn assert_delayed_initial_failure_preserves_newer_authority(successful_generation: u64) {
    let test_root = TestRoot::new("quota-refresh-delayed-initial-resolution-failure");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let state = must_ok(AsyncSqliteStateStore::open(&state_path).await);
    let account_id = account_id("acct_delayed_initial_resolution_failure");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "delayed-initial-resolution-failure",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let initial_access = if successful_generation == 1 {
        "newer-resolution-token"
    } else {
        "superseded-generation-one-token"
    };
    let initial_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &initial_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(initial_access, None)
                    .with_expires_unix_seconds(4_000_000_000)
                    .to_secret_string(),
            ),
        ),
    );
    let (address, stop_server, provider_server) = start_credit_quota_fixture().await;

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let older_resolver = GatedInitialFailureCredentialResolver {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    };
    let older_state_path = state_path.clone();
    let older_secret_root = secret_root.clone();
    let older_refresh = tokio::spawn(async move {
        refresh_quota_store_paths_with_dependencies_async(
            &mut Vec::new(),
            &older_state_path,
            &older_secret_root,
            format!("http://{address}"),
            &older_resolver,
            &must_ok(HttpQuotaRefreshProvider::new()),
            100,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("older refresh must enter initial resolution before the newer cycle");

    let exhausted_peer_id =
        AccountId::new("acct_a_exhausted_peer").expect("fixed exhausted peer id must validate");
    assert!(exhausted_peer_id.as_str() < account_id.as_str());
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    exhausted_peer_id.clone(),
                    "exhausted-peer",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    must_ok(
        state
            .save_account_credit_usage_policy(
                &exhausted_peer_id,
                codex_router_core::credit_usage::CreditUsagePolicy::Allow,
            )
            .await,
    );
    let exhausted_key = must_ok(openai_account_credential_bundle_key(&exhausted_peer_id, 1));
    must_ok(
        secrets.write_secret(
            &exhausted_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth("exhausted-peer-token", None)
                    .with_expires_unix_seconds(4_000_000_000)
                    .to_secret_string(),
            ),
        ),
    );
    if successful_generation != 1 {
        must_ok(
            state
                .activate_account_credential_generation_and_invalidate_quota(
                    &account_id,
                    successful_generation,
                    AccountStatus::Enabled,
                )
                .await,
        );
        let current_key = must_ok(openai_account_credential_bundle_key(
            &account_id,
            successful_generation,
        ));
        must_ok(
            secrets.write_secret(
                &current_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth("newer-resolution-token", None)
                        .with_expires_unix_seconds(4_000_000_000)
                        .to_secret_string(),
                ),
            ),
        );
    }
    {
        let resolver_state = must_ok(SqliteStateStore::open(&state_path));
        let newer_resolver = RouterCredentialResolver::new(
            &resolver_state,
            &secrets,
            NoopCredentialRefreshClient,
            200,
        );
        must_ok(
            refresh_quota_store_paths_with_dependencies_async(
                &mut Vec::new(),
                &state_path,
                &secret_root,
                format!("http://{address}"),
                &newer_resolver,
                &must_ok(HttpQuotaRefreshProvider::new()),
                200,
            )
            .await,
        );
    }
    let newer_status = must_ok(
        state
            .quota_refresh_statuses_for_route_band("responses")
            .await,
    )
    .into_iter()
    .find(|status| status.account_id() == &account_id)
    .expect("newer successful refresh status must exist");
    assert_eq!(newer_status.last_success_unix_seconds(), Some(200));
    assert_eq!(newer_status.last_attempt_unix_seconds(), Some(200));
    assert_eq!(newer_status.last_error_class(), None);
    assert_eq!(newer_status.stale_after_unix_seconds(), Some(560));
    let newer_credit = must_ok(state.load_account_credit_observation(&account_id).await)
        .expect("newer successful credit observation must exist");
    assert_eq!(newer_credit.credential_generation(), successful_generation);
    assert_eq!(newer_credit.latest_started_attempt(), 2);
    assert_eq!(
        newer_credit.committed_attempt(),
        Some(newer_credit.latest_started_attempt())
    );
    assert!(newer_credit.authorizes_credit_usage(Some(successful_generation), 201));

    release.notify_one();
    let older_result = tokio::time::timeout(Duration::from_secs(5), older_refresh)
        .await
        .expect("released older resolution failure must finish")
        .expect("older refresh task must join");
    assert!(
        matches!(
            older_result,
            Err(crate::quota::QuotaCommandError::ProviderResponse { .. })
        ),
        "the older initial resolver error must make its refresh command fail"
    );
    stop_server.notify_one();
    assert_eq!(provider_server.await.expect("local fixture must join"), 8);
    let final_status = must_ok(
        state
            .quota_refresh_statuses_for_route_band("responses")
            .await,
    )
    .into_iter()
    .find(|status| status.account_id() == &account_id)
    .expect("newer status must remain after delayed older failure");
    let final_credit = must_ok(state.load_account_credit_observation(&account_id).await)
        .expect("newer credit facts must remain after delayed older failure");
    let final_credit_authorizes_usage =
        final_credit.authorizes_credit_usage(Some(successful_generation), 201);
    assert_eq!(
        (final_status, final_credit, final_credit_authorizes_usage),
        (newer_status, newer_credit, true),
        "an initial resolver error from the older cycle must preserve both quota freshness and committed credit authority"
    );
    let inputs = must_ok(state.selector_inputs_for_route_band("responses", 201).await);
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0].account_id(), &exhausted_peer_id);
    assert_eq!(inputs[0].account_status(), AccountStatus::Enabled);
    assert_eq!(inputs[0].active_credential_generation(), Some(1));
    assert_eq!(inputs[0].windows().len(), 2);
    assert!(inputs[0].windows().iter().all(|window| {
        window.status() == SelectorQuotaWindowStatus::Ineligible
            && window.remaining_headroom() == 0
            && window.observed_unix_seconds() == 200
    }));
    let exhausted_credit = must_ok(
        state
            .load_account_credit_observation(&exhausted_peer_id)
            .await,
    )
    .expect("peer exhaustion must be a committed current credit observation");
    assert_eq!(exhausted_credit.credential_generation(), 1);
    assert_eq!(exhausted_credit.latest_started_attempt(), 1);
    assert_eq!(exhausted_credit.committed_attempt(), Some(1));
    assert_eq!(exhausted_credit.observed_unix_seconds(), Some(200));
    assert_eq!(
        exhausted_credit.provider_observation().availability(),
        &codex_router_core::credit_usage::CreditAvailability::Depleted,
    );
    assert!(!exhausted_credit.authorizes_credit_usage(Some(1), 201));
    let selector = repository_selector(&state, 201);
    let selected = must_ok(
        selector
            .select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses"),
                TokenGeneration::new(1),
                None,
            )
            .await,
    );
    assert_eq!(selected.account_id(), &account_id);
    drop(selector);
    assert_preserved_account_routes_through_cli_serve(state_path, secret_root).await;
    must_ok(state.close().await);
}

#[tokio::test]
async fn delayed_initial_resolver_failure_preserves_newer_quota_and_credit_authority() {
    assert_delayed_initial_failure_preserves_newer_authority(1).await;
}

#[tokio::test]
async fn initial_resolver_failure_cannot_seize_successor_generation_authority() {
    assert_delayed_initial_failure_preserves_newer_authority(2).await;
}

async fn start_credit_quota_fixture() -> (
    std::net::SocketAddr,
    Arc<tokio::sync::Notify>,
    tokio::task::JoinHandle<usize>,
) {
    let listener = must_ok(tokio::net::TcpListener::bind("127.0.0.1:0").await);
    let address = must_ok(listener.local_addr());
    let stop_server = Arc::new(tokio::sync::Notify::new());
    let server_stop = Arc::clone(&stop_server);
    let provider_server = tokio::spawn(async move {
        let mut request_count = 0_usize;
        loop {
            tokio::select! {
                incoming = listener.accept() => {
                    let (mut stream, _peer_address) = incoming
                        .expect("local quota fixture should accept");
                    let request = read_http_request(&mut stream).await;
                    let (is_usage_request, _) = is_quota_provider_request(&request);
                    let request_headers = request.to_ascii_lowercase();
                    let exhausted_peer = request_headers.contains(
                        "authorization: bearer exhausted-peer-token\r\n"
                    );
                    assert!(exhausted_peer || request_headers.contains(
                        "authorization: bearer newer-resolution-token\r\n"
                    ), "only valid current file credentials may reach the fixture");
                    request_count += 1;
                    let body = if exhausted_peer && is_usage_request {
                        r#"{
                          "rate_limit": {
                            "primary_window": {"used_percent":100,"reset_at":8000,"limit_window_seconds":18000},
                            "secondary_window": {"used_percent":100,"reset_at":9000,"limit_window_seconds":604800}
                          },
                          "additional_rate_limits":[],
                          "credits":{"has_credits":false,"unlimited":false,"balance":"0"},
                          "spend_control":{"reached":false},
                          "reset_credits":{"available":0}
                        }"#
                    } else if exhausted_peer {
                        r#"{"reset_credits":{"available":0}}"#
                    } else if is_usage_request {
                        r#"{
                          "rate_limit": {
                            "primary_window": {"used_percent":20,"reset_at":8000,"limit_window_seconds":18000},
                            "secondary_window": {"used_percent":30,"reset_at":9000,"limit_window_seconds":604800}
                          },
                          "additional_rate_limits":[],
                          "credits":{"has_credits":true,"unlimited":false,"balance":"3.25"},
                          "spend_control":{"reached":false},
                          "reset_credits":{"available":1}
                        }"#
                    } else {
                        quota_provider_body(false)
                    };
                    write_http_response(&mut stream, body).await;
                }
                () = server_stop.notified() => return request_count,
            }
        }
    });
    (address, stop_server, provider_server)
}

fn repository_selector(
    state: &AsyncSqliteStateStore,
    now_unix_seconds: u64,
) -> AsyncRepositoryBackedAccountSelector<'_, AsyncSqliteStateStore> {
    AsyncRepositoryBackedAccountSelector::new_with_runtime(
        state,
        Default::default(),
        Default::default(),
        0,
        Arc::new(move || now_unix_seconds),
    )
}

#[tokio::test]
async fn newer_initial_resolver_failure_stales_quota_and_suppresses_credit() {
    let test_root = TestRoot::new("quota-refresh-newer-initial-failure");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&state_path).await);
    let account_id = account_id("acct_newer_initial_failure");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "newer-initial-failure",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let (address, stop_server, provider_server) = start_credit_quota_fixture().await;
    let resolver = FixedGenerationCredentialResolver {
        access_token: "newer-resolution-token",
        credential_generation: 1,
    };
    must_ok(
        refresh_quota_store_paths_with_dependencies_async(
            &mut Vec::new(),
            &state_path,
            &test_root.path().join("unused-secrets"),
            format!("http://{address}"),
            &resolver,
            &must_ok(HttpQuotaRefreshProvider::new()),
            200,
        )
        .await,
    );
    let successful_credit = must_ok(state.load_account_credit_observation(&account_id).await)
        .expect("successful credit facts should exist");
    assert_eq!(successful_credit.latest_started_attempt(), 1);
    assert_eq!(successful_credit.committed_attempt(), Some(1));
    assert!(successful_credit.authorizes_credit_usage(Some(1), 201));

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let failure_resolver = GatedInitialFailureCredentialResolver {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    };
    let failure_state_path = state_path.clone();
    let failed_refresh = tokio::spawn(async move {
        refresh_quota_store_paths_with_dependencies_async(
            &mut Vec::new(),
            &failure_state_path,
            &failure_state_path.with_extension("unused-secrets"),
            format!("http://{address}"),
            &failure_resolver,
            &must_ok(HttpQuotaRefreshProvider::new()),
            300,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("newer attempt must enter initial credential resolution");
    let pending_credit = must_ok(state.load_account_credit_observation(&account_id).await)
        .expect("pending attempt should retain cached facts");
    assert_eq!(pending_credit.latest_started_attempt(), 2);
    assert_eq!(pending_credit.committed_attempt(), Some(1));
    assert_eq!(pending_credit.observed_unix_seconds(), Some(200));
    assert!(!pending_credit.authorizes_credit_usage(Some(1), 301));
    release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(5), failed_refresh)
        .await
        .expect("released newer failure must finish")
        .expect("failed refresh must join");
    assert!(matches!(
        result,
        Err(crate::quota::QuotaCommandError::ProviderResponse { .. })
    ));
    stop_server.notify_one();
    assert_eq!(provider_server.await.expect("fixture must join"), 4);

    let status = must_ok(
        state
            .quota_refresh_statuses_for_route_band("responses")
            .await,
    )
    .into_iter()
    .find(|status| status.account_id() == &account_id)
    .expect("failure status");
    assert_eq!(status.last_success_unix_seconds(), Some(200));
    assert_eq!(status.last_attempt_unix_seconds(), Some(300));
    assert_eq!(
        status.last_error_class(),
        Some(codex_router_state::quota_snapshot::QuotaRefreshErrorClass::AuthError)
    );
    assert_eq!(status.stale_after_unix_seconds(), Some(300));
    assert_eq!(
        must_ok(state.load_account_credit_observation(&account_id).await),
        Some(pending_credit)
    );
    let inputs = must_ok(state.selector_inputs_for_route_band("responses", 301).await);
    let input = inputs
        .iter()
        .find(|input| input.account_id() == &account_id)
        .expect("the failed account must remain available for typed assessment");
    assert_eq!(input.windows().len(), 2);
    assert!(
        input
            .windows()
            .iter()
            .all(|window| window.status() == SelectorQuotaWindowStatus::Stale)
    );
    assert!(
        !input
            .credit_observation()
            .expect("cached credit facts remain visible")
            .authorizes_credit_usage(Some(1), 301)
    );
    must_ok(state.close().await);
}

struct GenerationReplacingInitialSuccessResolver {
    state_path: PathBuf,
    returned_generation: u64,
}

impl AsyncProviderCredentialResolver for GenerationReplacingInitialSuccessResolver {
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        _provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        let state = must_ok(AsyncSqliteStateStore::open(&self.state_path).await);
        must_ok(
            state
                .activate_account_credential_generation_and_invalidate_quota(
                    account_id,
                    2,
                    AccountStatus::Enabled,
                )
                .await,
        );
        must_ok(state.close().await);
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            SecretString::new("newer-resolution-token"),
            self.returned_generation,
        ))
    }
}

async fn assert_initial_success_generation_handling(returned_generation: u64) {
    let test_root = TestRoot::new("quota-refresh-initial-success-generation");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&state_path).await);
    let account_id = account_id("acct_initial_success_generation");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "initial-success-generation",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let (address, stop_server, provider_server) = start_credit_quota_fixture().await;
    let resolver = GenerationReplacingInitialSuccessResolver {
        state_path: state_path.clone(),
        returned_generation,
    };
    let result = refresh_quota_store_paths_with_dependencies_async(
        &mut Vec::new(),
        &state_path,
        &test_root.path().join("unused-secrets"),
        format!("http://{address}"),
        &resolver,
        &must_ok(HttpQuotaRefreshProvider::new()),
        200,
    )
    .await;
    stop_server.notify_one();
    let request_count = provider_server.await.expect("fixture must join");
    let credit = must_ok(state.load_account_credit_observation(&account_id).await)
        .expect("initial attempt marker must exist");
    assert_eq!(credit.credential_generation(), 2);
    if returned_generation == 2 {
        must_ok(result);
        assert_eq!(request_count, 4);
        assert_eq!(credit.latest_started_attempt(), 2);
        assert_eq!(credit.committed_attempt(), Some(2));
        assert!(credit.authorizes_credit_usage(Some(2), 201));
    } else {
        assert!(matches!(
            result,
            Err(crate::quota::QuotaCommandError::ProviderResponse { .. })
        ));
        assert_eq!(
            request_count, 0,
            "a stale returned generation must not begin quota I/O"
        );
        assert_eq!(credit.latest_started_attempt(), 1);
        assert_eq!(credit.committed_attempt(), None);
        assert!(!credit.authorizes_credit_usage(Some(2), 201));
    }
    must_ok(state.close().await);
}

#[tokio::test]
async fn renewed_generation_from_initial_resolution_gets_its_checked_attempt() {
    assert_initial_success_generation_handling(2).await;
}

#[tokio::test]
async fn stale_generation_returned_from_initial_resolution_skips_quota_io() {
    assert_initial_success_generation_handling(1).await;
}
