use super::*;

#[test]
fn refresh_success_replaces_selector_windows_and_records_status() {
    let temp_dir = TestTempDir::new("refresh_success_replaces_windows");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_refresh_success");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "refresh",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
        panic!("account should persist: {error}");
    }
    let old_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(99)
    .with_observed_unix_seconds(900);
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &old_window) {
        panic!("old selector window should persist: {error}");
    }
    let short_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(72)
    .with_reset_unix_seconds(19_000)
    .with_effective(true)
    .with_observed_unix_seconds(1_000);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(44)
    .with_reset_unix_seconds(700_000)
    .with_observed_unix_seconds(1_000);

    if let Err(error) = SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        &store,
        &account_id,
        "responses",
        &[short_window.clone(), weekly_window.clone()],
        1_000,
        2_000,
    ) {
        panic!("refresh success should persist atomically: {error}");
    }

    let selector_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 1_500) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
    assert_eq!(selector_inputs.len(), 1);
    assert_eq!(
        selector_inputs[0].windows(),
        &[short_window.clone(), weekly_window.clone()]
    );
    let statuses =
        match SelectorQuotaRepository::quota_refresh_statuses_for_route_band(&store, "responses") {
            Ok(statuses) => statuses,
            Err(error) => panic!("refresh statuses should load: {error}"),
        };
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].account_id(), &account_id);
    assert_eq!(
        statuses[0].status_source(),
        QuotaRefreshStatusSource::Recorded
    );
    assert_eq!(statuses[0].last_success_unix_seconds(), Some(1_000));
    assert_eq!(statuses[0].last_attempt_unix_seconds(), Some(1_000));
    assert_eq!(statuses[0].last_error_class(), None);
    assert_eq!(statuses[0].stale_after_unix_seconds(), Some(2_000));

    if let Err(error) = SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        &store,
        &account_id,
        "responses",
        std::slice::from_ref(&weekly_window),
        1_100,
        2_100,
    ) {
        panic!("weekly-only refresh should replace the current pair: {error}");
    }
    let weekly_only_inputs =
        SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 1_500)
            .unwrap_or_else(|error| panic!("weekly-only selector input should load: {error}"));
    assert_eq!(
        weekly_only_inputs[0].windows(),
        std::slice::from_ref(&weekly_window)
    );

    if let Err(error) = SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        &store,
        &account_id,
        "responses",
        &[short_window.clone(), weekly_window.clone()],
        1_200,
        2_200,
    ) {
        panic!("returned short window should restore the current pair: {error}");
    }
    let restored_inputs =
        SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 1_500)
            .unwrap_or_else(|error| panic!("restored selector input should load: {error}"));
    assert_eq!(restored_inputs[0].windows(), &[short_window, weekly_window]);
}

#[test]
fn refresh_failure_preserves_windows_and_overlays_stale_on_read() {
    let temp_dir = TestTempDir::new("refresh_failure_preserves_windows");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_refresh_failure");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "failure",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
        panic!("account should persist: {error}");
    }
    let short_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(61)
    .with_reset_unix_seconds(19_000)
    .with_effective(true)
    .with_observed_unix_seconds(1_000);
    if let Err(error) = SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        &store,
        &account_id,
        "responses",
        std::slice::from_ref(&short_window),
        1_000,
        10_000,
    ) {
        panic!("refresh success seed should persist: {error}");
    }

    if let Err(error) = SelectorQuotaRepository::record_refresh_failure_preserving_selector_windows(
        &store,
        &account_id,
        "responses",
        2_000,
        QuotaRefreshErrorClass::NetworkError,
    ) {
        panic!("refresh failure should preserve windows: {error}");
    }

    let selector_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 2_000) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
    assert_eq!(selector_inputs.len(), 1);
    let windows = selector_inputs[0].windows();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Stale);
    assert_eq!(windows[0].remaining_headroom(), 61);
    assert_eq!(windows[0].reset_unix_seconds(), Some(19_000));
    assert_eq!(windows[0].observed_unix_seconds(), 1_000);
    let statuses =
        match SelectorQuotaRepository::quota_refresh_statuses_for_route_band(&store, "responses") {
            Ok(statuses) => statuses,
            Err(error) => panic!("refresh statuses should load: {error}"),
        };
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].last_success_unix_seconds(), Some(1_000));
    assert_eq!(statuses[0].last_attempt_unix_seconds(), Some(2_000));
    assert_eq!(
        statuses[0].last_error_class(),
        Some(QuotaRefreshErrorClass::NetworkError)
    );
    assert_eq!(statuses[0].stale_after_unix_seconds(), Some(2_000));
}

#[test]
fn legacy_selector_rows_without_refresh_status_are_stale_and_reported() {
    let temp_dir = TestTempDir::new("legacy_missing_refresh_status");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let alpha_id = account_id("acct_alpha_legacy");
    let beta_id = account_id("acct_beta_empty");
    if let Err(error) = AccountStateRepository::upsert_account(
        &store,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            alpha_id.clone(),
            "alpha",
            AccountStatus::Enabled,
        ),
    ) {
        panic!("alpha account should persist: {error}");
    }
    if let Err(error) = AccountStateRepository::upsert_account(
        &store,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            beta_id,
            "beta",
            AccountStatus::Enabled,
        ),
    ) {
        panic!("beta account should persist: {error}");
    }
    let legacy_window = PersistedSelectorQuotaWindow::new(
        alpha_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(88)
    .with_reset_unix_seconds(20_000)
    .with_effective(true)
    .with_observed_unix_seconds(1_000);
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &legacy_window) {
        panic!("legacy selector window should persist: {error}");
    }

    let selector_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 1_500) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
    assert_eq!(selector_inputs.len(), 2);
    let alpha = selector_inputs
        .iter()
        .find(|input| input.account_id() == &alpha_id)
        .unwrap_or_else(|| panic!("alpha selector input should exist"));
    assert_eq!(alpha.windows().len(), 1);
    assert_eq!(
        alpha.windows()[0].status(),
        SelectorQuotaWindowStatus::Stale
    );
    assert_eq!(alpha.windows()[0].remaining_headroom(), 88);
    let statuses =
        match SelectorQuotaRepository::quota_refresh_statuses_for_route_band(&store, "responses") {
            Ok(statuses) => statuses,
            Err(error) => panic!("refresh statuses should load: {error}"),
        };
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].account_id(), &alpha_id);
    assert_eq!(
        statuses[0].status_source(),
        QuotaRefreshStatusSource::LegacyMissingRefreshStatus
    );
    assert_eq!(statuses[0].last_success_unix_seconds(), None);
    assert_eq!(statuses[0].last_attempt_unix_seconds(), None);
    assert_eq!(statuses[0].last_error_class(), None);
    assert_eq!(statuses[0].stale_after_unix_seconds(), None);
}

#[test]
fn v2_migration_backfills_selector_windows_from_existing_quota_snapshots() {
    let temp_dir = TestTempDir::new("v2_selector_backfill");
    let database_path = temp_dir.path().join("state.sqlite");
    create_v2_database_with_quota_snapshot(&database_path);

    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("v2 state store should migrate to current schema: {error}"),
    };

    assert_eq!(store.schema_version(), 13);
    let selector_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 1_000) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load after migration: {error}"),
        };
    assert_eq!(selector_inputs.len(), 1);
    let input = &selector_inputs[0];
    assert_eq!(input.account_id(), &account_id("acct_v2_backfill"));
    assert_eq!(input.active_credential_generation(), Some(1));
    assert_eq!(input.windows().len(), 1);
    let window = &input.windows()[0];
    assert_eq!(window.status(), SelectorQuotaWindowStatus::Stale);
    assert_eq!(window.remaining_headroom(), 64);
    assert_eq!(window.reset_unix_seconds(), Some(2_000));
    assert_eq!(window.limit_window_seconds(), 18_000);
    assert!(window.effective());
    let expected_code_review_snapshot = PersistedQuotaSnapshot::new(
        account_id("acct_v2_backfill"),
        QuotaSnapshotSource::MockEndpoint,
    )
    .with_observed_unix_seconds(1_000)
    .with_route_band("code_review", 64)
    .with_reset_unix_seconds(2_000)
    .with_stale_penalty(false);
    assert_eq!(
        QuotaSnapshotRepository::load_snapshot_for_route_band(
            &store,
            &account_id("acct_v2_backfill"),
            "code_review"
        ),
        Ok(Some(expected_code_review_snapshot))
    );
    let code_review_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "code_review", 1_000)
        {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
    assert_eq!(code_review_inputs.len(), 1);
    assert!(code_review_inputs[0].windows().is_empty());
}

#[test]
fn v3_migration_removes_legacy_code_review_selector_windows() {
    let temp_dir = TestTempDir::new("v3_code_review_selector_cleanup");
    let database_path = temp_dir.path().join("state.sqlite");
    create_v3_database_with_legacy_code_review_selector_window(&database_path);

    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("v3 state store should migrate to current schema: {error}"),
    };

    assert_eq!(store.schema_version(), 13);
    let responses_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 1_000) {
            Ok(inputs) => inputs,
            Err(error) => panic!("responses selector input should load: {error}"),
        };
    assert_eq!(responses_inputs.len(), 1);
    assert_eq!(responses_inputs[0].windows().len(), 1);
    let code_review_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "code_review", 1_000)
        {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
    assert_eq!(code_review_inputs.len(), 1);
    assert!(code_review_inputs[0].windows().is_empty());
}

#[test]
fn v6_migration_adds_reset_credits_without_losing_existing_quota() {
    let temp_dir = TestTempDir::new("v6_reset_credits_migration");
    let database_path = temp_dir.path().join("state.sqlite");
    create_v6_database_without_reset_credits(&database_path);

    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("v6 state store should migrate to current schema: {error}"),
    };

    assert_eq!(store.schema_version(), 13);
    let account_id = account_id("acct_v6_reset_credits");
    let snapshot = match QuotaSnapshotRepository::load_snapshot_for_route_band(
        &store,
        &account_id,
        "responses",
    ) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => panic!("v6 quota snapshot should survive migration"),
        Err(error) => panic!("v6 quota snapshot should load after migration: {error}"),
    };
    assert_eq!(snapshot.remaining_headroom(), 42);
    assert_eq!(snapshot.reset_unix_seconds(), Some(2_000));
    assert_eq!(snapshot.reset_credits_available(), None);
}
