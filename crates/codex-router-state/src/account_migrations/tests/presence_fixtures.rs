use std::path::Path;

use sqlx::Connection;

use super::fixtures::execute_fixture_sql;
use super::fixtures::open_test_connection;

pub(super) async fn create_marker_only_database(database_path: &Path, version: i64) {
    let mut connection = open_test_connection(database_path, true).await;
    sqlx::raw_sql(
        "CREATE TABLE unrelated_sentinel (value TEXT NOT NULL);
         INSERT INTO unrelated_sentinel VALUES ('preserved');",
    )
    .execute(&mut connection)
    .await
    .expect("marker-only sentinel should initialize");
    let version_statement = format!("PRAGMA user_version = {version}");
    sqlx::query(sqlx::AssertSqlSafe(version_statement))
        .execute(&mut connection)
        .await
        .expect("legacy version marker should initialize");
    connection
        .close()
        .await
        .expect("marker-only fixture should close");
}

pub(super) async fn create_unrecognized_v10_missing_core_database(database_path: &Path) {
    execute_fixture_sql(
        database_path,
        "CREATE TABLE accounts (
            account_id TEXT PRIMARY KEY NOT NULL,
            label TEXT NOT NULL,
            status TEXT NOT NULL,
            active_credential_generation INTEGER
         );
         CREATE TABLE unrelated_sentinel (value TEXT NOT NULL);
         INSERT INTO accounts VALUES ('preserved-account', 'partial', 'disabled', 3);
         INSERT INTO unrelated_sentinel VALUES ('preserved');
         PRAGMA user_version = 10;",
    )
    .await;
}

pub(super) async fn create_v13_missing_accounts_database(database_path: &Path) {
    super::fixtures::create_legacy_v13_database(database_path, "missing-accounts").await;
    execute_fixture_sql(
        database_path,
        "DROP TABLE accounts;
         CREATE TABLE unrelated_sentinel (value TEXT NOT NULL);
         INSERT INTO unrelated_sentinel VALUES ('preserved');",
    )
    .await;
}

pub(super) async fn create_known_v10_missing_projection_database(database_path: &Path) {
    execute_fixture_sql(
        database_path,
        "CREATE TABLE accounts (
            account_id TEXT PRIMARY KEY NOT NULL,
            label TEXT NOT NULL,
            status TEXT NOT NULL,
            active_credential_generation INTEGER
         );
         CREATE TABLE quota_snapshots (
            account_id TEXT NOT NULL,
            source TEXT NOT NULL,
            observed_unix_seconds INTEGER NOT NULL,
            route_band TEXT NOT NULL,
            remaining_headroom INTEGER NOT NULL,
            reset_unix_seconds INTEGER,
            reset_credits_available INTEGER,
            stale_penalty INTEGER NOT NULL,
            PRIMARY KEY (account_id, route_band)
         );
         CREATE TABLE selector_quota_windows (
            account_id TEXT NOT NULL,
            route_band TEXT NOT NULL,
            limit_window_seconds INTEGER NOT NULL,
            status TEXT NOT NULL,
            remaining_headroom INTEGER NOT NULL,
            reset_unix_seconds INTEGER,
            effective INTEGER NOT NULL,
            observed_unix_seconds INTEGER NOT NULL,
            PRIMARY KEY (account_id, route_band, limit_window_seconds)
         );
         CREATE TABLE quota_refresh_status (
            account_id TEXT NOT NULL,
            route_band TEXT NOT NULL,
            last_success_unix_seconds INTEGER,
            last_attempt_unix_seconds INTEGER,
            last_error_class TEXT,
            stale_after_unix_seconds INTEGER,
            PRIMARY KEY (account_id, route_band)
         );
         CREATE TABLE unrelated_sentinel (value TEXT NOT NULL);
         INSERT INTO accounts VALUES ('preserved-account', 'projection', 'disabled', 9);
         INSERT INTO quota_snapshots VALUES (
            'preserved-account', 'mock_endpoint', 123, 'responses', 44, 999, 7, 0
         );
         INSERT INTO selector_quota_windows VALUES (
            'preserved-account', 'responses', 18000, 'eligible', 44, 999, 1, 123
         );
         INSERT INTO quota_refresh_status VALUES (
            'preserved-account', 'responses', 120, 123, NULL, 423
         );
         INSERT INTO unrelated_sentinel VALUES ('preserved');
         PRAGMA user_version = 10;",
    )
    .await;
}

pub(super) async fn create_known_v10_partial_history_database(database_path: &Path) {
    execute_fixture_sql(
        database_path,
        "CREATE TABLE active_client_leases (
            route_band TEXT NOT NULL,
            process_run_id TEXT NOT NULL,
            reservation_id TEXT NOT NULL,
            account_id TEXT NOT NULL,
            acquired_unix_seconds INTEGER NOT NULL,
            active_pressure INTEGER NOT NULL,
            PRIMARY KEY (route_band, process_run_id, reservation_id)
         );
         CREATE INDEX active_client_leases_account_lookup
            ON active_client_leases (route_band, account_id, acquired_unix_seconds);
         CREATE TABLE active_session_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            account_id TEXT NOT NULL,
            route_band TEXT NOT NULL,
            process_run_id TEXT NOT NULL,
            reservation_id TEXT NOT NULL,
            event_kind TEXT NOT NULL,
            event_unix_seconds INTEGER NOT NULL
         );
         CREATE INDEX active_session_events_route_lookup
            ON active_session_events (route_band, account_id, event_unix_seconds);
         CREATE TABLE active_session_rollups (
            account_id TEXT NOT NULL,
            route_band TEXT NOT NULL,
            bucket_start_unix_seconds INTEGER NOT NULL,
            bucket_end_unix_seconds INTEGER NOT NULL,
            active_session_seconds INTEGER NOT NULL,
            max_concurrent_sessions INTEGER NOT NULL,
            PRIMARY KEY (
                account_id, route_band, bucket_start_unix_seconds, bucket_end_unix_seconds
            )
         );
         CREATE TABLE unrelated_sentinel (value TEXT NOT NULL);
         INSERT INTO active_client_leases VALUES (
            'responses', 'partial-process', 'partial-reservation',
            'history-only-account', 123, 6
         );
         INSERT INTO active_session_events VALUES (
            41, 'history-only-account', 'responses', 'partial-process',
            'partial-reservation', 'acquired', 123
         );
         INSERT INTO active_session_rollups VALUES (
            'history-only-account', 'responses', 0, 300, 77, 2
         );
         INSERT INTO unrelated_sentinel VALUES ('preserved');
         PRAGMA user_version = 10;",
    )
    .await;
}
