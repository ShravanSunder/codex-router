use super::*;

#[test]
fn quota_snapshot_upsert_keeps_code_review_out_of_selector_projection() {
    let temp_dir = TestTempDir::new("code_review_status_only");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_code_review_status_only");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "status-only",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
        panic!("account should persist: {error}");
    }
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(3_000)
            .with_route_band("code_review", 88)
            .with_reset_unix_seconds(4_000)
            .with_stale_penalty(false);

    if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &snapshot) {
        panic!("code_review quota snapshot should persist: {error}");
    }

    assert_eq!(
        QuotaSnapshotRepository::load_snapshot_for_route_band(&store, &account_id, "code_review"),
        Ok(Some(snapshot))
    );
    let selector_inputs =
        match SelectorQuotaRepository::selector_inputs_for_route_band(&store, "code_review", 3_000)
        {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
    assert_eq!(selector_inputs.len(), 1);
    assert!(selector_inputs[0].windows().is_empty());
}

#[test]
fn corrupt_account_metadata_fails_closed_for_that_account_only() {
    let temp_dir = TestTempDir::new("corrupt_account");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let healthy_id = match AccountId::new("acct_healthy") {
        Ok(account_id) => account_id,
        Err(error) => panic!("account id should parse: {error}"),
    };
    let corrupt_id = match AccountId::new("acct_corrupt") {
        Ok(account_id) => account_id,
        Err(error) => panic!("account id should parse: {error}"),
    };

    if let Err(error) = store.upsert_account(&AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        healthy_id.clone(),
        "healthy",
        AccountStatus::Enabled,
    )) {
        panic!("healthy account should persist: {error}");
    }
    if let Err(error) = store.insert_raw_account_for_test(
        corrupt_id.as_str(),
        "refresh-token-canary",
        "not-a-status",
    ) {
        panic!("corrupt fixture should persist: {error}");
    }

    let corrupt_error = match store.load_account(&corrupt_id) {
        Ok(account) => panic!("corrupt account should not load: {account:?}"),
        Err(error) => error,
    };

    assert!(matches!(
        corrupt_error,
        StateStoreError::CorruptAccount { .. }
    ));
    assert!(!format!("{corrupt_error:?}").contains("refresh-token-canary"));
    assert_eq!(
        store.load_account(&healthy_id),
        Ok(Some(AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            healthy_id,
            "healthy",
            AccountStatus::Enabled
        )))
    );
}

#[test]
fn unsupported_schema_version_fails_closed_on_open() {
    let temp_dir = TestTempDir::new("unsupported_schema");
    let database_path = temp_dir.path().join("state.sqlite");
    let raw = match rusqlite::Connection::open(&database_path) {
        Ok(raw) => raw,
        Err(error) => panic!("raw sqlite should open: {error}"),
    };
    if let Err(error) = raw.pragma_update(None, "user_version", 999_i64) {
        panic!("schema fixture should persist: {error}");
    }
    drop(raw);

    let error = match SqliteStateStore::open(&database_path) {
        Ok(store) => panic!("unsupported schema should not open: {store:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        StateStoreError::UnsupportedSchemaVersion { version: 999 }
    );
}

#[test]
fn state_repository_contracts_are_proxy_usable() {
    let temp_dir = TestTempDir::new("repository_contracts");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_contract");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "contract",
        AccountStatus::Enabled,
    );
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_200)
            .with_route_band("responses", 33);
    let affinity_key = AffinityKey::new("response_previous");

    if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
        panic!("account repository should persist: {error}");
    }
    if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &snapshot) {
        panic!("quota repository should persist: {error}");
    }
    if let Err(error) = AffinityRepository::pin_account(&store, &affinity_key, &account_id) {
        panic!("affinity repository should persist: {error}");
    }

    assert_eq!(
        AccountStateRepository::load_account(&store, &account_id),
        Ok(Some(account))
    );
    assert_eq!(
        QuotaSnapshotRepository::load_snapshot(&store, &account_id),
        Ok(Some(snapshot))
    );
    assert_eq!(
        AffinityRepository::load_pin(&store, &affinity_key),
        Ok(Some(account_id))
    );
}
