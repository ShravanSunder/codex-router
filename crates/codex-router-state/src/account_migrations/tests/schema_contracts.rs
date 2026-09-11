use super::*;

#[tokio::test]
async fn marker_only_v7_through_v13_reject_without_mutation() {
    for version in 7_i64..=13 {
        let temporary_database = TemporaryDatabase::new(&format!("marker_only_v{version}"));
        create_marker_only_database(temporary_database.path(), version).await;

        let error = AsyncSqliteStateStore::open(temporary_database.path())
            .await
            .expect_err("version marker without owned schema must reject");
        assert_eq!(
            error,
            crate::sqlite::StateStoreError::Sqlite {
                message: "incompatible account database schema".to_owned()
            }
        );
        let mut connection = open_test_connection(temporary_database.path(), false).await;
        let observed_version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut connection)
            .await
            .expect("legacy marker should remain");
        assert_eq!(observed_version, version);
        let sentinel: String = sqlx::query_scalar("SELECT value FROM unrelated_sentinel")
            .fetch_one(&mut connection)
            .await
            .expect("sentinel should remain");
        assert_eq!(sentinel, "preserved");
        let history_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
              WHERE type = 'table' AND name = '_sqlx_migrations')",
        )
        .fetch_one(&mut connection)
        .await
        .expect("history presence should query");
        assert!(!history_exists);
        connection.close().await.expect("inspection should close");
    }
}

#[tokio::test]
async fn unrecognized_v10_missing_core_combination_rejects_without_mutation() {
    let temporary_database = TemporaryDatabase::new("unrecognized_v10_core");
    create_unrecognized_v10_missing_core_database(temporary_database.path()).await;

    let error = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect_err("unrecognized v10 missing-core shape must reject");
    assert_eq!(
        error,
        crate::sqlite::StateStoreError::Sqlite {
            message: "incompatible account database schema".to_owned()
        }
    );
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let preserved: (String, String, i64) = sqlx::query_as(
        "SELECT label, status, active_credential_generation
           FROM accounts WHERE account_id = 'preserved-account'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("existing account should remain");
    assert_eq!(preserved, ("partial".to_owned(), "disabled".to_owned(), 3));
    let observed_version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("legacy marker should remain");
    assert_eq!(observed_version, 10);
    let sentinel: String = sqlx::query_scalar("SELECT value FROM unrelated_sentinel")
        .fetch_one(&mut connection)
        .await
        .expect("sentinel should remain");
    assert_eq!(sentinel, "preserved");
    let history_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master
          WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(&mut connection)
    .await
    .expect("history presence should query");
    assert!(!history_exists);
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn v13_missing_accounts_rejects_with_other_state_unchanged() {
    let temporary_database = TemporaryDatabase::new("v13_missing_accounts");
    create_v13_missing_accounts_database(temporary_database.path()).await;

    let error = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect_err("v13 missing accounts must reject");
    assert_eq!(
        error,
        crate::sqlite::StateStoreError::Sqlite {
            message: "incompatible account database schema".to_owned()
        }
    );
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let observed_version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("legacy marker should remain");
    assert_eq!(observed_version, 13);
    let accounts_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master
          WHERE type = 'table' AND name = 'accounts')",
    )
    .fetch_one(&mut connection)
    .await
    .expect("accounts presence should query");
    assert!(!accounts_exists, "rejection must not recreate accounts");
    let policy: i64 = sqlx::query_scalar(
        "SELECT weekly_quota_floor_basis_points FROM account_routing_policies
          WHERE account_id = 'preserved-account'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("policy should remain");
    assert_eq!(policy, 900);
    let sentinel: String = sqlx::query_scalar("SELECT value FROM unrelated_sentinel")
        .fetch_one(&mut connection)
        .await
        .expect("sentinel should remain");
    assert_eq!(sentinel, "preserved");
    let history_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master
          WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(&mut connection)
    .await
    .expect("history presence should query");
    assert!(!history_exists);
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn known_v10_missing_projection_shape_completes_and_preserves_rows() {
    let temporary_database = TemporaryDatabase::new("known_v10_missing_projection");
    create_known_v10_missing_projection_database(temporary_database.path()).await;

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("known missing-projection v10 shape should complete");
    let account = migrated
        .load_account(
            &codex_router_core::ids::AccountId::new("preserved-account").expect("valid account"),
        )
        .await
        .expect("account read should succeed")
        .expect("account should remain");
    assert_eq!(account.label(), "projection");
    assert_eq!(account.status(), crate::account::AccountStatus::Disabled);
    assert_eq!(account.active_credential_generation(), Some(9));
    migrated.close().await.expect("migrated store should close");

    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let quota: (i64, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT remaining_headroom, reset_unix_seconds, reset_credits_available
           FROM quota_snapshots WHERE account_id = 'preserved-account'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("quota row should remain");
    assert_eq!(quota, (44, Some(999), Some(7)));
    let missing_tables_rows: i64 = sqlx::query_scalar(
        "SELECT
            (SELECT COUNT(*) FROM affinity_pins)
          + (SELECT COUNT(*) FROM previous_response_affinity_owners)",
    )
    .fetch_one(&mut connection)
    .await
    .expect("created projection tables should query");
    assert_eq!(missing_tables_rows, 0);
    let sentinel: String = sqlx::query_scalar("SELECT value FROM unrelated_sentinel")
        .fetch_one(&mut connection)
        .await
        .expect("sentinel should remain");
    assert_eq!(sentinel, "preserved");
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn known_v10_partial_history_shape_completes_without_fabricated_accounts() {
    let temporary_database = TemporaryDatabase::new("known_v10_partial_history");
    create_known_v10_partial_history_database(temporary_database.path()).await;

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("known partial-history v10 shape should complete");
    assert!(
        migrated
            .list_accounts()
            .await
            .expect("accounts should list")
            .is_empty()
    );
    migrated.close().await.expect("migrated store should close");

    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let lease: (String, String, i64) = sqlx::query_as(
        "SELECT process_run_id, reservation_id, active_pressure FROM active_client_leases",
    )
    .fetch_one(&mut connection)
    .await
    .expect("lease should remain");
    assert_eq!(
        lease,
        (
            "partial-process".to_owned(),
            "partial-reservation".to_owned(),
            6
        )
    );
    let event: (i64, String, i64, Option<i64>, String) = sqlx::query_as(
        "SELECT id, logical_session_id, session_started_unix_seconds,
                session_ended_unix_seconds, transport_kind
           FROM active_session_events",
    )
    .fetch_one(&mut connection)
    .await
    .expect("history event should remain");
    assert_eq!(event, (41, String::new(), 0, None, "unknown".to_owned()));
    let rollup: (i64, i64, i64) = sqlx::query_as(
        "SELECT active_session_seconds, completed_sessions, stale_purged_sessions
           FROM active_session_rollups",
    )
    .fetch_one(&mut connection)
    .await
    .expect("history rollup should remain");
    assert_eq!(rollup, (77, 0, 0));
    let account_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
        .fetch_one(&mut connection)
        .await
        .expect("accounts should query");
    assert_eq!(account_rows, 0);
    let sentinel: String = sqlx::query_scalar("SELECT value FROM unrelated_sentinel")
        .fetch_one(&mut connection)
        .await
        .expect("sentinel should remain");
    assert_eq!(sentinel, "preserved");
    connection.close().await.expect("inspection should close");
}

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
