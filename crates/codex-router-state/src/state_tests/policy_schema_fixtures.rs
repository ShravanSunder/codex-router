use std::path::Path;

use rusqlite::Connection;

pub(super) fn convert_current_fixture_to_v10(database_path: &Path) {
    let connection = Connection::open(database_path)
        .unwrap_or_else(|error| panic!("fixture should reopen for v10 conversion: {error}"));
    connection
        .execute_batch(
            "DROP TABLE IF EXISTS _sqlx_migrations;
                 DROP TABLE IF EXISTS account_credit_observations;
                 DROP TABLE IF EXISTS account_credit_policies;
                 DROP TABLE IF EXISTS account_window_observations;
                 DROP TABLE IF EXISTS account_window_rejections;
                 DROP TABLE IF EXISTS credential_maintenance;
                 DROP TABLE account_routing_policies;
                 DROP TABLE session_account_affinities;
                 PRAGMA user_version = 10;",
        )
        .unwrap_or_else(|error| panic!("fixture should convert to v10: {error}"));
    remove_account_provider_column_for_legacy_fixture(&connection);
}

pub(super) fn convert_current_fixture_to_v11(database_path: &Path) {
    let connection = Connection::open(database_path)
        .unwrap_or_else(|error| panic!("fixture should reopen for v11 conversion: {error}"));
    connection
        .execute_batch(
            "DROP TABLE IF EXISTS _sqlx_migrations;
                 DROP TABLE IF EXISTS account_credit_observations;
                 DROP TABLE IF EXISTS account_credit_policies;
                 DROP TABLE IF EXISTS account_window_observations;
                 DROP TABLE IF EXISTS account_window_rejections;
                 DROP TABLE IF EXISTS credential_maintenance;
                 ALTER TABLE account_routing_policies RENAME TO account_routing_policies_current;
                 CREATE TABLE account_routing_policies (
                    account_id TEXT PRIMARY KEY NOT NULL,
                    weekly_quota_floor_basis_points INTEGER NOT NULL
                        CHECK (
                            weekly_quota_floor_basis_points BETWEEN 100 AND 1000
                            AND weekly_quota_floor_basis_points % 100 = 0
                        )
                 );
                 INSERT INTO account_routing_policies
                    SELECT * FROM account_routing_policies_current;
                 DROP TABLE account_routing_policies_current;
                 DROP TABLE session_account_affinities;
                 PRAGMA user_version = 11;",
        )
        .unwrap_or_else(|error| panic!("fixture should convert to v11: {error}"));
    remove_account_provider_column_for_legacy_fixture(&connection);
}

pub(super) fn convert_current_fixture_to_v12(database_path: &Path) {
    let connection = Connection::open(database_path)
        .unwrap_or_else(|error| panic!("fixture should reopen for v12 conversion: {error}"));
    connection
        .execute_batch(
            "DROP TABLE IF EXISTS _sqlx_migrations;
                 DROP TABLE IF EXISTS account_credit_observations;
                 DROP TABLE IF EXISTS account_credit_policies;
                 DROP TABLE IF EXISTS account_window_observations;
                 DROP TABLE IF EXISTS account_window_rejections;
                 DROP TABLE IF EXISTS credential_maintenance;
                 DROP TABLE session_account_affinities;
                 PRAGMA user_version = 12;",
        )
        .unwrap_or_else(|error| panic!("fixture should convert to v12: {error}"));
    remove_account_provider_column_for_legacy_fixture(&connection);
}

pub(super) fn assert_credit_migration_tables(database_path: &Path, expected_present: bool) {
    let connection = Connection::open(database_path).unwrap_or_else(|error| {
        panic!("fixture should reopen for credit-table assertion: {error}")
    });
    for table_name in ["account_credit_observations", "account_credit_policies"] {
        let table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table_name],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("{table_name} schema should query: {error}"));
        assert_eq!(
            table_count,
            if expected_present { 1 } else { 0 },
            "{table_name} should be {} at this migration boundary",
            if expected_present {
                "present"
            } else {
                "absent"
            }
        );
    }
}

fn remove_account_provider_column_for_legacy_fixture(connection: &Connection) {
    connection
        .execute_batch(
            "ALTER TABLE accounts RENAME TO accounts_with_provider;
                 CREATE TABLE accounts (
                    account_id TEXT PRIMARY KEY NOT NULL,
                    label TEXT NOT NULL,
                    status TEXT NOT NULL,
                    active_credential_generation INTEGER
                 );
                 INSERT INTO accounts (
                    account_id, label, status, active_credential_generation
                 )
                 SELECT
                    account_id, label, status, active_credential_generation
                   FROM accounts_with_provider;
                 DROP TABLE accounts_with_provider;",
        )
        .unwrap_or_else(|error| panic!("legacy account fixture should downgrade: {error}"));
}

pub(super) fn assert_intact_v11_policy_database(
    database_path: &Path,
    expected_account_id: &str,
    expected_basis_points: i64,
) {
    let connection = Connection::open(database_path).expect("v11 fixture should reopen");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version should read");
    assert_eq!(version, 11);
    let stored_basis_points: i64 = connection
        .query_row(
            "SELECT weekly_quota_floor_basis_points
                   FROM account_routing_policies
                  WHERE account_id = ?1",
            [expected_account_id],
            |row| row.get(0),
        )
        .expect("preserved policy should read");
    assert_eq!(stored_basis_points, expected_basis_points);
    let table_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'account_routing_policies'",
                [],
                |row| row.get(0),
            )
            .expect("v11 policy schema should read");
    assert!(table_sql.contains("BETWEEN 100 AND 1000"));
    let replacement_exists: i64 = connection
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
    let integrity: String = connection
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .expect("integrity check should run");
    assert_eq!(integrity, "ok");
}
