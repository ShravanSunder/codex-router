use super::*;

#[tokio::test]
async fn async_repository_backed_selector_releases_active_load_reservation() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_active_load_release");
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
    let active_reservations = RouteBandReservationBooks::default();
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_and_reservations(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        active_reservations.clone(),
        0,
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
        Err(error) => panic!("first request should select account: {error}"),
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
        Err(error) => panic!("second request should avoid active loaded account: {error}"),
    };
    release_account_reservation(
        &active_reservations,
        "responses",
        match first.reservation_handle() {
            Some(reservation_handle) => reservation_handle,
            None => panic!("first selection should reserve active load"),
        },
    );
    let third = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses").with_websocket_upgrade(true),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("third request should return to released account: {error}"),
    };

    assert_eq!(first.account_id(), alpha.account_id());
    assert_eq!(second.account_id(), beta.account_id());
    assert!(second.reservation_handle().is_some());
    assert_eq!(third.account_id(), alpha.account_id());
}

#[tokio::test]
async fn async_repository_backed_selector_counts_http_and_websocket_as_one_session_each() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_distinct_load_weights");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_alpha"),
        "alpha",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &account,
        "responses",
        &[(18_000, 90, true), (604_800, 90, false)],
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
        0,
        Arc::new(test_unix_seconds),
    );

    let http_selection = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("HTTP request should reserve account load: {error}"),
    };
    let websocket_selection = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses").with_websocket_upgrade(true),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("WebSocket request should reserve account load: {error}"),
    };

    assert_eq!(
        http_selection
            .reservation_handle()
            .map(ReservationHandle::headroom_cost),
        Some(1)
    );
    assert_eq!(
        websocket_selection
            .reservation_handle()
            .map(ReservationHandle::headroom_cost),
        Some(1)
    );
}

#[tokio::test]
async fn async_repository_backed_selector_ignores_stale_active_load_reservations() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_stale_active_load");
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
        &[(18_000, 70, true), (604_800, 70, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &beta,
        "responses",
        &[(18_000, 70, true), (604_800, 70, false)],
    );
    let active_reservations = RouteBandReservationBooks::default();
    {
        let mut reservations = lock_test_mutex(&active_reservations, "active reservations");
        reservations
            .entry("responses".to_owned())
            .or_insert_with(ReservationBook::default)
            .reserve_at(
                ReservationId::new("stale_alpha_load"),
                alpha.account_id().clone(),
                80,
                test_unix_seconds() - 10_000,
            );
    }
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_and_reservations(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        active_reservations.clone(),
        0,
        Arc::new(test_unix_seconds),
    );

    let selected = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses").with_websocket_upgrade(true),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("stale active load should be purged before selection: {error}"),
    };

    assert_eq!(selected.account_id(), alpha.account_id());
    let reservations = lock_test_mutex(&active_reservations, "active reservations");
    let active_sessions = reservations
        .get("responses")
        .map_or(0, |book| book.active_session_count(alpha.account_id()));
    assert_eq!(active_sessions, 1);
}

#[tokio::test]
async fn repository_backed_selector_weights_weekly_pressure_over_short_window_headroom() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_weekly_pressure");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let short_rich_weekly_poor = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_short_rich_weekly_poor"),
        "short-rich-weekly-poor",
        AccountStatus::Enabled,
    );
    let weekly_healthy = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_weekly_healthy"),
        "weekly-healthy",
        AccountStatus::Enabled,
    );
    // Equal reset times keep this test about weekly health rather than reset ordering.
    let now = test_unix_seconds();
    for (account, windows) in [
        (
            &short_rich_weekly_poor,
            [(18_000, 90, true), (604_800, 5, false)],
        ),
        (&weekly_healthy, [(18_000, 50, true), (604_800, 50, false)]),
    ] {
        let account_with_generation = account.clone().with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&state, &account_with_generation)
        {
            panic!("account should persist: {error}");
        }
        for (limit_window_seconds, remaining_headroom, effective) in windows {
            let selector_window = PersistedSelectorQuotaWindow::new(
                account.account_id().clone(),
                "responses",
                limit_window_seconds,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(remaining_headroom)
            .with_effective(effective)
            .with_observed_unix_seconds(now)
            .with_reset_unix_seconds(now.saturating_add(limit_window_seconds));
            if let Err(error) =
                SelectorQuotaRepository::upsert_selector_window(&state, &selector_window)
            {
                panic!("selector quota window should persist: {error}");
            }
        }
    }
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    let weekly_reset = now.saturating_add(604_800);
    append_history_series_with_reset(
        &async_state,
        short_rich_weekly_poor.account_id(),
        "responses",
        604_800,
        weekly_reset,
        &[(now - 7_200, 20), (now - 3_600, 12), (now, 5)],
    )
    .await;
    append_history_series_with_reset(
        &async_state,
        weekly_healthy.account_id(),
        "responses",
        604_800,
        weekly_reset,
        &[(now - 7_200, 52), (now - 3_600, 51), (now, 50)],
    )
    .await;

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("repository-backed selector should select account: {error}"),
    };

    assert_eq!(selected.account_id(), weekly_healthy.account_id());
    assert_eq!(selected.selection_reason(), "preferred_weekly_healthier");
}
