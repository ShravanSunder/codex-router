use super::*;

#[test]
fn quota_refresh_rejects_non_provider_base_url_before_token_egress() {
    let test_root = TestRoot::new("quota-refresh-disallowed");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_refresh_reject");
    let account = AccountRecord::new(account_id.clone(), "reject", AccountStatus::Enabled);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
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
    let account = AccountRecord::new(account_id.clone(), "refresh", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let expired_key = must_ok(account_credential_bundle_key(&account_id, 1));
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
    let account = AccountRecord::new(account_id.clone(), "store-paths", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
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
        1_000,
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
    let account = AccountRecord::new(account_id.clone(), "missing", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let expired_key = must_ok(account_credential_bundle_key(&account_id, 1));
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
        failing_account_id.clone(),
        "failing",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    let healthy_account = AccountRecord::new(
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
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    for (account_id, access_token) in [
        (&failing_account_id, "failing-provider-token-canary"),
        (&healthy_account_id, "healthy-provider-token-canary"),
    ] {
        let bundle_key = must_ok(account_credential_bundle_key(account_id, 1));
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
fn quota_refresh_excludes_only_account_rejected_with_http_401() {
    for (provider_status, expected_account_status) in [
        (401, AccountStatus::Disabled),
        (403, AccountStatus::Enabled),
    ] {
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
            let account =
                AccountRecord::new(account_id.clone(), account_label, AccountStatus::Enabled)
                    .with_active_credential_generation(1);
            must_ok(AccountStateRepository::upsert_account(&state, &account));
        }
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        for (account_id, access_token) in [
            (&rejected_account_id, "rejected-provider-token-canary"),
            (&healthy_account_id, "healthy-provider-token-canary"),
        ] {
            let bundle_key = must_ok(account_credential_bundle_key(account_id, 1));
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
