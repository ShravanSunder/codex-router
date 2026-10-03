//! SQLite legacy schema v6to10 responsibilities.
use super::*;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) const ASYNC_QUOTA_HISTORY_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS quota_history_observations (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
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
    )",
    "CREATE INDEX IF NOT EXISTS quota_history_window_lookup
        ON quota_history_observations (
            account_id, route_band, limit_window_seconds, observed_unix_seconds
        )",
];

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) const ASYNC_CREDIT_USAGE_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS account_credit_policies (
        account_id TEXT PRIMARY KEY NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
        allow_credits INTEGER NOT NULL CHECK (allow_credits IN (0, 1))
    )",
    "CREATE TABLE IF NOT EXISTS account_credit_observations (
        account_id TEXT PRIMARY KEY NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
        credential_generation INTEGER NOT NULL,
        latest_started_attempt INTEGER NOT NULL,
        committed_attempt INTEGER,
        observed_unix_seconds INTEGER,
        stale_after_unix_seconds INTEGER,
        availability TEXT NOT NULL,
        balance TEXT,
        spend_control_state TEXT NOT NULL,
        provider_limit_reason TEXT
    )",
];

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) const ASYNC_ACTIVE_CLIENT_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS active_client_leases (
        route_band TEXT NOT NULL,
        process_run_id TEXT NOT NULL,
        reservation_id TEXT NOT NULL,
        account_id TEXT NOT NULL,
        acquired_unix_seconds INTEGER NOT NULL,
        active_pressure INTEGER NOT NULL,
        PRIMARY KEY (route_band, process_run_id, reservation_id)
    )",
    "CREATE INDEX IF NOT EXISTS active_client_leases_account_lookup
        ON active_client_leases (
            route_band, account_id, acquired_unix_seconds
        )",
];

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) const ASYNC_ROUTE_BAND_ACCOUNT_STATE_SCHEMA_STATEMENTS: &[&str] =
    &["CREATE TABLE IF NOT EXISTS route_band_account_states (
        account_id TEXT NOT NULL,
        route_band TEXT NOT NULL,
        state TEXT NOT NULL,
        reason_code TEXT NOT NULL,
        observed_unix_seconds INTEGER NOT NULL,
        expires_unix_seconds INTEGER,
        PRIMARY KEY (account_id, route_band)
    )"];

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) const ASYNC_ACTIVE_SESSION_HISTORY_TABLE_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS active_session_events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        account_id TEXT NOT NULL,
        route_band TEXT NOT NULL,
        process_run_id TEXT NOT NULL,
        logical_session_id TEXT NOT NULL,
        reservation_id TEXT NOT NULL,
        event_kind TEXT NOT NULL,
        event_unix_seconds INTEGER NOT NULL,
        session_started_unix_seconds INTEGER NOT NULL,
        session_ended_unix_seconds INTEGER,
        transport_kind TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS active_session_rollups (
        account_id TEXT NOT NULL,
        route_band TEXT NOT NULL,
        bucket_start_unix_seconds INTEGER NOT NULL,
        bucket_end_unix_seconds INTEGER NOT NULL,
        active_session_seconds INTEGER NOT NULL,
        max_concurrent_sessions INTEGER NOT NULL,
        completed_sessions INTEGER NOT NULL,
        stale_purged_sessions INTEGER NOT NULL,
        PRIMARY KEY (account_id, route_band, bucket_start_unix_seconds, bucket_end_unix_seconds)
    )",
];

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) const ASYNC_ACTIVE_SESSION_HISTORY_INDEX_STATEMENTS: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS active_session_events_route_lookup
        ON active_session_events (
            route_band, account_id, event_unix_seconds
        )",
    "CREATE INDEX IF NOT EXISTS active_session_events_terminal_interval_lookup
        ON active_session_events (
            route_band, event_kind, session_started_unix_seconds
        )",
    "CREATE INDEX IF NOT EXISTS active_session_events_reservation_terminal_lookup
        ON active_session_events (
            route_band, process_run_id, reservation_id, event_kind, event_unix_seconds
        )",
];

impl SqliteStateStore {
    pub(super) fn apply_v6(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS previous_response_affinity_owners (
                    affinity_key_hash TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    account_id TEXT NOT NULL,
                    credential_generation INTEGER NOT NULL,
                    source_transport TEXT NOT NULL,
                    created_unix_seconds INTEGER NOT NULL,
                    PRIMARY KEY (affinity_key_hash, route_band, account_id)
                );

                PRAGMA user_version = 6;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    pub(super) fn apply_v7(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                ALTER TABLE quota_snapshots ADD COLUMN reset_credits_available INTEGER;
                PRAGMA user_version = 7;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    pub(super) fn apply_v8(&self) -> Result<(), StateStoreError> {
        if !self.table_exists("active_client_leases")? {
            self.connection
                .execute_batch(
                    "
                    CREATE TABLE IF NOT EXISTS active_client_leases (
                        route_band TEXT NOT NULL,
                        process_run_id TEXT NOT NULL,
                        reservation_id TEXT NOT NULL,
                        account_id TEXT NOT NULL,
                        acquired_unix_seconds INTEGER NOT NULL,
                        active_pressure INTEGER NOT NULL,
                        PRIMARY KEY (route_band, process_run_id, reservation_id)
                    );

                    CREATE INDEX IF NOT EXISTS active_client_leases_account_lookup
                        ON active_client_leases (
                            route_band, account_id, acquired_unix_seconds
                        );

                    PRAGMA user_version = 8;
                    ",
                )
                .map_err(sqlite_error)?;
            return Ok(());
        }

        if self.table_has_column("active_client_leases", "process_run_id")? {
            self.connection
                .execute_batch("PRAGMA user_version = 8;")
                .map_err(sqlite_error)?;
            return Ok(());
        }

        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        transaction
            .execute_batch(
                "
                DROP INDEX IF EXISTS active_client_leases_account_lookup;

                CREATE TABLE active_client_leases_v8 (
                    route_band TEXT NOT NULL,
                    process_run_id TEXT NOT NULL,
                    reservation_id TEXT NOT NULL,
                    account_id TEXT NOT NULL,
                    acquired_unix_seconds INTEGER NOT NULL,
                    active_pressure INTEGER NOT NULL,
                    PRIMARY KEY (route_band, process_run_id, reservation_id)
                );

                INSERT OR IGNORE INTO active_client_leases_v8 (
                    route_band, process_run_id, reservation_id, account_id,
                    acquired_unix_seconds, active_pressure
                )
                SELECT
                    route_band, 'legacy', reservation_id, account_id,
                    acquired_unix_seconds, 8
                FROM active_client_leases;

                DROP TABLE active_client_leases;
                ALTER TABLE active_client_leases_v8 RENAME TO active_client_leases;

                CREATE INDEX IF NOT EXISTS active_client_leases_account_lookup
                    ON active_client_leases (
                        route_band, account_id, acquired_unix_seconds
                    );

                PRAGMA user_version = 8;
                ",
            )
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    pub(super) fn apply_v9(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS active_session_events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    account_id TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    process_run_id TEXT NOT NULL,
                    logical_session_id TEXT NOT NULL,
                    reservation_id TEXT NOT NULL,
                    event_kind TEXT NOT NULL,
                    event_unix_seconds INTEGER NOT NULL,
                    session_started_unix_seconds INTEGER NOT NULL,
                    session_ended_unix_seconds INTEGER,
                    transport_kind TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS active_session_events_route_lookup
                    ON active_session_events (
                        route_band, account_id, event_unix_seconds
                    );

                CREATE INDEX IF NOT EXISTS active_session_events_terminal_interval_lookup
                    ON active_session_events (
                        route_band, event_kind, session_started_unix_seconds
                    );

                CREATE INDEX IF NOT EXISTS active_session_events_reservation_terminal_lookup
                    ON active_session_events (
                        route_band, process_run_id, reservation_id, event_kind, event_unix_seconds
                    );

                CREATE TABLE IF NOT EXISTS active_session_rollups (
                    account_id TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    bucket_start_unix_seconds INTEGER NOT NULL,
                    bucket_end_unix_seconds INTEGER NOT NULL,
                    active_session_seconds INTEGER NOT NULL,
                    max_concurrent_sessions INTEGER NOT NULL,
                    completed_sessions INTEGER NOT NULL,
                    stale_purged_sessions INTEGER NOT NULL,
                    PRIMARY KEY (
                        account_id,
                        route_band,
                        bucket_start_unix_seconds,
                        bucket_end_unix_seconds
                    )
                );

                PRAGMA user_version = 9;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    pub(super) fn apply_v10(&self) -> Result<(), StateStoreError> {
        for (table_name, column_name, column_definition) in [
            (
                "active_session_events",
                "logical_session_id",
                "logical_session_id TEXT NOT NULL DEFAULT ''",
            ),
            (
                "active_session_events",
                "session_started_unix_seconds",
                "session_started_unix_seconds INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "active_session_events",
                "session_ended_unix_seconds",
                "session_ended_unix_seconds INTEGER",
            ),
            (
                "active_session_events",
                "transport_kind",
                "transport_kind TEXT NOT NULL DEFAULT 'unknown'",
            ),
            (
                "active_session_rollups",
                "completed_sessions",
                "completed_sessions INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "active_session_rollups",
                "stale_purged_sessions",
                "stale_purged_sessions INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            if !self.table_has_column(table_name, column_name)? {
                self.connection
                    .execute(
                        &format!("ALTER TABLE {table_name} ADD COLUMN {column_definition}"),
                        [],
                    )
                    .map_err(sqlite_error)?;
            }
        }
        self.connection
            .execute_batch("PRAGMA user_version = 10;")
            .map_err(sqlite_error)?;

        Ok(())
    }
}
