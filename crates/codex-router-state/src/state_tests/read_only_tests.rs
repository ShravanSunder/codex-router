use super::*;

#[tokio::test]
async fn async_read_only_store_reads_without_allowing_writes() {
    let temp_dir = TestTempDir::new("async_read_only_store");
    let database_path = temp_dir.path().join("state.sqlite");
    let writable_store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("writable async state store should open and migrate: {error}"),
    };
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_async_read_only"),
        "read-only",
        AccountStatus::Enabled,
    );
    if let Err(error) = writable_store.upsert_account(&account).await {
        panic!("writable async store should persist account: {error}");
    }
    if let Err(error) = writable_store.close().await {
        panic!("writable async store should close cleanly: {error}");
    }
    drop(writable_store);

    let read_only_store = match AsyncSqliteStateStore::open_read_only(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("read-only async state store should open: {error}"),
    };
    let accounts = match read_only_store.list_accounts().await {
        Ok(accounts) => accounts,
        Err(error) => panic!("read-only async store should list accounts: {error}"),
    };
    assert_eq!(accounts, vec![account]);

    let write_error = read_only_store
        .upsert_account(&AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id("acct_async_read_only_blocked"),
            "blocked",
            AccountStatus::Enabled,
        ))
        .await;
    assert!(
        write_error.is_err(),
        "read-only async store must reject writes"
    );
}

#[tokio::test]
async fn async_read_only_store_reports_missing_current_schema_without_writes() {
    let temp_dir = TestTempDir::new("async_read_only_missing_schema");
    let database_path = temp_dir.path().join("state.sqlite");
    create_v10_database_missing_async_projection_tables(&database_path);
    let raw = Connection::open(&database_path).expect("fixture should reopen");
    raw.pragma_update(None, "user_version", 13_i64)
        .expect("fixture should claim current schema");
    drop(raw);

    let error = expect_error(
        AsyncSqliteStateStore::open_read_only(&database_path).await,
        "read-only open should fail before status queries hit missing tables",
    );

    assert_eq!(
        error,
        StateStoreError::MissingReadOnlySchemaObject {
            object_kind: "table",
            object_name: "quota_history_observations",
        }
    );
}

#[tokio::test]
async fn async_read_only_store_requires_both_credit_tables_in_native_schema() {
    for table_name in ["account_credit_policies", "account_credit_observations"] {
        let temp_dir = TestTempDir::new("async_read_only_missing_credit_schema");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("current native state should initialize");
        store.close().await.expect("state should close");

        let raw = Connection::open(&database_path).expect("native fixture should reopen");
        raw.execute_batch(&format!("DROP TABLE {table_name};"))
            .expect("test should remove one required credit table");
        drop(raw);

        assert_eq!(
            AsyncSqliteStateStore::open_read_only(&database_path)
                .await
                .expect_err("read-only open must reject a missing credit table"),
            StateStoreError::MissingReadOnlySchemaObject {
                object_kind: "table",
                object_name: table_name,
            }
        );
    }

    let temp_dir = TestTempDir::new("async_read_only_missing_credit_column");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("current native state should initialize");
    store.close().await.expect("state should close");
    let raw = Connection::open(&database_path).expect("native fixture should reopen");
    raw.execute_batch(
        "DROP TABLE account_credit_observations;
             CREATE TABLE account_credit_observations (
                 account_id TEXT PRIMARY KEY NOT NULL,
                 credential_generation INTEGER NOT NULL
             );",
    )
    .expect("test should replace the credit table with a missing column");
    drop(raw);

    assert_eq!(
        AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect_err("read-only open must reject a missing credit column"),
        StateStoreError::MissingReadOnlySchemaObject {
            object_kind: "column",
            object_name: "latest_started_attempt",
        }
    );
}

#[tokio::test]
async fn sync_seeded_state_store_is_accepted_by_quota_status_read_only_schema() {
    let temp_dir = TestTempDir::new("sync_seeded_read_only_quota_status");
    let database_path = temp_dir.path().join("state.sqlite");
    let sync_store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("sync state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_sync_seeded_quota_status");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "sync-seeded",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
        panic!("sync seed should persist account: {error}");
    }
    let short_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(91)
    .with_reset_unix_seconds(18_000)
    .with_effective(true)
    .with_observed_unix_seconds(1_000);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(54)
    .with_reset_unix_seconds(604_800)
    .with_observed_unix_seconds(1_000);
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&sync_store, &short_window)
    {
        panic!("sync seed should persist short selector window: {error}");
    }
    if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&sync_store, &weekly_window)
    {
        panic!("sync seed should persist weekly selector window: {error}");
    }
    drop(sync_store);

    let migrated_store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("sync-seeded legacy state should migrate to provider schema");
    migrated_store
        .close()
        .await
        .expect("migrated sync-seeded state should close");
    let read_only_store = match AsyncSqliteStateStore::open_read_only(&database_path).await {
        Ok(store) => store,
        Err(error) => {
            panic!(
                "sync-seeded state database should satisfy read-only quota status schema: {error}"
            )
        }
    };
    let observations = match AsyncQuotaHistoryRepository::quota_history_observations_for_window(
        &read_only_store,
        &account_id,
        "responses",
        604_800,
        0,
        2_000,
    )
    .await
    {
        Ok(observations) => observations,
        Err(error) => {
            panic!("quota status read-only quota history query should find current schema: {error}")
        }
    };

    assert!(
        observations.is_empty(),
        "sync-seeded state should start with no quota history observations"
    );
}

#[tokio::test]
async fn async_writable_store_enables_wal_journal_mode() {
    let temp_dir = TestTempDir::new("async_wal_journal_mode");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    if let Err(error) = store.close().await {
        panic!("async state store should close cleanly: {error}");
    }
    drop(store);

    let connection = match Connection::open(&database_path) {
        Ok(connection) => connection,
        Err(error) => panic!("sqlite database should open for journal inspection: {error}"),
    };
    let journal_mode: String =
        match connection.query_row("PRAGMA journal_mode", [], |row| row.get(0)) {
            Ok(journal_mode) => journal_mode,
            Err(error) => panic!("journal mode should be readable: {error}"),
        };
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
}
