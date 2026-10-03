use super::*;

#[test]
fn credential_mutation_invalidates_response_backed_alias_family_atomically() {
    let temp_dir = TestTempDir::new("credential_mutation_invalidates_aliases");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_credential_mutation");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "mutation",
        AccountStatus::Disabled,
    );
    if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
        panic!("account should persist: {error}");
    }
    for route_band in [
        "responses",
        "models",
        "memories_trace_summarize",
        "responses_compact",
        "code_review",
    ] {
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(3_000)
                .with_route_band(route_band, 88)
                .with_reset_unix_seconds(4_000)
                .with_stale_penalty(false);
        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &snapshot) {
            panic!("{route_band} snapshot should persist: {error}");
        }
    }
    let legacy_code_review_selector_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "code_review",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(77)
    .with_reset_unix_seconds(999_999)
    .with_effective(true)
    .with_observed_unix_seconds(12_345);
    if let Err(error) =
        SelectorQuotaRepository::upsert_selector_window(&store, &legacy_code_review_selector_window)
    {
        panic!("legacy code_review selector window should persist: {error}");
    }

    if let Err(error) = store.activate_account_credential_generation_and_invalidate_quota(
        &account_id,
        7,
        AccountStatus::Enabled,
    ) {
        panic!("credential mutation should activate and invalidate atomically: {error}");
    }

    let loaded_account = match AccountStateRepository::load_account(&store, &account_id) {
        Ok(Some(account)) => account,
        Ok(None) => panic!("account should still exist"),
        Err(error) => panic!("account should load: {error}"),
    };
    assert_eq!(loaded_account.status(), AccountStatus::Enabled);
    assert_eq!(loaded_account.active_credential_generation(), Some(7));
    for route_band in [
        "responses",
        "models",
        "memories_trace_summarize",
        "responses_compact",
    ] {
        let snapshot = match QuotaSnapshotRepository::load_snapshot_for_route_band(
            &store,
            &account_id,
            route_band,
        ) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("{route_band} stale marker should exist"),
            Err(error) => panic!("{route_band} stale marker should load: {error}"),
        };
        assert_eq!(snapshot.remaining_headroom(), 0);
        assert_eq!(snapshot.observed_unix_seconds(), 0);
        assert_eq!(snapshot.reset_unix_seconds(), None);
        assert!(snapshot.stale_penalty());
        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store, route_band, 3_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => {
                panic!("{route_band} selector input should load after mutation: {error}")
            }
        };
        assert_eq!(selector_inputs.len(), 1);
        let windows = selector_inputs[0].windows();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Ineligible);
        assert_eq!(windows[0].remaining_headroom(), 0);
        assert_eq!(windows[0].observed_unix_seconds(), 0);
        assert!(windows[0].effective());
    }
    let code_review_snapshot = match QuotaSnapshotRepository::load_snapshot_for_route_band(
        &store,
        &account_id,
        "code_review",
    ) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => panic!("code_review stale marker should exist"),
        Err(error) => panic!("code_review stale marker should load: {error}"),
    };
    assert_eq!(code_review_snapshot.remaining_headroom(), 0);
    assert!(code_review_snapshot.stale_penalty());
    let code_review_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "code_review", 3_000)
        {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
    assert_eq!(code_review_inputs.len(), 1);
    assert!(code_review_inputs[0].windows().is_empty());
}

#[tokio::test]
async fn async_credential_generation_activation_fails_when_account_was_disabled() {
    let temp_dir = TestTempDir::new("async_credential_activation_disabled_race");
    let database_path = temp_dir.path().join("state.sqlite");
    let sync_store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("sync state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_async_activation_disabled_race");
    let enabled_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "race",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &enabled_account) {
        panic!("enabled account should persist: {error}");
    }
    drop(sync_store);
    let async_store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let disabled_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "race",
        AccountStatus::Disabled,
    )
    .with_active_credential_generation(1);
    if let Err(error) = async_store.upsert_account(&disabled_account).await {
        panic!("disabled account should persist: {error}");
    }

    let activation_error = match async_store
        .activate_account_credential_generation_if_current_and_invalidate_quota(
            &account_id,
            1,
            2,
            AccountStatus::Enabled,
        )
        .await
    {
        Ok(()) => panic!("disabled account must not be re-enabled by stale refresh commit"),
        Err(error) => error,
    };

    assert_eq!(
        activation_error,
        StateStoreError::AccountConcurrentModification {
            account_id: account_id.as_str().to_owned()
        }
    );
    let loaded_account = async_store
        .load_account(&account_id)
        .await
        .unwrap_or_else(|error| panic!("account should load after failed activation: {error}"))
        .unwrap_or_else(|| panic!("account should still exist"));
    assert_eq!(loaded_account.status(), AccountStatus::Disabled);
    assert_eq!(loaded_account.active_credential_generation(), Some(1));
}

#[tokio::test]
async fn provider_rejection_disables_only_current_credential_generation() {
    let temp_dir = TestTempDir::new("provider_rejection_generation_guard");
    let database_path = temp_dir.path().join("state.sqlite");
    let sync_store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("sync state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_provider_rejection_generation_guard");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "provider-rejection",
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
    if let Err(error) = sync_store.activate_account_credential_generation_and_invalidate_quota(
        &account_id,
        2,
        AccountStatus::Enabled,
    ) {
        panic!("new credential generation should activate: {error}");
    }

    let stale_generation_disabled = match async_store
        .disable_account_if_credential_generation_current(&account_id, 1)
        .await
    {
        Ok(disabled) => disabled,
        Err(error) => panic!("stale provider rejection should be checked: {error}"),
    };
    assert!(!stale_generation_disabled);
    let current_generation_disabled = match async_store
        .disable_account_if_credential_generation_current(&account_id, 2)
        .await
    {
        Ok(disabled) => disabled,
        Err(error) => panic!("current provider rejection should disable account: {error}"),
    };
    assert!(current_generation_disabled);
    let disabled_account = match AccountStateRepository::load_account(&sync_store, &account_id) {
        Ok(Some(account)) => account,
        Ok(None) => panic!("disabled account should still exist"),
        Err(error) => panic!("disabled account should load: {error}"),
    };
    assert_eq!(disabled_account.status(), AccountStatus::Disabled);
    assert_eq!(disabled_account.active_credential_generation(), Some(2));

    if let Err(error) = sync_store.activate_account_credential_generation_and_invalidate_quota(
        &account_id,
        3,
        AccountStatus::Enabled,
    ) {
        panic!("new login generation should restore eligibility: {error}");
    }
    let restored_account = match AccountStateRepository::load_account(&sync_store, &account_id) {
        Ok(Some(account)) => account,
        Ok(None) => panic!("restored account should still exist"),
        Err(error) => panic!("restored account should load: {error}"),
    };
    assert_eq!(restored_account.status(), AccountStatus::Enabled);
    assert_eq!(restored_account.active_credential_generation(), Some(3));
}

#[test]
fn credential_mutation_invalidates_selector_windows_atomically() {
    let temp_dir = TestTempDir::new("credential_mutation_invalidates_selector_windows");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_selector_mutation");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "selector-mutation",
        AccountStatus::Disabled,
    );
    if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
        panic!("account should persist: {error}");
    }
    let selector_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(50)
    .with_effective(true)
    .with_observed_unix_seconds(9_000);
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &selector_window) {
        panic!("selector window should persist: {error}");
    }
    let weekly_selector_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(99)
    .with_reset_unix_seconds(700_000)
    .with_effective(true)
    .with_observed_unix_seconds(9_000);
    if let Err(error) =
        SelectorQuotaRepository::upsert_selector_window(&store, &weekly_selector_window)
    {
        panic!("weekly selector window should persist: {error}");
    }

    if let Err(error) = store.activate_account_credential_generation_and_invalidate_quota(
        &account_id,
        2,
        AccountStatus::Enabled,
    ) {
        panic!("credential mutation should invalidate selector windows: {error}");
    }

    let selector_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 9_000) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
    assert_eq!(selector_inputs.len(), 1);
    let windows = selector_inputs[0].windows();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].limit_window_seconds(), 18_000);
    assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Ineligible);
    assert_eq!(windows[0].remaining_headroom(), 0);
    assert_eq!(windows[0].reset_unix_seconds(), None);
    assert_eq!(windows[0].observed_unix_seconds(), 0);
    assert!(windows[0].effective());
}
