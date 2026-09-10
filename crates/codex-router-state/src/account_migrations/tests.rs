use std::path::Path;

use rusqlite::Connection;

use super::MIGRATOR;
use super::migrate_and_rollback_for_test;
use super::migrate_with_process_checkpoint_for_test;
use crate::sqlite::AsyncSqliteStateStore;

mod fixtures;

use fixtures::*;

#[tokio::test]
async fn fresh_database_records_the_native_baseline() {
    let temporary_database = TemporaryDatabase::new("fresh_native_baseline");
    let database_path = temporary_database.path().to_path_buf();

    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("fresh database should migrate");
    store.close().await.expect("store should close");

    let connection = Connection::open(&database_path).expect("database should reopen");
    let recorded = connection
        .query_row(
            "SELECT version, success, checksum FROM _sqlx_migrations",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .expect("native history should exist");
    let baseline = MIGRATOR.iter().next().expect("baseline should be embedded");
    assert_eq!(recorded.0, baseline.version);
    assert!(recorded.1);
    assert_eq!(recorded.2, baseline.checksum.as_ref());
}

#[tokio::test]
async fn incompatible_owned_table_rejects_without_native_history() {
    let temporary_database = TemporaryDatabase::new("incompatible_owned_table");
    let database_path = temporary_database.path().to_path_buf();
    create_conflicting_v13_database(&database_path);

    let error = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect_err("conflicting schema must reject");
    assert!(format!("{error}").contains("incompatible account database schema"));

    let connection = Connection::open(&database_path).expect("database should reopen");
    let native_history_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
            [],
            |row| row.get(0),
        )
        .expect("history presence should query");
    assert!(!native_history_exists);
    let label: String = connection
        .query_row(
            "SELECT label FROM accounts WHERE account_id = 'preserved'",
            [],
            |row| row.get(0),
        )
        .expect("preserved row should remain");
    assert_eq!(label, "before-failure");
}

#[tokio::test]
async fn weakened_policy_check_rejects_without_adoption() {
    let temporary_database = TemporaryDatabase::new("weakened_policy_check");
    let database_path = temporary_database.path().to_path_buf();
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("baseline database should initialize");
    store.close().await.expect("baseline store should close");
    let connection = Connection::open(&database_path).expect("fixture should reopen");
    connection
        .execute_batch(
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
        .expect("weakened policy fixture should install");
    drop(connection);

    let error = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect_err("weakened check must reject");
    assert!(format!("{error}").contains("incompatible account database schema"));
    let connection = Connection::open(&database_path).expect("fixture should inspect");
    assert!(
        connection
            .execute(
                "INSERT INTO account_routing_policies VALUES ('invalid', 101)",
                []
            )
            .is_ok(),
        "rejection must leave the weakened fixture unchanged"
    );
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
        create_legacy_v13_database(temporary_database.path(), label);
        let connection = Connection::open(temporary_database.path()).expect("fixture should open");
        connection
            .execute_batch(mutation)
            .unwrap_or_else(|error| panic!("{label} fixture should install: {error}"));
        drop(connection);

        let error = AsyncSqliteStateStore::open(temporary_database.path())
            .await
            .expect_err("schema modifier must reject");
        assert_eq!(
            error,
            crate::sqlite::StateStoreError::Sqlite {
                message: "incompatible account database schema".to_owned()
            }
        );
        let connection =
            Connection::open(temporary_database.path()).expect("fixture should inspect");
        let history_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
                [],
                |row| row.get(0),
            )
            .expect("history presence should query");
        assert!(!history_exists);
    }
}

#[tokio::test]
async fn legacy_adoption_rolls_back_as_one_native_transaction() {
    let temporary_database = TemporaryDatabase::new("native_rollback");
    create_legacy_v13_database(temporary_database.path(), "rollback-account");
    let pool = open_test_pool(temporary_database.path()).await;

    migrate_and_rollback_for_test(&pool)
        .await
        .expect("injected outer rollback should succeed");
    pool.close().await;

    assert_legacy_database_unchanged(temporary_database.path(), "rollback-account");
    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("migration should retry after rollback");
    migrated.close().await.expect("migrated store should close");
    assert_native_history_present(temporary_database.path());
}

#[tokio::test]
async fn fresh_native_baseline_savepoint_rolls_back_with_outer_transaction() {
    let temporary_database = TemporaryDatabase::new("fresh_native_rollback");
    Connection::open(temporary_database.path())
        .expect("empty database should be created")
        .close()
        .expect("empty database should close");
    let pool = open_test_pool(temporary_database.path()).await;

    migrate_and_rollback_for_test(&pool)
        .await
        .expect("outer rollback after nested baseline should succeed");
    pool.close().await;

    let connection = Connection::open(temporary_database.path()).expect("database should inspect");
    let application_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
              WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .expect("application tables should query");
    assert_eq!(application_tables, 0);
    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("fresh baseline should retry after rollback");
    migrated.close().await.expect("fresh retry should close");
    assert_native_history_present(temporary_database.path());
}

#[tokio::test]
async fn killed_process_before_outer_commit_leaves_legacy_database() {
    let temporary_database = TemporaryDatabase::new("killed_before_commit");
    create_legacy_v13_database(temporary_database.path(), "killed-account");

    run_migration_child_to_checkpoint(temporary_database.path(), false);

    assert_legacy_database_unchanged(temporary_database.path(), "killed-account");
    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("migration should recover after killed writer");
    migrated.close().await.expect("migrated store should close");
    assert_native_history_present(temporary_database.path());
}

#[tokio::test]
async fn killed_process_rolls_back_nested_fresh_baseline() {
    let temporary_database = TemporaryDatabase::new("killed_fresh_baseline");
    Connection::open(temporary_database.path())
        .expect("empty database should be created")
        .close()
        .expect("empty database should close");

    run_migration_child_to_checkpoint(temporary_database.path(), false);

    let connection = Connection::open(temporary_database.path()).expect("database should inspect");
    let application_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
              WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .expect("application tables should query");
    assert_eq!(application_tables, 0);
    drop(connection);
    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("fresh baseline should recover after killed child");
    migrated
        .close()
        .await
        .expect("fresh recovered store should close");
    assert_native_history_present(temporary_database.path());
}

#[tokio::test]
async fn killed_process_after_outer_commit_reopens_native_state() {
    let temporary_database = TemporaryDatabase::new("killed_after_commit");
    create_legacy_v13_database(temporary_database.path(), "committed-account");

    run_migration_child_to_checkpoint(temporary_database.path(), true);

    assert_native_history_present(temporary_database.path());
    let reopened = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("committed native state should reopen");
    reopened.close().await.expect("reopened store should close");
}

#[tokio::test]
async fn killed_v11_policy_rebuild_restores_old_constraint_and_retries() {
    let temporary_database = TemporaryDatabase::new("killed_v11_policy_rebuild");
    create_legacy_v11_database(temporary_database.path());

    run_migration_child_to_checkpoint(temporary_database.path(), false);

    let connection = Connection::open(temporary_database.path()).expect("v11 should inspect");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("v11 marker should read");
    assert_eq!(version, 11);
    let policy: i64 = connection
        .query_row(
            "SELECT weekly_quota_floor_basis_points FROM account_routing_policies
              WHERE account_id = 'preserved-account'",
            [],
            |row| row.get(0),
        )
        .expect("v11 policy should survive");
    assert_eq!(policy, 900);
    let table_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master
              WHERE type = 'table' AND name = 'account_routing_policies'",
            [],
            |row| row.get(0),
        )
        .expect("v11 policy definition should read");
    assert!(table_sql.contains("BETWEEN 100 AND 1000"));
    assert!(!table_sql.contains("BETWEEN 100 AND 1500"));
    let replacement_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
              WHERE type = 'table' AND name = 'account_routing_policies_v12')",
            [],
            |row| row.get(0),
        )
        .expect("replacement presence should query");
    assert!(!replacement_exists);
    drop(connection);

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("v11 migration should retry");
    migrated
        .close()
        .await
        .expect("migrated v11 store should close");
    assert_native_history_present(temporary_database.path());
    let connection =
        Connection::open(temporary_database.path()).expect("native v11 should inspect");
    let policy: i64 = connection
        .query_row(
            "SELECT weekly_quota_floor_basis_points FROM account_routing_policies
              WHERE account_id = 'preserved-account'",
            [],
            |row| row.get(0),
        )
        .expect("migrated policy should survive");
    assert_eq!(policy, 900);
}

#[tokio::test]
async fn killed_v7_lease_rebuild_restores_old_shape_and_retries() {
    let temporary_database = TemporaryDatabase::new("killed_v7_lease_rebuild");
    create_legacy_v7_database(temporary_database.path());

    run_migration_child_to_checkpoint(temporary_database.path(), false);

    let connection = Connection::open(temporary_database.path()).expect("v7 should inspect");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("v7 marker should read");
    assert_eq!(version, 7);
    let columns = connection
        .prepare("PRAGMA table_info(active_client_leases)")
        .expect("old lease columns should prepare")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("old lease columns should query")
        .collect::<Result<Vec<_>, _>>()
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
    let old_row: (String, String, String, i64) = connection
        .query_row(
            "SELECT route_band, reservation_id, account_id, acquired_unix_seconds
               FROM active_client_leases",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
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
    drop(connection);

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("v7 migration should retry");
    migrated
        .close()
        .await
        .expect("migrated v7 store should close");
    let connection = Connection::open(temporary_database.path()).expect("native v7 should inspect");
    let migrated_row: (String, String, i64) = connection
        .query_row(
            "SELECT process_run_id, reservation_id, active_pressure FROM active_client_leases",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("migrated lease should query");
    assert_eq!(
        migrated_row,
        ("legacy".to_owned(), "legacy-reservation".to_owned(), 8)
    );
}

#[tokio::test]
async fn v7_current_lease_shape_preserves_identity_pressure_and_empty_history() {
    let temporary_database = TemporaryDatabase::new("v7_current_lease");
    create_legacy_v7_current_lease_database(temporary_database.path());

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("v7 current lease fixture should migrate");
    let counts = migrated
        .active_client_counts_for_route_band_read_only("responses", 200, 300)
        .await
        .expect("preserved current lease should count");
    assert_eq!(
        counts,
        vec![crate::sqlite::ActiveClientCount::new(
            codex_router_core::ids::AccountId::new("preserved-account").expect("valid account"),
            1,
            5,
        )]
    );
    assert!(
        migrated
            .active_session_events_for_route_band("responses")
            .await
            .expect("session history should load")
            .is_empty(),
        "adoption must not fabricate history from a current lease"
    );
    migrated.close().await.expect("migrated store should close");

    let connection = Connection::open(temporary_database.path()).expect("database should inspect");
    let lease: (String, String, String, i64) = connection
        .query_row(
            "SELECT process_run_id, reservation_id, account_id, active_pressure
               FROM active_client_leases",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("current lease should remain exact");
    assert_eq!(
        lease,
        (
            "current-process".to_owned(),
            "current-reservation".to_owned(),
            "preserved-account".to_owned(),
            5,
        )
    );
    assert_native_history_present(temporary_database.path());
}

#[tokio::test]
async fn v7_absent_lease_table_creates_empty_table_without_fabricated_history() {
    let temporary_database = TemporaryDatabase::new("v7_absent_lease");
    create_legacy_v7_without_lease_table(temporary_database.path());

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("v7 absent lease fixture should migrate");
    assert!(
        migrated
            .active_client_counts_for_route_band_read_only("responses", 200, 300)
            .await
            .expect("empty lease table should read")
            .is_empty()
    );
    assert!(
        migrated
            .active_session_events_for_route_band("responses")
            .await
            .expect("session history should load")
            .is_empty(),
        "adoption must not fabricate history for an absent lease table"
    );
    migrated.close().await.expect("migrated store should close");

    let connection = Connection::open(temporary_database.path()).expect("database should inspect");
    let lease_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM active_client_leases", [], |row| {
            row.get(0)
        })
        .expect("created lease table should query");
    assert_eq!(lease_rows, 0);
    assert_native_history_present(temporary_database.path());
}

#[tokio::test]
async fn every_v0_initialization_prefix_completes_without_losing_existing_rows() {
    for prefix_length in 0..=LEGACY_V0_TABLE_STATEMENTS.len() {
        let temporary_database = TemporaryDatabase::new(&format!("v0_prefix_{prefix_length}"));
        let connection =
            Connection::open(temporary_database.path()).expect("v0 fixture should open");
        for statement in &LEGACY_V0_TABLE_STATEMENTS[..prefix_length] {
            connection
                .execute_batch(statement)
                .expect("v0 prefix statement should execute");
        }
        if prefix_length != 0 {
            connection
                .execute(
                    "INSERT INTO accounts VALUES ('v0-account', 'prefix', 'disabled', 17)",
                    [],
                )
                .expect("v0 account should seed");
        }
        drop(connection);

        let migrated = AsyncSqliteStateStore::open(temporary_database.path())
            .await
            .unwrap_or_else(|error| panic!("v0 prefix {prefix_length} should migrate: {error}"));
        if prefix_length != 0 {
            let account = migrated
                .load_account(
                    &codex_router_core::ids::AccountId::new("v0-account").expect("valid id"),
                )
                .await
                .expect("v0 account should load")
                .expect("v0 account should remain");
            assert_eq!(account.label(), "prefix");
            assert_eq!(account.status(), crate::account::AccountStatus::Disabled);
            assert_eq!(account.active_credential_generation(), Some(17));
        }
        migrated
            .close()
            .await
            .expect("v0 migrated store should close");
        assert_native_history_present(temporary_database.path());
    }
}

#[tokio::test]
async fn v9_missing_history_fields_adopts_exact_defaults_and_preserves_rows() {
    for optional_field_mask in 0_u8..64 {
        let temporary_database =
            TemporaryDatabase::new(&format!("v9_history_fields_{optional_field_mask}"));
        create_legacy_v9_history_shape(temporary_database.path(), optional_field_mask);

        let migrated = AsyncSqliteStateStore::open(temporary_database.path())
            .await
            .unwrap_or_else(|error| {
                panic!("v9 history mask {optional_field_mask} should migrate: {error}")
            });
        migrated
            .close()
            .await
            .expect("v9 migrated store should close");

        let connection = Connection::open(temporary_database.path()).expect("v9 should inspect");
        let event: (String, i64, Option<i64>, String) = connection
            .query_row(
                "SELECT logical_session_id, session_started_unix_seconds,
                        session_ended_unix_seconds, transport_kind
                   FROM active_session_events WHERE id = 41",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("v9 event should remain");
        assert_eq!(
            event,
            (
                if optional_field_mask & 1 != 0 {
                    "logical-v9".to_owned()
                } else {
                    String::new()
                },
                if optional_field_mask & 2 != 0 { 88 } else { 0 },
                (optional_field_mask & 4 != 0).then_some(99),
                if optional_field_mask & 8 != 0 {
                    "websocket".to_owned()
                } else {
                    "unknown".to_owned()
                },
            ),
            "event values should match mask {optional_field_mask}"
        );
        let rollup: (i64, i64, i64) = connection
            .query_row(
                "SELECT active_session_seconds, completed_sessions, stale_purged_sessions
                   FROM active_session_rollups WHERE account_id = 'preserved-account'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("v9 rollup should remain");
        assert_eq!(
            rollup,
            (
                77,
                if optional_field_mask & 16 != 0 { 3 } else { 0 },
                if optional_field_mask & 32 != 0 { 4 } else { 0 },
            ),
            "rollup values should match mask {optional_field_mask}"
        );
        assert_native_history_present(temporary_database.path());
    }
}

#[tokio::test]
async fn two_writable_openers_serialize_adoption_without_duplicate_history() {
    let temporary_database = TemporaryDatabase::new("two_writers");
    create_legacy_v13_database(temporary_database.path(), "two-writer-account");
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

    let connection = Connection::open(temporary_database.path()).expect("database should inspect");
    let history_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM _sqlx_migrations", [], |row| {
            row.get(0)
        })
        .expect("history should query");
    assert_eq!(history_rows, 1);
    let account_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
        .expect("accounts should query");
    assert_eq!(account_rows, 1);
}

#[tokio::test]
async fn checksum_mismatch_rejects_without_repair() {
    let temporary_database = TemporaryDatabase::new("checksum_mismatch");
    let store = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("native database should initialize");
    store.close().await.expect("native store should close");
    let connection = Connection::open(temporary_database.path()).expect("history should open");
    connection
        .execute("UPDATE _sqlx_migrations SET checksum = X'00'", [])
        .expect("checksum should be corrupted");
    drop(connection);

    let error = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect_err("checksum mismatch must reject");
    assert!(format!("{error}").contains("incompatible account migration history"));
    let connection = Connection::open(temporary_database.path()).expect("history should inspect");
    let checksum: Vec<u8> = connection
        .query_row("SELECT checksum FROM _sqlx_migrations", [], |row| {
            row.get(0)
        })
        .expect("checksum should query");
    assert_eq!(checksum, vec![0]);
}

#[tokio::test]
async fn dirty_native_history_rejects_without_repair() {
    let temporary_database = TemporaryDatabase::new("dirty_history");
    let store = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("native database should initialize");
    store.close().await.expect("native store should close");
    let connection = Connection::open(temporary_database.path()).expect("history should open");
    connection
        .execute("UPDATE _sqlx_migrations SET success = 0", [])
        .expect("history should become dirty");
    drop(connection);

    let error = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect_err("dirty history must reject");
    assert!(format!("{error}").contains("dirty account migration history"));
    let connection = Connection::open(temporary_database.path()).expect("history should inspect");
    let success: bool = connection
        .query_row("SELECT success FROM _sqlx_migrations", [], |row| row.get(0))
        .expect("dirty marker should query");
    assert!(!success);
}

#[tokio::test]
async fn legacy_and_native_read_only_openers_never_create_history() {
    let legacy_database = TemporaryDatabase::new("legacy_read_only");
    create_legacy_v13_database(legacy_database.path(), "legacy-read-only");
    let legacy_reader = AsyncSqliteStateStore::open_read_only(legacy_database.path())
        .await
        .expect("legacy v13 should remain readable");
    legacy_reader
        .close()
        .await
        .expect("legacy reader should close");
    assert_legacy_database_unchanged(legacy_database.path(), "legacy-read-only");

    let native_database = TemporaryDatabase::new("native_read_only");
    let native_writer = AsyncSqliteStateStore::open(native_database.path())
        .await
        .expect("native database should initialize");
    native_writer
        .close()
        .await
        .expect("native writer should close");
    let before = migration_history_bytes(native_database.path());
    let native_reader = AsyncSqliteStateStore::open_read_only(native_database.path())
        .await
        .expect("native current database should read");
    native_reader
        .close()
        .await
        .expect("native reader should close");
    assert_eq!(migration_history_bytes(native_database.path()), before);
}

#[tokio::test]
async fn unsupported_async_legacy_version_preserves_rows_and_history_absence() {
    let temporary_database = TemporaryDatabase::new("unsupported_version");
    let connection = Connection::open(temporary_database.path()).expect("fixture should open");
    connection
        .execute_batch(
            "CREATE TABLE sentinel (value TEXT NOT NULL);
             INSERT INTO sentinel VALUES ('preserved');
             PRAGMA user_version = 6;",
        )
        .expect("unsupported fixture should initialize");
    drop(connection);

    assert_eq!(
        AsyncSqliteStateStore::open(temporary_database.path())
            .await
            .expect_err("unsupported async legacy version must reject"),
        crate::sqlite::StateStoreError::UnsupportedSchemaVersion { version: 6 }
    );
    let connection = Connection::open(temporary_database.path()).expect("fixture should inspect");
    let value: String = connection
        .query_row("SELECT value FROM sentinel", [], |row| row.get(0))
        .expect("sentinel should remain");
    assert_eq!(value, "preserved");
    let history_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
            [],
            |row| row.get(0),
        )
        .expect("history presence should query");
    assert!(!history_exists);
}

#[tokio::test]
async fn migration_process_helper() {
    let Ok(database_path) = std::env::var("CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_DATABASE") else {
        return;
    };
    let commit_before_checkpoint =
        std::env::var_os("CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_COMMIT_BEFORE_CHECKPOINT").is_some();
    let pool = open_test_pool(Path::new(&database_path)).await;
    migrate_with_process_checkpoint_for_test(&pool, commit_before_checkpoint)
        .await
        .expect("migration child should reach and leave checkpoint");
    pool.close().await;
}
