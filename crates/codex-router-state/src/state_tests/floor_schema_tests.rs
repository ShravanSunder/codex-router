use super::*;

#[tokio::test]
async fn mutation_only_store_requires_v12_and_roundtrips_set_then_delete() {
    let temp_dir = TestTempDir::new("weekly_floor_mutation");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("state should open: {error}"));
    let account_id = account_id("acct_weekly_floor");
    store
        .upsert_account(&AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "weekly-floor",
            AccountStatus::Enabled,
        ))
        .await
        .unwrap_or_else(|error| panic!("account should persist: {error}"));
    store.close().await.expect("state should close");

    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("v12 mutation store should open: {error}"));
    let floor = WeeklyQuotaFloorBasisPoints::new(500)
        .unwrap_or_else(|error| panic!("floor should validate: {error}"));
    assert_eq!(
        mutation
            .set_weekly_quota_floor_by_account_id(&account_id, Some(floor))
            .await,
        Ok(WeeklyQuotaFloorMutationResult::Enabled(
            AccountRoutingPolicy::new(account_id.clone(), floor)
        ))
    );
    assert_eq!(
        mutation
            .set_weekly_quota_floor_by_account_id(&account_id, None)
            .await,
        Ok(WeeklyQuotaFloorMutationResult::Disabled)
    );
    let missing_account_id =
        AccountId::new("acct_weekly_floor_missing").expect("valid missing account id");
    assert_eq!(
        mutation
            .set_weekly_quota_floor_by_account_id(&missing_account_id, Some(floor))
            .await,
        Err(StateStoreError::WeeklyQuotaFloorAccountNotFound)
    );
    mutation.close().await;

    let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
        .await
        .unwrap_or_else(|error| panic!("state should reopen: {error}"));
    assert_eq!(
        read_only.list_account_routing_policies().await,
        Ok(Vec::new())
    );
}

#[tokio::test]
async fn mutation_only_store_rejects_v10_and_v11_without_migrating() {
    let temp_dir = TestTempDir::new("weekly_floor_requires_v12");
    let database_path = temp_dir.path().join("state.sqlite");
    let raw = Connection::open(&database_path)
        .unwrap_or_else(|error| panic!("v10 fixture should open: {error}"));
    raw.execute_batch(
        "CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
             );
             PRAGMA user_version = 10;",
    )
    .unwrap_or_else(|error| panic!("v10 fixture should initialize: {error}"));
    drop(raw);

    let error = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect_err("mutation open must reject v10");
    assert_eq!(
        error,
        StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired
    );

    let raw = Connection::open(&database_path)
        .unwrap_or_else(|error| panic!("v10 fixture should reopen: {error}"));
    let version: i64 = raw
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or_else(|error| panic!("version should read: {error}"));
    assert_eq!(version, 10);
    assert!(
        raw.prepare("SELECT 1 FROM account_routing_policies")
            .is_err()
    );
    raw.execute_batch(
        "CREATE TABLE account_routing_policies (
                account_id TEXT PRIMARY KEY NOT NULL,
                weekly_quota_floor_basis_points INTEGER NOT NULL
                    CHECK (
                        weekly_quota_floor_basis_points BETWEEN 100 AND 1000
                        AND weekly_quota_floor_basis_points % 100 = 0
                    )
             );
             PRAGMA user_version = 11;",
    )
    .expect("v11 policy schema should initialize");
    drop(raw);

    let error = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect_err("mutation open must reject v11");
    assert_eq!(
        error,
        StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired
    );
    let raw = Connection::open(&database_path)
        .unwrap_or_else(|error| panic!("v11 fixture should reopen: {error}"));
    let version: i64 = raw
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or_else(|error| panic!("version should read: {error}"));
    assert_eq!(version, 11);
}

#[tokio::test]
async fn read_only_open_rejects_v11_without_migrating_policy_state() {
    let temp_dir = TestTempDir::new("read_only_v11_no_migration");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("current state should open");
    let account_id = account_id("acct_read_only_v11");
    store
        .upsert_account(&AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "read-only-v11",
            AccountStatus::Enabled,
        ))
        .await
        .expect("account should persist");
    store.close().await.expect("state should close");
    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect("current mutation store should open");
    mutation
        .set_weekly_quota_floor_by_account_id(
            &account_id,
            Some(WeeklyQuotaFloorBasisPoints::new(800).expect("valid floor")),
        )
        .await
        .expect("policy should persist");
    mutation.close().await;
    convert_current_fixture_to_v11(&database_path);

    assert_eq!(
        AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect_err("read-only open must reject v11 without migrating"),
        StateStoreError::UnsupportedSchemaVersion { version: 11 }
    );
    assert_intact_v11_policy_database(&database_path, account_id.as_str(), 800);
}

#[tokio::test]
async fn every_v12_open_rejects_malformed_policy_columns() {
    let temp_dir = TestTempDir::new("v12_malformed_policy_columns");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("valid v12 should open");
    store.close().await.expect("valid v12 should close");
    let raw = Connection::open(&database_path).expect("fixture should reopen");
    raw.execute_batch(
        "DROP TABLE account_routing_policies;
             CREATE TABLE account_routing_policies (account_id TEXT PRIMARY KEY NOT NULL);",
    )
    .expect("malformed policy table should install");
    drop(raw);

    assert_eq!(
        AsyncSqliteStateStore::open(&database_path)
            .await
            .expect_err("async writable open must reject malformed policy schema"),
        StateStoreError::Sqlite {
            message: "incompatible account database schema".to_owned()
        }
    );
    assert_eq!(
        AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect_err("read-only open must reject malformed policy schema"),
        StateStoreError::MissingReadOnlySchemaObject {
            object_kind: "column",
            object_name: "weekly_quota_floor_basis_points",
        }
    );
    assert_eq!(
        AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect_err("mutation open must reject malformed policy schema"),
        StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired
    );
    assert_eq!(
        SqliteStateStore::open(&database_path)
            .expect_err("sync writable open must reject malformed policy schema"),
        StateStoreError::MissingReadOnlySchemaObject {
            object_kind: "column",
            object_name: "weekly_quota_floor_basis_points",
        }
    );

    let raw = Connection::open(&database_path).expect("fixture should reopen");
    raw.execute_batch("DROP TABLE account_routing_policies;")
        .expect("policy table should drop");
    drop(raw);
    assert_eq!(
        AsyncSqliteStateStore::open(&database_path)
            .await
            .expect_err("async writable open must reject missing policy table"),
        StateStoreError::Sqlite {
            message: "incompatible account database schema".to_owned()
        }
    );
    assert_eq!(
        AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect_err("read-only open must reject missing policy table"),
        StateStoreError::MissingReadOnlySchemaObject {
            object_kind: "table",
            object_name: "account_routing_policies",
        }
    );
    assert_eq!(
        AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect_err("mutation open must reject missing policy table"),
        StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired
    );
    assert_eq!(
        SqliteStateStore::open(&database_path)
            .expect_err("sync writable open must reject missing policy table"),
        StateStoreError::MissingReadOnlySchemaObject {
            object_kind: "table",
            object_name: "account_routing_policies",
        }
    );
}

#[tokio::test]
async fn production_async_v12_migration_contention_is_atomic_and_retryable() {
    let temp_dir = TestTempDir::new("v12_production_migration_contention");
    let database_path = temp_dir.path().join("state.sqlite");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_migration_contention"),
        "migration-contention",
        AccountStatus::Enabled,
    );
    let store = SqliteStateStore::open(&database_path).expect("fixture should open");
    store
        .upsert_account(&account)
        .expect("account should persist");
    drop(store);
    let raw = Connection::open(&database_path).expect("fixture should reopen");
    raw.execute(
        "INSERT INTO account_routing_policies VALUES (?1, 800)",
        [account.account_id().as_str()],
    )
    .expect("v11-compatible policy should persist");
    drop(raw);
    convert_current_fixture_to_v11(&database_path);

    let lock = Connection::open(&database_path).expect("lock connection should open");
    lock.execute_batch("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
        .expect("writer lock should be held");
    let migration_attempt = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        AsyncSqliteStateStore::open(&database_path),
    )
    .await;
    match migration_attempt {
        Ok(Err(StateStoreError::Sqlite { message })) => assert!(
            message.contains("locked") || message.contains("busy"),
            "held-writer migration should return a lock error, got: {message}"
        ),
        Ok(Err(error)) => panic!("held-writer migration returned the wrong error: {error}"),
        Ok(Ok(_)) => panic!("migration must not commit beneath a held writer"),
        Err(_) => panic!("production migration must return before the test timeout"),
    }
    let version: i64 = lock
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version should read under lock");
    assert_eq!(version, 11);
    let preserved_floor: i64 = lock
            .query_row(
                "SELECT weekly_quota_floor_basis_points FROM account_routing_policies WHERE account_id = ?1",
                [account.account_id().as_str()],
                |row| row.get(0),
            )
            .expect("v11 policy should remain readable");
    assert_eq!(preserved_floor, 800);
    let replacement_exists: i64 = lock
        .query_row(
            "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                     WHERE type = 'table' AND name = 'account_routing_policies_v12'
                 )",
            [],
            |row| row.get(0),
        )
        .expect("replacement-table state should read");
    assert_eq!(replacement_exists, 0);
    let preserved_accounts: i64 = lock
        .query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
        .expect("preserved account count should read");
    assert_eq!(preserved_accounts, 1);
    lock.execute_batch("ROLLBACK;")
        .expect("lock should release");

    let migrated = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("production migration retry should succeed");
    assert_eq!(migrated.schema_version().await, Ok(13));
    assert_eq!(migrated.list_accounts().await, Ok(vec![account.clone()]));
    assert_eq!(
        migrated.list_account_routing_policies().await,
        Ok(vec![AccountRoutingPolicy::new(
            account.account_id().clone(),
            WeeklyQuotaFloorBasisPoints::new(800).expect("valid floor")
        )])
    );
    migrated.close().await.expect("migrated store should close");
    let raw = Connection::open(&database_path).expect("migrated DB should inspect");
    let integrity: String = raw
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .expect("integrity check should run");
    assert_eq!(integrity, "ok");
}
