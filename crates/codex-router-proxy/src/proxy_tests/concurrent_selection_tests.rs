use super::*;

#[tokio::test]
async fn async_repository_backed_selector_six_concurrent_sessions_use_active_load() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_six_session_load");
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
    let now = test_unix_seconds();
    persist_account_with_selector_window_reset_specs(
        &state,
        &askluna,
        "responses",
        &[
            (18_000, 100, true, now + 4 * 3_600),
            (604_800, 18, false, now + 24 * 3_600),
        ],
    );
    persist_account_with_selector_window_reset_specs(
        &state,
        &matches,
        "responses",
        &[
            (18_000, 100, true, now + 4 * 3_600),
            (604_800, 19, false, now + 24 * 3_600),
        ],
    );
    persist_account_with_selector_window_reset_specs(
        &state,
        &ssdev,
        "responses",
        &[
            (18_000, 100, true, now + 4 * 3_600),
            (604_800, 20, false, now + 24 * 3_600),
        ],
    );
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    let history_start = now - 9_200;
    let history_middle = now - 4_700;
    let history_latest = now - 200;
    for (account, start_remaining, latest_remaining) in [
        (askluna.account_id(), 19, 18),
        (matches.account_id(), 20, 19),
        (ssdev.account_id(), 21, 20),
    ] {
        append_history_series_with_reset(
            &async_state,
            account,
            "responses",
            604_800,
            now + 24 * 3_600,
            &[
                (history_start, start_remaining),
                (history_middle, start_remaining),
                (history_latest, latest_remaining),
            ],
        )
        .await;
        record_completed_active_session(
            &async_state,
            account,
            &format!("process-history-{}", account.as_str()),
            &format!("reservation-history-{}", account.as_str()),
            history_start,
            history_latest,
        )
        .await;
    }
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_and_reservations(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        RouteBandReservationBooks::default(),
        0,
        Arc::new(test_unix_seconds),
    );
    let mut selected_accounts = Vec::new();
    let mut active_sessions = Vec::new();

    for session_index in 0..6 {
        let selected = match selector
            .select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses").with_websocket_upgrade(true),
                TokenGeneration::new(1),
                None,
            )
            .await
        {
            Ok(selected) => selected,
            Err(error) => panic!("session {session_index} should select account: {error}"),
        };
        selected_accounts.push(selected.account_id().as_str().to_owned());
        active_sessions.push(selected);
    }

    assert_eq!(
        selected_accounts,
        vec![
            "acct_ssdev".to_owned(),
            "acct_matches".to_owned(),
            "acct_askluna".to_owned(),
            "acct_ssdev".to_owned(),
            "acct_matches".to_owned(),
            "acct_askluna".to_owned(),
        ],
        "six starts should spread across the same effective weekly pool: {selected_accounts:?}"
    );
    assert_eq!(active_sessions.len(), 6);
}

#[tokio::test]
async fn async_repository_backed_selector_concurrent_starts_reserve_atomically() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_atomic_reserve");
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
    persist_account_with_selector_window_reset_specs(
        &state,
        &alpha,
        "responses",
        &[
            (18_000, 50, true, test_unix_seconds() + 4 * 3_600),
            (604_800, 50, false, test_unix_seconds() + 3 * 86_400),
        ],
    );
    persist_account_with_selector_window_reset_specs(
        &state,
        &beta,
        "responses",
        &[
            (18_000, 50, true, test_unix_seconds() + 4 * 3_600),
            (604_800, 50, false, test_unix_seconds() + 3 * 86_400),
        ],
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
    let request = HttpProxyRequest::new(Method::Post, "/v1/responses").with_websocket_upgrade(true);
    let barrier = Arc::new(tokio::sync::Barrier::new(6));
    let selections = futures_util::future::join_all((0..6).map(|session_index| {
        let barrier = Arc::clone(&barrier);
        let request = request.clone();
        let selector = &selector;
        async move {
            barrier.wait().await;
            selector
                .select_upstream_account(&request, TokenGeneration::new(1), None)
                .await
                .unwrap_or_else(|error| {
                    panic!("session {session_index} should select account: {error}")
                })
        }
    }))
    .await;

    let selected_accounts = selections
        .iter()
        .map(|selected| selected.account_id().as_str())
        .collect::<Vec<_>>();
    let alpha_count = selected_accounts
        .iter()
        .filter(|account| **account == "acct_alpha")
        .count();
    let beta_count = selected_accounts
        .iter()
        .filter(|account| **account == "acct_beta")
        .count();

    assert_eq!(
        (alpha_count, beta_count),
        (3, 3),
        "six simultaneous equal-account starts must reserve atomically and balance, got {selected_accounts:?}"
    );
}

#[tokio::test]
async fn async_repository_backed_selector_drains_near_reset_pool_then_far_reset_reserve_s4() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_s4_drain_pool");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let now = test_unix_seconds();
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
    persist_account_with_selector_window_reset_specs(
        &state,
        &askluna,
        "responses",
        &[
            (18_000, 99, true, now + hours_minutes(4, 46)),
            (604_800, 4, false, now + hours_minutes(22, 49)),
        ],
    );
    persist_account_with_selector_window_reset_specs(
        &state,
        &matches,
        "responses",
        &[
            (18_000, 100, true, now + hours_minutes(4, 59)),
            (604_800, 8, false, now + hours_minutes(23, 56)),
        ],
    );
    persist_account_with_selector_window_reset_specs(
        &state,
        &ssdev,
        "responses",
        &[
            (18_000, 97, true, now + hours_minutes(4, 36)),
            (604_800, 26, false, now + 84 * 3_600),
        ],
    );
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    let history_latest = now - 200;
    let askluna_history_start = now - 18_700;
    let askluna_history_middle = now - 9_450;
    append_history_series_with_reset(
        &async_state,
        askluna.account_id(),
        "responses",
        604_800,
        now + hours_minutes(22, 49),
        &[
            (askluna_history_start, 6),
            (askluna_history_middle, 5),
            (history_latest, 4),
        ],
    )
    .await;
    record_completed_active_session(
        &async_state,
        askluna.account_id(),
        "process-history-askluna",
        "reservation-history-askluna",
        askluna_history_start,
        history_latest,
    )
    .await;
    let matches_history_start = now - 7_000;
    let matches_history_middle = now - 3_600;
    append_history_series_with_reset(
        &async_state,
        matches.account_id(),
        "responses",
        604_800,
        now + hours_minutes(23, 56),
        &[
            (matches_history_start, 9),
            (matches_history_middle, 9),
            (history_latest, 8),
        ],
    )
    .await;
    record_completed_active_session(
        &async_state,
        matches.account_id(),
        "process-history-matches",
        "reservation-history-matches",
        matches_history_start,
        history_latest,
    )
    .await;
    let ssdev_history_start = now - 3_650;
    let ssdev_history_middle = now - 1_900;
    append_history_series_with_reset(
        &async_state,
        ssdev.account_id(),
        "responses",
        604_800,
        now + 84 * 3_600,
        &[
            (ssdev_history_start, 27),
            (ssdev_history_middle, 27),
            (history_latest, 26),
        ],
    )
    .await;
    record_completed_active_session(
        &async_state,
        ssdev.account_id(),
        "process-history-ssdev",
        "reservation-history-ssdev",
        ssdev_history_start,
        history_latest,
    )
    .await;
    let active_reservations = RouteBandReservationBooks::default();
    {
        let mut reservations = lock_test_mutex(&active_reservations, "active reservations");
        let book = reservations
            .entry("responses".to_owned())
            .or_insert_with(ReservationBook::default);
        book.reserve_next_at(askluna.account_id().clone(), 1, now);
        book.reserve_next_at(ssdev.account_id().clone(), 1, now);
    }
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_and_reservations(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        Arc::clone(&active_reservations),
        0,
        Arc::new(move || now),
    );
    let mut selected_accounts = Vec::new();
    let mut active_sessions = Vec::new();
    let mut projection_trace = Vec::new();

    for session_index in 0..5 {
        let active_session_overrides = {
            let reservations = lock_test_mutex(&active_reservations, "active reservations");
            reservations.get("responses").map(|book| {
                [
                    askluna.account_id(),
                    matches.account_id(),
                    ssdev.account_id(),
                ]
                .into_iter()
                .map(|account_id| (account_id.clone(), book.active_session_count(account_id)))
                .collect::<HashMap<_, _>>()
            })
        };
        let projection = project_route_band_selection_inputs_with_active_counts(
            &async_state,
            "responses",
            now,
            120,
            active_session_overrides.as_ref(),
        )
        .await
        .unwrap_or_else(|error| panic!("S4 projection should load: {error}"));
        let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
            RouteBand::Responses,
            now,
            RESPONSES_HTTP.clone(),
            projection.accounts().to_vec(),
        ));
        projection_trace.push(
            assessment
                .accounts()
                .iter()
                .map(|account| {
                    (
                        account.account_id().as_str().to_owned(),
                        account.current_active_sessions_for_selection(),
                        account.weekly_projected_exhaustion_unix_seconds().map(
                            |projected_exhaustion_unix_seconds| {
                                projected_exhaustion_unix_seconds.saturating_sub(now)
                            },
                        ),
                        account.projected_drain_gap_after_selection(),
                        account.weekly_survival_margin_basis_points(),
                        account.projected_weekly_runway_seconds(),
                        account.routing_reason().as_str().to_owned(),
                    )
                })
                .collect::<Vec<_>>(),
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
            Err(error) => panic!("session {session_index} should select account: {error}"),
        };
        selected_accounts.push(selected.account_id().as_str().to_owned());
        active_sessions.push(selected);
    }

    assert_eq!(
        selected_accounts,
        vec![
            "acct_matches".to_owned(),
            "acct_matches".to_owned(),
            "acct_askluna".to_owned(),
            "acct_matches".to_owned(),
            "acct_askluna".to_owned(),
        ],
        "S4 runtime path should drain near-reset A/B before consuming far-reset C; projection trace: {projection_trace:?}"
    );
    assert_eq!(active_sessions.len(), 5);
}
