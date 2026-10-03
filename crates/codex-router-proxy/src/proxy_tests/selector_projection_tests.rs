use super::*;

#[test]
fn quota_aware_selector_prefers_fresh_headroom_over_penalized_stale_account() {
    let fresh_account = quota_account(
        "acct_fresh",
        50,
        SnapshotFreshness::Fresh { age_seconds: 10 },
    );
    let stale_account = quota_account(
        "acct_stale",
        100,
        SnapshotFreshness::StaleWithPenalty { age_seconds: 600 },
    );
    let selector = QuotaAwareAccountSelector::new(vec![stale_account, fresh_account]);

    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("selector should choose eligible account: {error}"),
    };

    assert_eq!(selected.account_id().as_str(), "acct_fresh");
    assert_eq!(selected.selection_reason(), "preferred_weekly_reset_soon");
}

#[test]
fn quota_aware_selector_fails_closed_when_no_account_has_headroom() {
    let selector = QuotaAwareAccountSelector::new(vec![quota_account(
        "acct_empty",
        0,
        SnapshotFreshness::Fresh { age_seconds: 10 },
    )]);

    assert_eq!(
        selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ),
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
        })
    );
}

#[test]
fn repository_backed_selector_hydrates_enabled_accounts_from_state_quota_and_secret_store() {
    let temp_dir = ProxyTestTempDir::new("repository_selector");
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
    let disabled = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_disabled"),
        "disabled",
        AccountStatus::Disabled,
    );

    persist_account_with_snapshot_and_token(&state, &secrets, &beta, 40, "beta-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &alpha, 80, "alpha-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &disabled, 500, "disabled-token");

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("repository-backed selector should select account: {error}"),
    };

    assert_eq!(selected.account_id().as_str(), "acct_alpha");
    assert_eq!(selected.selection_reason(), "preferred_weekly_healthier");
}

#[test]
fn repository_backed_selector_uses_route_specific_quota_snapshots() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_route_band");
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
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_route_specific"),
        "route-specific",
        AccountStatus::Enabled,
    );
    if let Err(error) = AccountStateRepository::upsert_account(
        &state,
        &account.clone().with_active_credential_generation(1),
    ) {
        panic!("account should persist: {error}");
    }
    let responses_snapshot = PersistedQuotaSnapshot::new(
        account.account_id().clone(),
        QuotaSnapshotSource::MockEndpoint,
    )
    .with_observed_unix_seconds(1_000)
    .with_route_band("responses", 0)
    .with_stale_penalty(false);
    let models_snapshot = PersistedQuotaSnapshot::new(
        account.account_id().clone(),
        QuotaSnapshotSource::MockEndpoint,
    )
    .with_observed_unix_seconds(1_000)
    .with_route_band("models", 10)
    .with_stale_penalty(false);
    if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&state, &responses_snapshot) {
        panic!("responses quota should persist: {error}");
    }
    if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&state, &models_snapshot) {
        panic!("models quota should persist: {error}");
    }
    let responses_window = PersistedSelectorQuotaWindow::new(
        account.account_id().clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Ineligible,
    )
    .with_remaining_headroom(0)
    .with_effective(true)
    .with_observed_unix_seconds(test_unix_seconds())
    .with_reset_unix_seconds(selector_reset_seconds(18_000));
    let responses_weekly_window = PersistedSelectorQuotaWindow::new(
        account.account_id().clone(),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Ineligible,
    )
    .with_remaining_headroom(0)
    .with_effective(false)
    .with_observed_unix_seconds(test_unix_seconds())
    .with_reset_unix_seconds(selector_reset_seconds(604_800));
    let models_window = PersistedSelectorQuotaWindow::new(
        account.account_id().clone(),
        "models",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(10)
    .with_effective(true)
    .with_observed_unix_seconds(test_unix_seconds())
    .with_reset_unix_seconds(selector_reset_seconds(18_000));
    let models_weekly_window = PersistedSelectorQuotaWindow::new(
        account.account_id().clone(),
        "models",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(10)
    .with_effective(false)
    .with_observed_unix_seconds(test_unix_seconds())
    .with_reset_unix_seconds(selector_reset_seconds(604_800));
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&state, &responses_window) {
        panic!("responses selector window should persist: {error}");
    }
    if let Err(error) =
        SelectorQuotaRepository::upsert_selector_window(&state, &responses_weekly_window)
    {
        panic!("responses weekly selector window should persist: {error}");
    }
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&state, &models_window) {
        panic!("models selector window should persist: {error}");
    }
    if let Err(error) =
        SelectorQuotaRepository::upsert_selector_window(&state, &models_weekly_window)
    {
        panic!("models weekly selector window should persist: {error}");
    }
    let token_key = match openai_account_credential_bundle_key(account.account_id(), 1) {
        Ok(token_key) => token_key,
        Err(error) => panic!("token key should build: {error}"),
    };
    let bundle = match AccountCredentialBundle::imported_codex_auth(
        "route-specific-token",
        Some("route-specific-refresh-token".to_owned()),
    )
    .to_secret_string()
    {
        Ok(bundle) => bundle,
        Err(error) => panic!("credential bundle should serialize: {error}"),
    };
    if let Err(error) = secrets.write_secret(&token_key, &bundle) {
        panic!("upstream token should persist: {error}");
    }

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Get, "/v1/models"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("models request should select from models quota: {error}"),
    };
    assert_eq!(selected.account_id(), account.account_id());

    assert_eq!(
        selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ),
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
        })
    );
}

#[tokio::test]
async fn async_repository_backed_selector_uses_route_specific_quota_snapshots() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_route_band");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_route_specific"),
        "route-specific",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_status_specs(
        &state,
        &account,
        "responses",
        &[
            (18_000, 0, true, SelectorQuotaWindowStatus::Ineligible),
            (604_800, 0, false, SelectorQuotaWindowStatus::Ineligible),
        ],
    );
    persist_account_with_selector_window_specs(
        &state,
        &account,
        "models",
        &[(18_000, 10, true), (604_800, 10, false)],
    );
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    let selector = AsyncRepositoryBackedAccountSelector::new(&async_state);
    let selected = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Get, "/v1/models"),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("models request should select from models quota: {error}"),
    };
    assert_eq!(selected.account_id(), account.account_id());

    assert_eq!(
        selector
            .select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses"),
                TokenGeneration::new(1),
                None,
            )
            .await,
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
        })
    );
}

#[tokio::test]
async fn async_repository_selector_reports_stale_authority_as_unavailable() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_stale_authority");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path)
        .unwrap_or_else(|error| panic!("state store should open: {error}"));
    let stale_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_async_stale_authority"),
        "async-stale-authority",
        AccountStatus::Enabled,
    );
    let exhausted_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_async_fresh_exhausted"),
        "async-fresh-exhausted",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_status_specs(
        &state,
        &stale_account,
        "responses",
        &[
            (18_000, 90, true, SelectorQuotaWindowStatus::Stale),
            (604_800, 90, false, SelectorQuotaWindowStatus::Stale),
        ],
    );
    persist_account_with_selector_window_status_specs(
        &state,
        &exhausted_account,
        "responses",
        &[
            (18_000, 0, true, SelectorQuotaWindowStatus::Ineligible),
            (604_800, 0, false, SelectorQuotaWindowStatus::Ineligible),
        ],
    );
    drop(state);
    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("weekly-floor store should open: {error}"));
    mutation
        .set_weekly_quota_floor_by_label(
            stale_account.label(),
            Some(
                WeeklyQuotaFloorBasisPoints::new(500)
                    .unwrap_or_else(|error| panic!("weekly floor should validate: {error}")),
            ),
        )
        .await
        .unwrap_or_else(|error| panic!("weekly floor should persist: {error}"));
    mutation.close().await;
    let async_state = AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("async state store should open: {error}"));
    let selector = AsyncRepositoryBackedAccountSelector::new(&async_state);

    let error = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        )
        .await
        .expect_err("stale quota authority must not select an account");

    assert_eq!(
        error,
        HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::StateUnavailable,
        }
    );
}

#[tokio::test]
async fn async_repository_backed_selector_prefers_slower_projected_burn_history() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_history_burn");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let fast_burn = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_fast_burn"),
        "fast-burn",
        AccountStatus::Enabled,
    );
    let slow_burn = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_slow_burn"),
        "slow-burn",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_reset_specs(
        &state,
        &fast_burn,
        "responses",
        &[
            (18_000, 70, true, 9_900 + 18_000),
            (604_800, 70, false, 9_900 + 604_800),
        ],
    );
    persist_account_with_selector_window_reset_specs(
        &state,
        &slow_burn,
        "responses",
        &[
            (18_000, 70, true, 9_900 + 18_000),
            (604_800, 70, false, 9_900 + 604_800),
        ],
    );
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    append_history_series_with_reset(
        &async_state,
        fast_burn.account_id(),
        "responses",
        18_000,
        9_900 + 18_000,
        &[(9_000, 90), (9_450, 80), (9_900, 70)],
    )
    .await;
    append_history_series_with_reset(
        &async_state,
        slow_burn.account_id(),
        "responses",
        18_000,
        9_900 + 18_000,
        &[(9_000, 72), (9_450, 71), (9_900, 70)],
    )
    .await;

    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
        &async_state,
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(HashMap::new())),
        0,
        Arc::new(|| 9_900),
    );
    let selected = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("selector should choose slower projected burn: {error}"),
    };

    assert_eq!(selected.account_id(), slow_burn.account_id());
}
