use super::*;

#[test]
fn sqlite_migration_roundtrips_account_and_quota_snapshot() {
    let temp_dir = TestTempDir::new("migration_roundtrip");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };

    assert_eq!(store.schema_version(), 13);

    let account_id = match AccountId::new("acct_primary") {
        Ok(account_id) => account_id,
        Err(error) => panic!("account id should parse: {error}"),
    };
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "primary",
        AccountStatus::Enabled,
    );
    if let Err(error) = store.upsert_account(&account) {
        panic!("account should persist: {error}");
    }

    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(100)
            .with_route_band("responses", 42)
            .with_reset_unix_seconds(160)
            .with_reset_credits_available(1)
            .with_stale_penalty(false);
    if let Err(error) = store.upsert_quota_snapshot(&snapshot) {
        panic!("quota snapshot should persist: {error}");
    }

    assert_eq!(store.load_account(&account_id), Ok(Some(account)));
    assert_eq!(store.load_quota_snapshot(&account_id), Ok(Some(snapshot)));
}

#[test]
fn quota_snapshots_are_partitioned_by_route_band_for_one_account() {
    let temp_dir = TestTempDir::new("quota_route_band_partition");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_route_partition");
    let responses_snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 90);
    let models_snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_010)
            .with_route_band("models", 7);

    if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &responses_snapshot) {
        panic!("responses quota snapshot should persist: {error}");
    }
    if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &models_snapshot) {
        panic!("models quota snapshot should persist: {error}");
    }

    assert_eq!(
        QuotaSnapshotRepository::load_snapshot_for_route_band(&store, &account_id, "responses"),
        Ok(Some(responses_snapshot))
    );
    assert_eq!(
        QuotaSnapshotRepository::load_snapshot_for_route_band(&store, &account_id, "models"),
        Ok(Some(models_snapshot))
    );
}

#[test]
fn selector_input_reads_durable_per_window_rows_without_status_renderer() {
    let temp_dir = TestTempDir::new("selector_input_windows");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_selector_windows");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "selector",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(3);
    if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
        panic!("account should persist: {error}");
    }
    let short_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Stale,
    )
    .with_remaining_headroom(72)
    .with_reset_unix_seconds(19_000)
    .with_observed_unix_seconds(1_000);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Stale,
    )
    .with_remaining_headroom(41)
    .with_reset_unix_seconds(700_000)
    .with_effective(true)
    .with_observed_unix_seconds(1_000);
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &short_window) {
        panic!("short window should persist: {error}");
    }
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &weekly_window) {
        panic!("weekly window should persist: {error}");
    }

    let selector_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 1_000) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };

    assert_eq!(selector_inputs.len(), 1);
    let input = &selector_inputs[0];
    assert_eq!(input.account_id(), &account_id);
    assert_eq!(input.account_label(), "selector");
    assert_eq!(input.account_status(), AccountStatus::Enabled);
    assert_eq!(input.active_credential_generation(), Some(3));
    assert_eq!(input.route_band(), "responses");
    assert_eq!(input.windows(), &[weekly_window, short_window]);
    let effective = input
        .windows()
        .iter()
        .find(|window| window.effective())
        .unwrap_or_else(|| panic!("effective selector window should exist"));
    assert_eq!(effective.limit_window_seconds(), 604_800);
    assert_eq!(effective.status(), SelectorQuotaWindowStatus::Stale);
    assert_eq!(effective.remaining_headroom(), 41);
    assert_eq!(effective.reset_unix_seconds(), Some(700_000));
}

#[tokio::test]
async fn async_selector_input_matches_sync_repository_projection() {
    let temp_dir = TestTempDir::new("async_selector_input_windows");
    let database_path = temp_dir.path().join("state.sqlite");
    let sync_store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("sync state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_async_selector_windows");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "async-selector",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(9);
    if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
        panic!("account should persist: {error}");
    }
    let short_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(88)
    .with_reset_unix_seconds(20_000)
    .with_observed_unix_seconds(1_000);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(55)
    .with_reset_unix_seconds(700_000)
    .with_effective(true)
    .with_observed_unix_seconds(1_000);
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&sync_store, &short_window)
    {
        panic!("short window should persist: {error}");
    }
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&sync_store, &weekly_window)
    {
        panic!("weekly window should persist: {error}");
    }

    let async_store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let sync_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
        &sync_store,
        "responses",
        1_000,
    ) {
        Ok(inputs) => inputs,
        Err(error) => panic!("sync selector input should load: {error}"),
    };
    let async_inputs = match AsyncSelectorQuotaRepository::selector_inputs_for_route_band(
        &async_store,
        "responses",
        1_000,
    )
    .await
    {
        Ok(inputs) => inputs,
        Err(error) => panic!("async selector input should load: {error}"),
    };

    assert_eq!(async_inputs, sync_inputs);
}
