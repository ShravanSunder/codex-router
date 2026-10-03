use super::*;

#[test]
fn repository_backed_selector_reuses_held_account_inside_cooldown() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_hold_cooldown");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
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
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &beta,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );

    let now = Arc::new(Mutex::new(test_unix_seconds()));
    let clock_now = Arc::clone(&now);
    let selector = RepositoryBackedAccountSelector::new_with_runtime(
        &state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        120,
        Arc::new(move || *lock_test_mutex(&clock_now, "test clock")),
    );

    let first = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("first request should select account: {error}"),
    };
    let second = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("second request should reuse held account: {error}"),
    };

    assert_eq!(first.account_id(), alpha.account_id());
    assert_eq!(first.selection_reason(), "preferred_safest_quota");
    assert_eq!(second.account_id(), alpha.account_id());
    assert_eq!(second.selection_reason(), "account_hold_cooldown");
}

#[test]
fn repository_backed_selector_keeps_preferring_healthiest_quota_account() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_prefers_healthiest_quota");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let askluna = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_askluna"),
        "askluna",
        AccountStatus::Enabled,
    );
    let matches = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_matches"),
        "matches",
        AccountStatus::Enabled,
    );
    let ssdev = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_ssdev"),
        "ssdev",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &askluna,
        "responses",
        &[(18_000, 98, true), (604_800, 23, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &matches,
        "responses",
        &[(18_000, 99, true), (604_800, 34, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &ssdev,
        "responses",
        &[(18_000, 78, true), (604_800, 76, false)],
    );

    let selector = RepositoryBackedAccountSelector::new_with_runtime(
        &state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        0,
        Arc::new(test_unix_seconds),
    );

    for request_index in 0..80 {
        let selected = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("request {request_index} should select account: {error}"),
        };
        assert_eq!(
            selected.account_id(),
            ssdev.account_id(),
            "request {request_index} should not spend weak weekly accounts while ssdev is healthiest",
        );
        assert_eq!(selected.selection_reason(), "preferred_weekly_healthier");
    }
}

#[tokio::test]
async fn async_repository_backed_selector_reuses_held_account_inside_cooldown() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_hold_cooldown");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
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
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &beta,
        "responses",
        &[(18_000, 50, true), (604_800, 50, false)],
    );
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    let now = Arc::new(Mutex::new(test_unix_seconds()));
    let clock_now = Arc::clone(&now);
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        120,
        Arc::new(move || *lock_test_mutex(&clock_now, "test clock")),
    );

    let first = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("first request should select account: {error}"),
    };
    let second = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("second request should reuse held account: {error}"),
    };

    assert_eq!(first.account_id(), alpha.account_id());
    assert_eq!(first.selection_reason(), "preferred_weekly_healthier");
    assert_eq!(second.account_id(), alpha.account_id());
    assert_eq!(second.selection_reason(), "account_hold_cooldown");
}

#[tokio::test]
async fn async_repository_backed_selector_breaks_hold_when_active_load_changes_next_choice() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_hold_active_load_break");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
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
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &beta,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_and_reservations(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        RouteBandReservationBooks::default(),
        120,
        Arc::new(test_unix_seconds),
    );

    let first = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses").with_websocket_upgrade(true),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("first request should select held account: {error}"),
    };
    let second = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses").with_websocket_upgrade(true),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("second request should break hold under active load: {error}"),
    };

    assert_eq!(first.account_id(), alpha.account_id());
    assert_eq!(second.account_id(), beta.account_id());
    assert_ne!(second.selection_reason(), "account_hold_cooldown");
}

#[test]
fn repository_backed_selector_keeps_strict_preferred_account_after_cooldown_expires() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_hold_expired");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
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
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &beta,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );

    let now = Arc::new(Mutex::new(test_unix_seconds()));
    let clock_now = Arc::clone(&now);
    let selector = RepositoryBackedAccountSelector::new_with_runtime(
        &state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        120,
        Arc::new(move || *lock_test_mutex(&clock_now, "test clock")),
    );

    let first = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("first request should select account: {error}"),
    };
    {
        let mut now = lock_test_mutex(&now, "test clock");
        *now = now.saturating_add(121);
    }
    let second = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("second request should keep strict preferred account: {error}"),
    };

    assert_eq!(first.account_id(), alpha.account_id());
    assert_eq!(second.account_id(), alpha.account_id());
    assert_eq!(second.selection_reason(), "preferred_safest_quota");
}

#[test]
fn repository_backed_selector_breaks_hold_when_account_needs_probe() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_hold_probe_required");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
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
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &beta,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );

    let selector = RepositoryBackedAccountSelector::new_with_runtime(
        &state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        120,
        Arc::new(test_unix_seconds),
    );
    let first = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("first request should select account: {error}"),
    };
    persist_account_with_selector_window_status_specs(
        &state,
        &alpha,
        "responses",
        &[
            (18_000, 100, true, SelectorQuotaWindowStatus::Unknown),
            (604_800, 100, false, SelectorQuotaWindowStatus::Unknown),
        ],
    );
    let second = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("second request should skip probe-required held account: {error}"),
    };

    assert_eq!(first.account_id(), alpha.account_id());
    assert_eq!(second.account_id(), beta.account_id());
    assert_eq!(second.selection_reason(), "preferred_safest_quota");
}

#[test]
fn repository_backed_selector_partitions_weighted_state_by_route_band() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_weighted_route_band");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
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
    persist_account_with_selector_windows(&state, &alpha, &["models", "responses"], 10);
    persist_account_with_selector_windows(&state, &beta, &["models", "responses"], 10);

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected_models = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Get, "/v1/models"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("models request should select account: {error}"),
    };
    let selected_responses = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("responses request should select account: {error}"),
    };

    assert_eq!(selected_models.account_id().as_str(), "acct_alpha");
    assert_eq!(selected_responses.account_id().as_str(), "acct_alpha");
}
