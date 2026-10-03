use super::*;

#[tokio::test]
async fn selection_projection_refreshes_session_rollups_for_candidate_burn() {
    let temp_dir = TestTempDir::new("selection_projection_session_count");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_projection_session_count");
    let account_record = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account.clone(),
        "projection",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    store
        .upsert_account(&account_record)
        .await
        .unwrap_or_else(|error| panic!("account should persist: {error}"));
    store
        .upsert_selector_quota_window(
            &PersistedSelectorQuotaWindow::new(
                account.clone(),
                "responses",
                V1_WEEKLY_WINDOW_SECONDS,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(45)
            .with_reset_unix_seconds(100_000)
            .with_effective(true)
            .with_observed_unix_seconds(3_700),
        )
        .await
        .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
    for (observed_unix_seconds, remaining_headroom) in [(100, 50), (1_900, 48), (3_700, 45)] {
        store
            .append_quota_history_observation(&quota_history_observation(
                account.clone(),
                "responses",
                V1_WEEKLY_WINDOW_SECONDS,
                observed_unix_seconds,
                remaining_headroom,
                Some(100_000),
            ))
            .await
            .unwrap_or_else(|error| panic!("quota history should persist: {error}"));
    }
    let historical_session = ReservationId::new("reservation_projection_history");
    store
        .record_active_client_acquired(
            "responses",
            "process-projection-history",
            &historical_session,
            &account,
            100,
            99,
        )
        .await
        .unwrap_or_else(|error| panic!("historical session should acquire: {error}"));
    store
        .record_active_client_released(
            "responses",
            "process-projection-history",
            &historical_session,
            3_700,
        )
        .await
        .unwrap_or_else(|error| panic!("historical session should release: {error}"));
    for index in 0..2 {
        let current_session = ReservationId::new(format!("reservation_projection_current_{index}"));
        store
            .record_active_client_acquired(
                "responses",
                "process-projection-current",
                &current_session,
                &account,
                3_800 + index,
                99,
            )
            .await
            .unwrap_or_else(|error| panic!("current session should acquire: {error}"));
    }

    let projection = project_route_band_selection_inputs(&store, "responses", 3_900, 7_200)
        .await
        .unwrap_or_else(|error| panic!("selection projection should load: {error}"));

    assert_eq!(projection.accounts().len(), 1);
    let projected_account = &projection.accounts()[0];
    assert_eq!(projected_account.current_active_sessions(), 2);
    let weekly_window = projected_account
        .windows()
        .iter()
        .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
        .unwrap_or_else(|| panic!("weekly selector window should project"));
    assert_eq!(
        weekly_window.per_connection_burn_basis_points_per_hour(),
        Some(500)
    );
    assert_eq!(
        weekly_window.burn_rate_confidence(),
        QuotaRunRateConfidence::Normal
    );
}

#[tokio::test]
async fn projection_keeps_per_connection_burn_invariant_across_active_overrides() {
    let temp_dir = TestTempDir::new("selection_projection_per_connection_invariant");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_projection_per_connection");
    let account_record = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account.clone(),
        "projection",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    store
        .upsert_account(&account_record)
        .await
        .unwrap_or_else(|error| panic!("account should persist: {error}"));
    store
        .upsert_selector_quota_window(
            &PersistedSelectorQuotaWindow::new(
                account.clone(),
                "responses",
                V1_WEEKLY_WINDOW_SECONDS,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(45)
            .with_reset_unix_seconds(100_000)
            .with_effective(true)
            .with_observed_unix_seconds(3_700),
        )
        .await
        .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
    for (observed_unix_seconds, remaining_headroom) in [(100, 50), (1_900, 48), (3_700, 45)] {
        store
            .append_quota_history_observation(&quota_history_observation(
                account.clone(),
                "responses",
                V1_WEEKLY_WINDOW_SECONDS,
                observed_unix_seconds,
                remaining_headroom,
                Some(100_000),
            ))
            .await
            .unwrap_or_else(|error| panic!("quota history should persist: {error}"));
    }
    let historical_session = ReservationId::new("reservation_projection_invariant_history");
    store
        .record_active_client_acquired(
            "responses",
            "process-projection-invariant-history",
            &historical_session,
            &account,
            100,
            99,
        )
        .await
        .unwrap_or_else(|error| panic!("historical session should acquire: {error}"));
    store
        .record_active_client_released(
            "responses",
            "process-projection-invariant-history",
            &historical_session,
            3_700,
        )
        .await
        .unwrap_or_else(|error| panic!("historical session should release: {error}"));

    let projection_for_active_override = |active_sessions: u32| {
        let store = &store;
        let account = account.clone();
        async move {
            let mut overrides = HashMap::new();
            overrides.insert(account.clone(), active_sessions);
            let projection = project_route_band_selection_inputs_with_active_counts(
                store,
                "responses",
                3_900,
                7_200,
                Some(&overrides),
            )
            .await
            .unwrap_or_else(|error| panic!("selection projection should load: {error}"));
            let projected_account = projection
                .accounts()
                .first()
                .unwrap_or_else(|| panic!("projected account should exist"));
            let weekly_window = projected_account
                .windows()
                .iter()
                .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
                .unwrap_or_else(|| panic!("weekly selector window should project"));
            (
                weekly_window.per_connection_burn_basis_points_per_hour(),
                weekly_window.projected_exhaustion_unix_seconds(),
            )
        }
    };

    let (zero_active_burn, zero_active_exhaustion) = projection_for_active_override(0).await;
    let (one_active_burn, one_active_exhaustion) = projection_for_active_override(1).await;
    let (two_active_burn, two_active_exhaustion) = projection_for_active_override(2).await;

    assert_eq!(zero_active_burn, Some(500));
    assert_eq!(
        one_active_burn, zero_active_burn,
        "per-connection burn must not change when current active sessions change"
    );
    assert_eq!(
        two_active_burn, zero_active_burn,
        "per-connection burn must not be pre-multiplied by projected active sessions"
    );
    assert_eq!(zero_active_exhaustion, Some(36_300));
    assert_eq!(
        one_active_exhaustion,
        Some(20_100),
        "projected exhaustion must use the candidate aggregate burn for one current active session plus the new session"
    );
    assert_eq!(
        two_active_exhaustion,
        Some(14_700),
        "projected exhaustion must continue shrinking as active-session overrides grow"
    );
}

#[tokio::test]
async fn selection_projection_downgrades_zero_active_session_history() {
    let temp_dir = TestTempDir::new("selection_projection_zero_active_history");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_projection_zero_active");
    let account_record = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account.clone(),
        "zero-active",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    store
        .upsert_account(&account_record)
        .await
        .unwrap_or_else(|error| panic!("account should persist: {error}"));
    store
        .upsert_selector_quota_window(
            &PersistedSelectorQuotaWindow::new(
                account.clone(),
                "responses",
                V1_WEEKLY_WINDOW_SECONDS,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(45)
            .with_reset_unix_seconds(100_000)
            .with_effective(true)
            .with_observed_unix_seconds(3_700),
        )
        .await
        .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
    for (observed_unix_seconds, remaining_headroom) in [(100, 50), (1_900, 48), (3_700, 45)] {
        store
            .append_quota_history_observation(&quota_history_observation(
                account.clone(),
                "responses",
                V1_WEEKLY_WINDOW_SECONDS,
                observed_unix_seconds,
                remaining_headroom,
                Some(100_000),
            ))
            .await
            .unwrap_or_else(|error| panic!("quota history should persist: {error}"));
    }

    let projection = project_route_band_selection_inputs(&store, "responses", 3_900, 7_200)
        .await
        .unwrap_or_else(|error| panic!("selection projection should load: {error}"));

    assert_eq!(projection.accounts().len(), 1);
    let projected_account = &projection.accounts()[0];
    let weekly_window = projected_account
        .windows()
        .iter()
        .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
        .unwrap_or_else(|| panic!("weekly selector window should project"));
    assert_eq!(
        weekly_window.per_connection_burn_basis_points_per_hour(),
        None
    );
    assert_eq!(
        weekly_window.aggregate_burn_basis_points_per_hour(),
        Some(500)
    );
    assert_eq!(
        weekly_window.burn_rate_confidence(),
        QuotaRunRateConfidence::Low,
        "quota burn with zero active-session history must not keep normal confidence"
    );
}

#[tokio::test]
async fn projection_never_manufactures_zero_from_one_observation() {
    let temp_dir = TestTempDir::new("selection_projection_one_observation_no_fake_zero");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_projection_one_observation");
    let account_record = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account.clone(),
        "one-observation",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    store
        .upsert_account(&account_record)
        .await
        .unwrap_or_else(|error| panic!("account should persist: {error}"));
    store
        .upsert_selector_quota_window(
            &PersistedSelectorQuotaWindow::new(
                account.clone(),
                "responses",
                V1_WEEKLY_WINDOW_SECONDS,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(45)
            .with_reset_unix_seconds(100_000)
            .with_effective(true)
            .with_observed_unix_seconds(3_700),
        )
        .await
        .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
    store
        .append_quota_history_observation(&quota_history_observation(
            account.clone(),
            "responses",
            V1_WEEKLY_WINDOW_SECONDS,
            3_700,
            45,
            Some(100_000),
        ))
        .await
        .unwrap_or_else(|error| panic!("quota history should persist: {error}"));

    let projection = project_route_band_selection_inputs(&store, "responses", 3_900, 7_200)
        .await
        .unwrap_or_else(|error| panic!("selection projection should load: {error}"));

    let projected_account = projection
        .accounts()
        .first()
        .unwrap_or_else(|| panic!("projected account should exist"));
    let weekly_window = projected_account
        .windows()
        .iter()
        .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
        .unwrap_or_else(|| panic!("weekly selector window should project"));
    assert_eq!(
        weekly_window.per_connection_burn_basis_points_per_hour(),
        None,
        "one quota observation is insufficient and must not become fake zero burn"
    );
    assert_eq!(
        weekly_window.burn_rate_confidence(),
        QuotaRunRateConfidence::Insufficient
    );
}
