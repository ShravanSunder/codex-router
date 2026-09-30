use super::*;

struct FirstUnauthorizedQuotaProvider {
    seen_tokens: Mutex<Vec<String>>,
    reject_retry: bool,
}

impl QuotaRefreshProvider for FirstUnauthorizedQuotaProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        let mut tokens = lock_test_mutex(&self.seen_tokens, "quota 401 token record");
        tokens.push(request.access_token().expose_secret().to_owned());
        let call = tokens.len();
        drop(tokens);
        if call == 1 || (self.reject_retry && call == 2) {
            return Err(crate::quota::QuotaCommandError::ProviderStatus { status: 401 });
        }
        Ok(QuotaRefreshProviderResponse {
            windows: vec![QuotaRefreshProviderWindow {
                limit_window_seconds: 18_000,
                remaining_headroom: 42,
                reset_unix_seconds: Some(2_000),
                effective: true,
            }],
            reset_credits_available: None,
        })
    }
}

#[test]
fn quota_401_renews_once_and_retries_with_the_committed_generation() {
    for (reject_retry, known_expiry) in [(false, true), (true, true), (false, false), (true, false)]
    {
        let test_root = TestRoot::new(if reject_retry {
            "quota-401-second-reject"
        } else {
            "quota-401-recovered"
        });
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let account_id = account_id("quota-401-account");
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                "quota-401",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        ));
        let secrets = must_ok(
            codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
        );
        let key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
        let bundle = AccountCredentialBundle::imported_codex_auth(
            "rejected-access-canary",
            Some("renewable-refresh-canary".to_owned()),
        );
        let bundle = if known_expiry {
            bundle.with_expires_unix_seconds(4_000_000_000)
        } else {
            bundle
        };
        must_ok(secrets.write_secret(&key, &must_ok(bundle.to_secret_string())));
        let refresh_client = RecordingRefreshClient::new(
            "quota-401-account",
            "renewable-refresh-canary",
            AccountCredentialBundle::imported_codex_auth(
                "recovered-access-canary",
                Some("rotated-refresh-canary".to_owned()),
            )
            .with_expires_unix_seconds(5_000_000_000),
        );
        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &state_path,
            &secret_root,
            refresh_client.clone(),
        ));
        let provider = FirstUnauthorizedQuotaProvider {
            seen_tokens: Mutex::new(Vec::new()),
            reject_retry,
        };
        let mut output = Vec::new();
        let _result = refresh_quota_store_paths_with_dependencies(
            &mut output,
            &state_path,
            &secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_100,
        );
        assert_eq!(refresh_client.calls(), 1);
        let recorded = lock_test_mutex(&provider.seen_tokens, "quota 401 token record");
        assert_eq!(recorded[0], "rejected-access-canary");
        assert_eq!(recorded[1], "recovered-access-canary");
        if !reject_retry {
            assert_eq!(recorded[2], "recovered-access-canary");
        }
        let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
            .expect("account should remain");
        assert_eq!(account.active_credential_generation(), Some(2));
        assert_eq!(
            account.status(),
            if reject_retry {
                AccountStatus::Disabled
            } else {
                AccountStatus::Enabled
            }
        );
    }
}

struct ConcurrentGenerationQuotaProvider {
    state_path: PathBuf,
    secret_root: PathBuf,
    account_id: AccountId,
    seen_tokens: Mutex<Vec<String>>,
}

impl QuotaRefreshProvider for ConcurrentGenerationQuotaProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        let call = {
            let mut seen_tokens = lock_test_mutex(&self.seen_tokens, "quota generation race");
            seen_tokens.push(request.access_token().expose_secret().to_owned());
            seen_tokens.len()
        };
        if call == 1 {
            let secrets = must_ok(
                codex_router_secret_store::test_support::open_encrypted_credential_store(
                    &self.secret_root,
                ),
            );
            let successor_key = must_ok(openai_account_credential_bundle_key(&self.account_id, 2));
            let successor = AccountCredentialBundle::imported_codex_auth(
                "concurrent-access-canary",
                Some("concurrent-refresh-canary".to_owned()),
            )
            .with_expires_unix_seconds(5_000_000_000);
            must_ok(secrets.write_secret(&successor_key, &must_ok(successor.to_secret_string())));
            let state = must_ok(SqliteStateStore::open(&self.state_path));
            must_ok(AccountStateRepository::upsert_account(
                &state,
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    self.account_id.clone(),
                    "quota-race",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(2),
            ));
            return Err(crate::quota::QuotaCommandError::ProviderStatus { status: 401 });
        }
        Ok(QuotaRefreshProviderResponse {
            windows: vec![QuotaRefreshProviderWindow {
                limit_window_seconds: 18_000,
                remaining_headroom: 42,
                reset_unix_seconds: Some(2_000),
                effective: true,
            }],
            reset_credits_available: None,
        })
    }
}

#[test]
fn quota_401_reuses_a_concurrently_committed_generation_without_oauth_refresh() {
    let test_root = TestRoot::new("quota-401-generation-race");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("quota-401-race-account");
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "quota-race",
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
    let refresh_client = RecordingRefreshClient::new(
        "quota-401-race-account",
        "original-refresh-canary",
        AccountCredentialBundle::imported_codex_auth("unexpected-refresh", None),
    );
    let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
        &state_path,
        &secret_root,
        refresh_client.clone(),
    ));
    let provider = ConcurrentGenerationQuotaProvider {
        state_path: state_path.clone(),
        secret_root: secret_root.clone(),
        account_id: account_id.clone(),
        seen_tokens: Mutex::new(Vec::new()),
    };
    let mut output = Vec::new();
    must_ok(refresh_quota_store_paths_with_dependencies(
        &mut output,
        &state_path,
        &secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        1_100,
    ));
    assert_eq!(refresh_client.calls(), 0);
    let seen_tokens = lock_test_mutex(&provider.seen_tokens, "quota generation race");
    assert_eq!(seen_tokens[0], "rejected-access-canary");
    assert!(
        seen_tokens[1..]
            .iter()
            .all(|token| token == "concurrent-access-canary")
    );
    let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .expect("account should remain");
    assert_eq!(account.active_credential_generation(), Some(2));
    assert_eq!(account.status(), AccountStatus::Enabled);
}

#[test]
fn quota_401_does_not_bypass_terminal_or_retry_cooldown_maintenance() {
    use codex_router_state::credential_maintenance::CredentialMaintenanceState;

    for (scenario, expected_state) in [
        ("terminal", CredentialMaintenanceState::Unrefreshable),
        ("cooldown", CredentialMaintenanceState::Retrying),
    ] {
        let test_root = TestRoot::new(&format!("quota-401-{scenario}"));
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let account_id = account_id("quota-401-maintenance-account");
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                "maintenance",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        ));
        let secrets = must_ok(
            codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
        );
        let key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
        let bundle = AccountCredentialBundle::imported_codex_auth(
            "rejected-access-canary",
            Some("refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(5_000_000_000);
        must_ok(secrets.write_secret(&key, &must_ok(bundle.to_secret_string())));
        let async_state =
            must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(&state_path)));
        let changed =
            if scenario == "terminal" {
                must_ok(
                    test_async_runtime()
                        .block_on(async_state.mark_credential_unrefreshable(&account_id, 1)),
                )
            } else {
                must_ok(test_async_runtime().block_on(
                    async_state.record_pre_provider_local_failure(&account_id, 1, 4_000_000_000),
                ))
            };
        assert!(changed);
        let refresh_client = RecordingRefreshClient::new(
            "quota-401-maintenance-account",
            "refresh-canary",
            AccountCredentialBundle::imported_codex_auth("unexpected-refresh", None),
        );
        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &state_path,
            &secret_root,
            refresh_client.clone(),
        ));
        let provider = FirstUnauthorizedQuotaProvider {
            seen_tokens: Mutex::new(Vec::new()),
            reject_retry: false,
        };
        let mut output = Vec::new();
        let _result = refresh_quota_store_paths_with_dependencies(
            &mut output,
            &state_path,
            &secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_100,
        );
        assert_eq!(refresh_client.calls(), 0, "{scenario}");
        assert_eq!(
            lock_test_mutex(&provider.seen_tokens, "quota maintenance provider").as_slice(),
            ["rejected-access-canary"],
            "{scenario}"
        );
        let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
            .expect("account should remain");
        assert_eq!(account.status(), AccountStatus::Enabled, "{scenario}");
        assert_eq!(
            account.active_credential_generation(),
            Some(1),
            "{scenario}"
        );
        let maintenance = must_ok(
            test_async_runtime().block_on(async_state.load_credential_maintenance(&account_id)),
        )
        .expect("maintenance should remain");
        assert_eq!(maintenance.state, expected_state, "{scenario}");
    }
}

#[test]
fn quota_refresh_rejects_non_provider_base_url_before_token_egress() {
    let test_root = TestRoot::new("quota-refresh-disallowed");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_refresh_reject");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "reject",
        AccountStatus::Enabled,
    );
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    let access_key = must_ok(upstream_access_token_key(&account_id));
    must_ok(secrets.write_secret(
        &access_key,
        &SecretString::new("quota-refresh-token-canary"),
    ));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = match test_async_runtime().block_on(run_with_io_async(
        vec![
            "codex-router".into(),
            "quota".into(),
            "refresh".into(),
            "--router-root".into(),
            router_root.as_os_str().to_owned(),
            "--base-url".into(),
            "http://attacker.example".into(),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    )) {
        Ok(()) => panic!("disallowed quota base URL must fail before token egress"),
        Err(error) => error,
    };
    let rendered_error = error.to_string();

    assert!(rendered_error.contains("quota refresh base URL is not allowed"));
    assert!(!rendered_error.contains("quota-refresh-token-canary"));
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn quota_refresh_resolver_refreshes_expired_access_token_before_provider_egress() {
    let test_root = TestRoot::new("quota-refresh-resolver");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_quota_refresh");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "refresh",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    let expired_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &expired_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-quota-access-token",
                    Some("quota-refresh-token".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let refresh_client = RecordingRefreshClient::new(
        "acct_quota_refresh",
        "quota-refresh-token",
        AccountCredentialBundle::imported_codex_auth(
            "refreshed-quota-access-token",
            Some("refreshed-quota-refresh-token".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    );
    let resolver = RouterCredentialResolver::new(&state, &secrets, refresh_client.clone(), 1_000);
    let provider = RecordingQuotaRefreshProvider::new(33);
    let mut stdout = Vec::new();

    must_ok(refresh_quota_with_dependencies(
        &mut stdout,
        router_root.clone(),
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        1_100,
    ));

    assert_eq!(refresh_client.calls(), 1);
    let recorded = provider.take_recorded();
    assert_eq!(
        recorded,
        vec![
            (
                "acct_quota_refresh".to_owned(),
                "refresh".to_owned(),
                "responses".to_owned(),
                "https://chatgpt.com/backend-api".to_owned(),
                "refreshed-quota-access-token".to_owned(),
            ),
            (
                "acct_quota_refresh".to_owned(),
                "refresh".to_owned(),
                "models".to_owned(),
                "https://chatgpt.com/backend-api".to_owned(),
                "refreshed-quota-access-token".to_owned(),
            )
        ]
    );
    let refreshed_snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
        &state,
        &account_id,
        "responses",
    ))
    .unwrap_or_else(|| panic!("quota snapshot should be persisted"));
    assert_eq!(refreshed_snapshot.remaining_headroom(), 33);
    assert_eq!(
        refreshed_snapshot.source(),
        QuotaSnapshotSource::OpenAiEndpoint
    );
    assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");
    let runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    runtime.block_on(async {
        let async_state =
            must_ok(AsyncSqliteStateStore::open(&router_root.join("state.sqlite")).await);
        let history = must_ok(
            async_state
                .quota_history_observations_for_window(&account_id, "responses", 18_000, 0, 2_000)
                .await,
        );
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].account_label(), "refresh");
        assert_eq!(history[0].remaining_headroom(), 33);
        assert_eq!(history[0].reset_credits_available(), None);
        assert_eq!(history[0].observed_unix_seconds(), 1_100);
    });
}

#[test]
fn quota_refresh_store_paths_uses_current_thread_runtime_without_nested_runtime() {
    let test_root = TestRoot::new("quota-refresh-store-paths");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("custom-state.sqlite");
    let secret_root = test_root.path().join("custom-secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("acct_quota_store_paths");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "store-paths",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &bundle_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "store-path-access-token",
                    Some("store-path-refresh-token".to_owned()),
                )
                .to_secret_string(),
            ),
        ),
    );
    let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
        &state_path,
        &secret_root,
        NoopCredentialRefreshClient,
    ));
    let provider = RecordingQuotaRefreshProvider::new(61);
    let mut stdout = Vec::new();

    must_ok(refresh_quota_store_paths_with_dependencies(
        &mut stdout,
        &state_path,
        &secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        1_200,
    ));

    let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
        &state,
        &account_id,
        "responses",
    ))
    .unwrap_or_else(|| panic!("explicit state-db snapshot should be persisted"));
    assert_eq!(snapshot.remaining_headroom(), 61);
    assert_eq!(snapshot.observed_unix_seconds(), 1_200);
    assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");
}

#[test]
fn quota_refresh_missing_refresh_token_fails_closed_before_provider_egress() {
    let test_root = TestRoot::new("quota-refresh-missing-refresh");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_quota_missing_refresh");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "missing",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    let expired_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &expired_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-quota-access-token-canary",
                    None,
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    let provider = RecordingQuotaRefreshProvider::new(44);
    let mut stdout = Vec::new();

    let error = match refresh_quota_with_dependencies(
        &mut stdout,
        router_root,
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        1_100,
    ) {
        Ok(()) => panic!("missing refresh token should fail before provider egress"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "quota refresh provider response was unusable: quota refresh failed for all eligible route bands"
    );
    assert!(provider.take_recorded().is_empty());
    let rendered_stdout = must_ok(String::from_utf8(stdout));
    assert!(rendered_stdout.contains(&format!(
        "refresh failed: account=missing route_band=* error={}\n",
        CredentialResolverError::RefreshUnavailable
    )));
    assert!(rendered_stdout.contains("refreshed: 0\n"));
    assert!(rendered_stdout.contains("failed: 2\n"));
    assert!(!rendered_stdout.contains("expired-quota-access-token-canary"));
}

#[test]
fn quota_refresh_continues_after_one_account_provider_failure() {
    let test_root = TestRoot::new("quota-refresh-partial-provider-failure");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let failing_account_id = account_id("acct_quota_provider_failing");
    let healthy_account_id = account_id("acct_quota_provider_healthy");
    let failing_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        failing_account_id.clone(),
        "failing",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    let healthy_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        healthy_account_id.clone(),
        "healthy",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &failing_account,
    ));
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &healthy_account,
    ));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    for (account_id, access_token) in [
        (&failing_account_id, "failing-provider-token-canary"),
        (&healthy_account_id, "healthy-provider-token-canary"),
    ] {
        let bundle_key = must_ok(openai_account_credential_bundle_key(account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        access_token,
                        Some(format!("{access_token}-refresh")),
                    )
                    .with_expires_unix_seconds(2_000)
                    .to_secret_string(),
                ),
            ),
        );
    }
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    let provider = AccountFailingQuotaRefreshProvider::new("failing", 429, 69);
    let mut stdout = Vec::new();

    must_ok(refresh_quota_with_dependencies(
        &mut stdout,
        router_root,
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        1_100,
    ));

    assert!(
        must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &failing_account_id,
            "responses",
        ))
        .is_none()
    );
    let healthy_snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
        &state,
        &healthy_account_id,
        "responses",
    ))
    .unwrap_or_else(|| panic!("healthy account quota snapshot should be persisted"));
    assert_eq!(healthy_snapshot.remaining_headroom(), 69);
    let rendered_stdout = must_ok(String::from_utf8(stdout));
    assert!(rendered_stdout.contains(
            "refresh failed: account=failing route_band=responses error=quota refresh provider returned HTTP 429\n"
        ));
    assert!(rendered_stdout.contains(
            "refresh failed: account=failing route_band=models error=quota refresh provider returned HTTP 429\n"
        ));
    assert!(rendered_stdout.contains("refreshed: 2\n"));
    assert!(rendered_stdout.contains("failed: 2\n"));
    assert!(!rendered_stdout.contains("failing-provider-token-canary"));
    assert!(!rendered_stdout.contains("healthy-provider-token-canary"));
}

#[test]
fn quota_refresh_preserves_enabled_account_when_401_recovery_is_unavailable() {
    for (provider_status, expected_account_status) in
        [(401, AccountStatus::Enabled), (403, AccountStatus::Enabled)]
    {
        let test_root = TestRoot::new(&format!(
            "quota-refresh-provider-auth-rejection-{provider_status}"
        ));
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let rejected_account_id = account_id("acct_quota_provider_rejected");
        let healthy_account_id = account_id("acct_quota_provider_healthy");
        for (account_id, account_label) in [
            (&rejected_account_id, "rejected"),
            (&healthy_account_id, "healthy"),
        ] {
            let account = AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                account_label,
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1);
            must_ok(AccountStateRepository::upsert_account(&state, &account));
        }
        let secrets = must_ok(
            codex_router_secret_store::test_support::open_encrypted_credential_store(
                router_root.join("secrets"),
            ),
        );
        for (account_id, access_token) in [
            (&rejected_account_id, "rejected-provider-token-canary"),
            (&healthy_account_id, "healthy-provider-token-canary"),
        ] {
            let bundle_key = must_ok(openai_account_credential_bundle_key(account_id, 1));
            must_ok(
                secrets.write_secret(
                    &bundle_key,
                    &must_ok(
                        AccountCredentialBundle::imported_codex_auth(
                            access_token,
                            Some(format!("{access_token}-refresh")),
                        )
                        .with_expires_unix_seconds(2_000)
                        .to_secret_string(),
                    ),
                ),
            );
        }
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let provider = AccountFailingQuotaRefreshProvider::new("rejected", provider_status, 73);
        let mut stdout = Vec::new();

        must_ok(refresh_quota_with_dependencies(
            &mut stdout,
            router_root,
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_100,
        ));

        let rejected_account = must_ok(AccountStateRepository::load_account(
            &state,
            &rejected_account_id,
        ))
        .unwrap_or_else(|| panic!("rejected account should remain registered"));
        assert_eq!(rejected_account.status(), expected_account_status);
        let healthy_account = must_ok(AccountStateRepository::load_account(
            &state,
            &healthy_account_id,
        ))
        .unwrap_or_else(|| panic!("healthy account should remain registered"));
        assert_eq!(healthy_account.status(), AccountStatus::Enabled);
        let healthy_snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &healthy_account_id,
            "responses",
        ))
        .unwrap_or_else(|| panic!("healthy account should still refresh"));
        assert_eq!(healthy_snapshot.remaining_headroom(), 73);
        let selector_inputs = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
            &state,
            "responses",
            1_100,
        ));
        let rejected_selector_input = selector_inputs
            .iter()
            .find(|input| input.account_id() == &rejected_account_id)
            .unwrap_or_else(|| panic!("rejected account should remain in selector state"));
        assert_eq!(
            rejected_selector_input.account_status(),
            expected_account_status
        );
        let healthy_selector_input = selector_inputs
            .iter()
            .find(|input| input.account_id() == &healthy_account_id)
            .unwrap_or_else(|| panic!("healthy account should remain in selector state"));
        assert_eq!(
            healthy_selector_input.account_status(),
            AccountStatus::Enabled
        );
    }
}
