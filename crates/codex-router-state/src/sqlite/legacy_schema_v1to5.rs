//! SQLite legacy schema v1to5 responsibilities.
use super::*;
impl SqliteStateStore {
    pub(super) fn apply_v1(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS accounts (
                    account_id TEXT PRIMARY KEY NOT NULL,
                    label TEXT NOT NULL,
                    status TEXT NOT NULL,
                    active_credential_generation INTEGER
                );

                CREATE TABLE IF NOT EXISTS quota_snapshots (
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

                CREATE TABLE IF NOT EXISTS affinity_pins (
                    affinity_key TEXT PRIMARY KEY NOT NULL,
                    account_id TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS selector_quota_windows (
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

                CREATE TABLE IF NOT EXISTS quota_refresh_status (
                    account_id TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    last_success_unix_seconds INTEGER,
                    last_attempt_unix_seconds INTEGER,
                    last_error_class TEXT,
                    stale_after_unix_seconds INTEGER,
                    PRIMARY KEY (account_id, route_band)
                );

                CREATE TABLE IF NOT EXISTS previous_response_affinity_owners (
                    affinity_key_hash TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    account_id TEXT NOT NULL,
                    credential_generation INTEGER NOT NULL,
                    source_transport TEXT NOT NULL,
                    created_unix_seconds INTEGER NOT NULL,
                    PRIMARY KEY (affinity_key_hash, route_band, account_id)
                );

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

                PRAGMA user_version = 10;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    pub(super) fn apply_v2(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                ALTER TABLE accounts ADD COLUMN active_credential_generation INTEGER;
                PRAGMA user_version = 2;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    pub(super) fn apply_v3(&self) -> Result<(), StateStoreError> {
        let [
            responses_route_band,
            models_route_band,
            memories_trace_summarize_route_band,
            responses_compact_route_band,
        ] = SELECTOR_INVALIDATED_ROUTE_BANDS;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        transaction
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS selector_quota_windows (
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
                ",
            )
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO selector_quota_windows (
                   account_id, route_band, limit_window_seconds, status,
                   remaining_headroom, reset_unix_seconds, effective,
                   observed_unix_seconds
                 )
                 SELECT
                   account_id,
                   route_band,
                   ?1,
                   CASE
                     WHEN remaining_headroom <= 0 THEN ?2
                     WHEN stale_penalty = 1 THEN ?3
                     ELSE ?4
                   END,
                   remaining_headroom,
                   reset_unix_seconds,
                   1,
                   observed_unix_seconds
                 FROM quota_snapshots
                 WHERE route_band IN (?5, ?6, ?7, ?8)
                 ON CONFLICT(account_id, route_band, limit_window_seconds) DO UPDATE SET
                   status = excluded.status,
                   remaining_headroom = excluded.remaining_headroom,
                   reset_unix_seconds = excluded.reset_unix_seconds,
                   effective = excluded.effective,
                   observed_unix_seconds = excluded.observed_unix_seconds",
                params![
                    u64_to_i64(DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS)?,
                    SelectorQuotaWindowStatus::Ineligible.as_str(),
                    SelectorQuotaWindowStatus::Stale.as_str(),
                    SelectorQuotaWindowStatus::Eligible.as_str(),
                    responses_route_band,
                    models_route_band,
                    memories_trace_summarize_route_band,
                    responses_compact_route_band,
                ],
            )
            .map_err(sqlite_error)?;
        transaction
            .execute_batch("PRAGMA user_version = 3;")
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    pub(super) fn apply_v4(&self) -> Result<(), StateStoreError> {
        let [
            responses_route_band,
            models_route_band,
            memories_trace_summarize_route_band,
            responses_compact_route_band,
        ] = SELECTOR_INVALIDATED_ROUTE_BANDS;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "DELETE FROM selector_quota_windows
                  WHERE route_band NOT IN (?1, ?2, ?3, ?4)",
                params![
                    responses_route_band,
                    models_route_band,
                    memories_trace_summarize_route_band,
                    responses_compact_route_band,
                ],
            )
            .map_err(sqlite_error)?;
        transaction
            .execute_batch("PRAGMA user_version = 4;")
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    pub(super) fn apply_v5(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS quota_refresh_status (
                    account_id TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    last_success_unix_seconds INTEGER,
                    last_attempt_unix_seconds INTEGER,
                    last_error_class TEXT,
                    stale_after_unix_seconds INTEGER,
                    PRIMARY KEY (account_id, route_band)
                );

                PRAGMA user_version = 5;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }
}
