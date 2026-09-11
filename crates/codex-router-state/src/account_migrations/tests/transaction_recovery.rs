use super::*;

#[tokio::test]
async fn legacy_adoption_rolls_back_as_one_native_transaction() {
    let temporary_database = TemporaryDatabase::new("native_rollback");
    create_legacy_v13_database(temporary_database.path(), "rollback-account").await;
    let pool = open_test_pool(temporary_database.path()).await;

    migrate_and_rollback_for_test(&pool)
        .await
        .expect("injected outer rollback should succeed");
    pool.close().await;

    assert_legacy_database_unchanged(temporary_database.path(), "rollback-account").await;
    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("migration should retry after rollback");
    migrated.close().await.expect("migrated store should close");
    assert_native_history_present(temporary_database.path()).await;
}

#[tokio::test]
async fn fresh_native_baseline_savepoint_rolls_back_with_outer_transaction() {
    let temporary_database = TemporaryDatabase::new("fresh_native_rollback");
    open_test_connection(temporary_database.path(), true)
        .await
        .close()
        .await
        .expect("empty database should close");
    let pool = open_test_pool(temporary_database.path()).await;

    migrate_and_rollback_for_test(&pool)
        .await
        .expect("outer rollback after nested baseline should succeed");
    pool.close().await;

    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let application_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
              WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("application tables should query");
    assert_eq!(application_tables, 0);
    connection.close().await.expect("inspection should close");
    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("fresh baseline should retry after rollback");
    migrated.close().await.expect("fresh retry should close");
    assert_native_history_present(temporary_database.path()).await;
}

#[tokio::test]
async fn killed_process_before_outer_commit_leaves_legacy_database() {
    let temporary_database = TemporaryDatabase::new("killed_before_commit");
    create_legacy_v13_database(temporary_database.path(), "killed-account").await;

    run_migration_child_to_checkpoint(temporary_database.path(), false);

    assert_legacy_database_unchanged(temporary_database.path(), "killed-account").await;
    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("migration should recover after killed writer");
    migrated.close().await.expect("migrated store should close");
    assert_native_history_present(temporary_database.path()).await;
}

#[tokio::test]
async fn killed_process_rolls_back_nested_fresh_baseline() {
    let temporary_database = TemporaryDatabase::new("killed_fresh_baseline");
    open_test_connection(temporary_database.path(), true)
        .await
        .close()
        .await
        .expect("empty database should close");

    run_migration_child_to_checkpoint(temporary_database.path(), false);

    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let application_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
              WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("application tables should query");
    assert_eq!(application_tables, 0);
    connection.close().await.expect("inspection should close");
    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("fresh baseline should recover after killed child");
    migrated
        .close()
        .await
        .expect("fresh recovered store should close");
    assert_native_history_present(temporary_database.path()).await;
}

#[tokio::test]
async fn killed_process_after_outer_commit_reopens_native_state() {
    let temporary_database = TemporaryDatabase::new("killed_after_commit");
    create_legacy_v13_database(temporary_database.path(), "committed-account").await;

    run_migration_child_to_checkpoint(temporary_database.path(), true);

    assert_native_history_present(temporary_database.path()).await;
    let reopened = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("committed native state should reopen");
    reopened.close().await.expect("reopened store should close");
}

#[tokio::test]
async fn killed_v11_policy_rebuild_restores_old_constraint_and_retries() {
    let temporary_database = TemporaryDatabase::new("killed_v11_policy_rebuild");
    create_legacy_v11_database(temporary_database.path()).await;

    run_migration_child_to_checkpoint(temporary_database.path(), false);

    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("v11 marker should read");
    assert_eq!(version, 11);
    let policy: i64 = sqlx::query_scalar(
        "SELECT weekly_quota_floor_basis_points FROM account_routing_policies
              WHERE account_id = 'preserved-account'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("v11 policy should survive");
    assert_eq!(policy, 900);
    let table_sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master
              WHERE type = 'table' AND name = 'account_routing_policies'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("v11 policy definition should read");
    assert!(table_sql.contains("BETWEEN 100 AND 1000"));
    assert!(!table_sql.contains("BETWEEN 100 AND 1500"));
    let replacement_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master
              WHERE type = 'table' AND name = 'account_routing_policies_v12')",
    )
    .fetch_one(&mut connection)
    .await
    .expect("replacement presence should query");
    assert!(!replacement_exists);
    connection
        .close()
        .await
        .expect("v11 inspection should close");

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("v11 migration should retry");
    migrated
        .close()
        .await
        .expect("migrated v11 store should close");
    assert_native_history_present(temporary_database.path()).await;
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let policy: i64 = sqlx::query_scalar(
        "SELECT weekly_quota_floor_basis_points FROM account_routing_policies
              WHERE account_id = 'preserved-account'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("migrated policy should survive");
    assert_eq!(policy, 900);
    connection
        .close()
        .await
        .expect("native inspection should close");
}

#[tokio::test]
async fn killed_v7_lease_rebuild_restores_old_shape_and_retries() {
    let temporary_database = TemporaryDatabase::new("killed_v7_lease_rebuild");
    create_legacy_v7_database(temporary_database.path()).await;

    run_migration_child_to_checkpoint(temporary_database.path(), false);

    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("v7 marker should read");
    assert_eq!(version, 7);
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('active_client_leases') ORDER BY cid",
    )
    .fetch_all(&mut connection)
    .await
    .expect("old lease columns should collect");
    assert_eq!(
        columns,
        vec![
            "route_band",
            "reservation_id",
            "account_id",
            "acquired_unix_seconds"
        ]
    );
    let old_row: (String, String, String, i64) = sqlx::query_as(
        "SELECT route_band, reservation_id, account_id, acquired_unix_seconds
               FROM active_client_leases",
    )
    .fetch_one(&mut connection)
    .await
    .expect("old lease should survive");
    assert_eq!(
        old_row,
        (
            "responses".to_owned(),
            "legacy-reservation".to_owned(),
            "preserved-account".to_owned(),
            123
        )
    );
    connection
        .close()
        .await
        .expect("v7 inspection should close");

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("v7 migration should retry");
    migrated
        .close()
        .await
        .expect("migrated v7 store should close");
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let migrated_row: (String, String, i64) = sqlx::query_as(
        "SELECT process_run_id, reservation_id, active_pressure FROM active_client_leases",
    )
    .fetch_one(&mut connection)
    .await
    .expect("migrated lease should query");
    assert_eq!(
        migrated_row,
        ("legacy".to_owned(), "legacy-reservation".to_owned(), 8)
    );
    connection
        .close()
        .await
        .expect("native inspection should close");
}

#[tokio::test]
async fn two_writable_openers_serialize_adoption_without_duplicate_history() {
    let temporary_database = TemporaryDatabase::new("two_writers");
    create_legacy_v13_database(temporary_database.path(), "two-writer-account").await;
    let first_path = temporary_database.path().to_path_buf();
    let second_path = first_path.clone();

    let (first, second) = tokio::join!(
        AsyncSqliteStateStore::open(&first_path),
        AsyncSqliteStateStore::open(&second_path)
    );
    let mut successful_stores = Vec::new();
    for result in [first, second] {
        match result {
            Ok(store) => successful_stores.push(store),
            Err(crate::sqlite::StateStoreError::Sqlite { message }) => {
                assert!(message.contains("locked") || message.contains("busy"));
            }
            Err(error) => panic!("concurrent opener returned unexpected error: {error}"),
        }
    }
    assert!(
        !successful_stores.is_empty(),
        "one opener must adopt the database"
    );
    for store in successful_stores {
        store.close().await.expect("concurrent store should close");
    }
    let retry = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("post-contention retry should open");
    retry.close().await.expect("retry should close");

    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let history_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await
        .expect("history should query");
    assert_eq!(history_rows, 1);
    let account_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
        .fetch_one(&mut connection)
        .await
        .expect("accounts should query");
    assert_eq!(account_rows, 1);
    connection.close().await.expect("inspection should close");
}
