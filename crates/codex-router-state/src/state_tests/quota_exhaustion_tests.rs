use super::*;

#[tokio::test]
async fn selection_projection_downgrades_partial_active_session_history() {
    let temp_dir = TestTempDir::new("selection_projection_partial_active_history");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_projection_partial_active");
    let account_record = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account.clone(),
        "partial-active",
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
    let late_session = ReservationId::new("reservation_projection_late_history");
    store
        .record_active_client_acquired(
            "responses",
            "process-projection-late-history",
            &late_session,
            &account,
            3_000,
            99,
        )
        .await
        .unwrap_or_else(|error| panic!("late session should acquire: {error}"));
    store
        .record_active_client_released(
            "responses",
            "process-projection-late-history",
            &late_session,
            3_700,
        )
        .await
        .unwrap_or_else(|error| panic!("late session should release: {error}"));

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
        None,
        "partial active-session history must fall back to aggregate quota burn"
    );
    assert_eq!(
        weekly_window.aggregate_burn_basis_points_per_hour(),
        Some(500),
        "partial active-session history keeps aggregate fallback separate"
    );
    assert_eq!(
        weekly_window.burn_rate_confidence(),
        QuotaRunRateConfidence::Low,
        "partial active-session history must not keep normal confidence"
    );
}

#[tokio::test]
async fn async_quota_exhaustion_marks_route_band_windows_ineligible() {
    let temp_dir = TestTempDir::new("async_quota_exhaustion");
    let database_path = temp_dir.path().join("state.sqlite");
    let sync_store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("sync state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_quota_exhausted");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "exhausted",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
        panic!("account should persist: {error}");
    }
    for (limit_window_seconds, remaining_headroom, effective) in
        [(18_000, 70, true), (604_800, 55, false)]
    {
        let window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            limit_window_seconds,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(remaining_headroom)
        .with_effective(effective)
        .with_observed_unix_seconds(1_000)
        .with_reset_unix_seconds(10_000 + limit_window_seconds);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&sync_store, &window) {
            panic!("selector window should persist: {error}");
        }
    }
    let async_store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };

    if let Err(error) = AsyncQuotaExhaustionRepository::mark_route_band_quota_exhausted(
        &async_store,
        &account_id,
        "responses",
        2_000,
    )
    .await
    {
        panic!("quota exhaustion should persist: {error}");
    }

    let inputs = match AsyncSelectorQuotaRepository::selector_inputs_for_route_band(
        &async_store,
        "responses",
        2_000,
    )
    .await
    {
        Ok(inputs) => inputs,
        Err(error) => panic!("selector input should load: {error}"),
    };
    let input = inputs
        .iter()
        .find(|input| input.account_id() == &account_id)
        .unwrap_or_else(|| panic!("exhausted account selector input should exist"));
    assert_eq!(input.windows().len(), 2);
    assert!(
        input
            .windows()
            .iter()
            .all(|window| window.status() == SelectorQuotaWindowStatus::Ineligible)
    );
    assert!(
        input
            .windows()
            .iter()
            .all(|window| window.remaining_headroom() == 0)
    );
    assert!(
        input
            .windows()
            .iter()
            .all(|window| window.observed_unix_seconds() == 2_000)
    );
}

#[tokio::test]
async fn async_quota_exhaustion_without_existing_windows_blocks_expected_windows() {
    let temp_dir = TestTempDir::new("async_quota_exhaustion_no_windows");
    let database_path = temp_dir.path().join("state.sqlite");
    let sync_store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("sync state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_unknown_exhausted");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "unknown-exhausted",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
        panic!("account should persist: {error}");
    }
    let async_store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };

    if let Err(error) = AsyncQuotaExhaustionRepository::mark_route_band_quota_exhausted(
        &async_store,
        &account_id,
        "responses",
        2_000,
    )
    .await
    {
        panic!("quota exhaustion should persist: {error}");
    }

    let inputs = match AsyncSelectorQuotaRepository::selector_inputs_for_route_band(
        &async_store,
        "responses",
        2_000,
    )
    .await
    {
        Ok(inputs) => inputs,
        Err(error) => panic!("selector input should load: {error}"),
    };
    let input = inputs
        .iter()
        .find(|input| input.account_id() == &account_id)
        .unwrap_or_else(|| panic!("exhausted account selector input should exist"));
    assert_eq!(
        input
            .windows()
            .iter()
            .map(|window| window.limit_window_seconds())
            .collect::<Vec<_>>(),
        vec![18_000, 604_800]
    );
    assert!(
        input
            .windows()
            .iter()
            .all(|window| window.status() == SelectorQuotaWindowStatus::Ineligible),
        "suspect-exhausted accounts without prior quota windows must not degrade to unknown fallback"
    );
}

#[tokio::test]
async fn async_quota_exhaustion_expires_back_to_probe_candidate() {
    let temp_dir = TestTempDir::new("async_quota_exhaustion_ttl");
    let database_path = temp_dir.path().join("state.sqlite");
    let sync_store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("sync state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_exhausted_ttl");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "exhausted-ttl",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
        panic!("account should persist: {error}");
    }
    let async_store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };

    if let Err(error) = AsyncQuotaExhaustionRepository::mark_route_band_quota_exhausted(
        &async_store,
        &account_id,
        "responses",
        2_000,
    )
    .await
    {
        panic!("quota exhaustion should persist: {error}");
    }

    let expired_inputs = match AsyncSelectorQuotaRepository::selector_inputs_for_route_band(
        &async_store,
        "responses",
        2_301,
    )
    .await
    {
        Ok(inputs) => inputs,
        Err(error) => panic!("selector input should load after suspect TTL expiry: {error}"),
    };
    let expired_input = expired_inputs
        .iter()
        .find(|input| input.account_id() == &account_id)
        .unwrap_or_else(|| panic!("expired suspect account selector input should exist"));
    assert!(
        expired_input.windows().is_empty(),
        "expired suspect-exhausted state should return to unknown probe behavior"
    );
}
