use super::*;

#[tokio::test]
async fn fresh_database_records_the_native_baseline() {
    let temporary_database = TemporaryDatabase::new("fresh_native_baseline");
    let database_path = temporary_database.path().to_path_buf();

    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("fresh database should migrate");
    store.close().await.expect("store should close");

    let mut connection = open_test_connection(&database_path, false).await;
    let recorded: (i64, bool, Vec<u8>) =
        sqlx::query_as("SELECT version, success, checksum FROM _sqlx_migrations")
            .fetch_one(&mut connection)
            .await
            .expect("native history should exist");
    let baseline = MIGRATOR.iter().next().expect("baseline should be embedded");
    assert_eq!(recorded.0, baseline.version);
    assert!(recorded.1);
    assert_eq!(recorded.2, baseline.checksum.as_ref());
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn incompatible_owned_table_rejects_without_native_history() {
    let temporary_database = TemporaryDatabase::new("incompatible_owned_table");
    let database_path = temporary_database.path().to_path_buf();
    create_conflicting_v13_database(&database_path).await;

    let error = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect_err("conflicting schema must reject");
    assert!(format!("{error}").contains("incompatible account database schema"));

    let mut connection = open_test_connection(&database_path, false).await;
    let native_history_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
        )
        .fetch_one(&mut connection)
        .await
        .expect("history presence should query");
    assert!(!native_history_exists);
    let label: String =
        sqlx::query_scalar("SELECT label FROM accounts WHERE account_id = 'preserved'")
            .fetch_one(&mut connection)
            .await
            .expect("preserved row should remain");
    assert_eq!(label, "before-failure");
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn weakened_policy_check_rejects_without_adoption() {
    let temporary_database = TemporaryDatabase::new("weakened_policy_check");
    let database_path = temporary_database.path().to_path_buf();
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("baseline database should initialize");
    store.close().await.expect("baseline store should close");
    let mut connection = open_test_connection(&database_path, false).await;
    sqlx::raw_sql(
        "DROP TABLE _sqlx_migrations;
             ALTER TABLE account_routing_policies RENAME TO valid_policies;
             CREATE TABLE account_routing_policies (
                account_id TEXT PRIMARY KEY NOT NULL,
                weekly_quota_floor_basis_points INTEGER NOT NULL
                    CHECK (
                        weekly_quota_floor_basis_points BETWEEN 100 AND 1500
                        AND weekly_quota_floor_basis_points % 100 = 0
                        OR 1
                    )
             );
             INSERT INTO account_routing_policies SELECT * FROM valid_policies;
             DROP TABLE valid_policies;
             PRAGMA user_version = 13;",
    )
    .execute(&mut connection)
    .await
    .expect("weakened policy fixture should install");
    connection.close().await.expect("fixture should close");

    let error = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect_err("weakened check must reject");
    assert!(format!("{error}").contains("incompatible account database schema"));
    let mut connection = open_test_connection(&database_path, false).await;
    assert!(
        sqlx::query("INSERT INTO account_routing_policies VALUES ('invalid', 101)")
            .execute(&mut connection)
            .await
            .is_ok(),
        "rejection must leave the weakened fixture unchanged"
    );
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn schema_token_modifiers_reject_without_adoption() {
    for (label, mutation) in [
        (
            "missing_autoincrement",
            "DROP TABLE quota_history_observations;
             CREATE TABLE quota_history_observations (
                id INTEGER PRIMARY KEY,
                account_id TEXT NOT NULL,
                account_label TEXT NOT NULL,
                route_band TEXT NOT NULL,
                limit_window_seconds INTEGER NOT NULL,
                observed_unix_seconds INTEGER NOT NULL,
                remaining_headroom INTEGER NOT NULL,
                reset_unix_seconds INTEGER,
                window_status TEXT NOT NULL,
                effective INTEGER NOT NULL,
                refresh_source TEXT NOT NULL,
                refresh_success INTEGER NOT NULL,
                refresh_error_class TEXT,
                reset_credits_available INTEGER
             );",
        ),
        (
            "added_collation",
            "DROP TABLE accounts;
             CREATE TABLE accounts (
                account_id TEXT COLLATE NOCASE PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
             );",
        ),
        (
            "added_conflict_clause",
            "DROP TABLE accounts;
             CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY ON CONFLICT REPLACE NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
             );",
        ),
        (
            "added_strict",
            "DROP TABLE accounts;
             CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
             ) STRICT;",
        ),
    ] {
        let temporary_database = TemporaryDatabase::new(label);
        create_legacy_v13_database(temporary_database.path(), label).await;
        let mut connection = open_test_connection(temporary_database.path(), false).await;
        sqlx::raw_sql(mutation)
            .execute(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("{label} fixture should install: {error}"));
        connection.close().await.expect("fixture should close");

        let error = AsyncSqliteStateStore::open(temporary_database.path())
            .await
            .expect_err("schema modifier must reject");
        assert_eq!(
            error,
            crate::sqlite::StateStoreError::Sqlite {
                message: "incompatible account database schema".to_owned()
            }
        );
        let mut connection = open_test_connection(temporary_database.path(), false).await;
        let history_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
            )
            .fetch_one(&mut connection)
            .await
            .expect("history presence should query");
        assert!(!history_exists);
        connection.close().await.expect("inspection should close");
    }
}
