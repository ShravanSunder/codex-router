use std::path::Path;

use rusqlite::Connection;

pub(super) fn create_v8_database_with_current_active_lease(database_path: &Path) {
    let connection = match Connection::open(database_path) {
        Ok(connection) => connection,
        Err(error) => panic!("raw v8 database should open: {error}"),
    };
    if let Err(error) = connection.execute_batch(
        "
            CREATE TABLE accounts (
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

            CREATE TABLE affinity_pins (
                affinity_key TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL
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

            CREATE TABLE previous_response_affinity_owners (
                affinity_key_hash TEXT NOT NULL,
                route_band TEXT NOT NULL,
                account_id TEXT NOT NULL,
                credential_generation INTEGER NOT NULL,
                source_transport TEXT NOT NULL,
                created_unix_seconds INTEGER NOT NULL,
                PRIMARY KEY (affinity_key_hash, route_band, account_id)
            );

            CREATE TABLE active_client_leases (
                route_band TEXT NOT NULL,
                process_run_id TEXT NOT NULL,
                reservation_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                acquired_unix_seconds INTEGER NOT NULL,
                active_pressure INTEGER NOT NULL,
                PRIMARY KEY (route_band, process_run_id, reservation_id)
            );

            CREATE INDEX active_client_leases_account_lookup
                ON active_client_leases (
                    route_band, account_id, acquired_unix_seconds
                );

            INSERT INTO active_client_leases (
                route_band, process_run_id, reservation_id, account_id,
                acquired_unix_seconds, active_pressure
            ) VALUES (
                'responses', 'process-v8', 'reservation-v8',
                'acct_v8_active_lease', 100, 8
            );

            PRAGMA user_version = 8;
            ",
    ) {
        panic!("raw v8 database should initialize: {error}");
    }
}

pub(super) fn create_v10_database_missing_async_projection_tables(database_path: &Path) {
    let connection = match Connection::open(database_path) {
        Ok(connection) => connection,
        Err(error) => panic!("raw v10 database should open: {error}"),
    };
    if let Err(error) = connection.execute_batch(
        "
            CREATE TABLE accounts (
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

            PRAGMA user_version = 10;
            ",
    ) {
        panic!("raw v10 database should initialize without async projection tables: {error}");
    }
}

pub(super) fn create_partial_v10_database_missing_active_session_columns(database_path: &Path) {
    let connection = match Connection::open(database_path) {
        Ok(connection) => connection,
        Err(error) => panic!("raw partial v10 database should open: {error}"),
    };
    if let Err(error) = connection.execute_batch(
        "
            CREATE TABLE active_client_leases (
                route_band TEXT NOT NULL,
                process_run_id TEXT NOT NULL,
                reservation_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                acquired_unix_seconds INTEGER NOT NULL,
                active_pressure INTEGER NOT NULL,
                PRIMARY KEY (route_band, process_run_id, reservation_id)
            );

            CREATE INDEX active_client_leases_account_lookup
                ON active_client_leases (
                    route_band, account_id, acquired_unix_seconds
                );

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
                ON active_session_events (
                    route_band, account_id, event_unix_seconds
                );

            CREATE TABLE active_session_rollups (
                account_id TEXT NOT NULL,
                route_band TEXT NOT NULL,
                bucket_start_unix_seconds INTEGER NOT NULL,
                bucket_end_unix_seconds INTEGER NOT NULL,
                active_session_seconds INTEGER NOT NULL,
                max_concurrent_sessions INTEGER NOT NULL,
                PRIMARY KEY (
                    account_id,
                    route_band,
                    bucket_start_unix_seconds,
                    bucket_end_unix_seconds
                )
            );

            PRAGMA user_version = 10;
            ",
    ) {
        panic!("raw partial v10 database should initialize: {error}");
    }
}
