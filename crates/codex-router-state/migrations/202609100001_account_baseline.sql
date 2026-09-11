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

CREATE TABLE IF NOT EXISTS quota_history_observations (
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
);

CREATE INDEX IF NOT EXISTS quota_history_window_lookup
    ON quota_history_observations (
        account_id, route_band, limit_window_seconds, observed_unix_seconds
    );

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
    ON active_client_leases (route_band, account_id, acquired_unix_seconds);

CREATE TABLE IF NOT EXISTS route_band_account_states (
    account_id TEXT NOT NULL,
    route_band TEXT NOT NULL,
    state TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    observed_unix_seconds INTEGER NOT NULL,
    expires_unix_seconds INTEGER,
    PRIMARY KEY (account_id, route_band)
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
    ON active_session_events (route_band, account_id, event_unix_seconds);

CREATE INDEX IF NOT EXISTS active_session_events_terminal_interval_lookup
    ON active_session_events (route_band, event_kind, session_started_unix_seconds);

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

CREATE TABLE IF NOT EXISTS account_routing_policies (
    account_id TEXT PRIMARY KEY NOT NULL,
    weekly_quota_floor_basis_points INTEGER NOT NULL
        CHECK (
            weekly_quota_floor_basis_points BETWEEN 100 AND 1500
            AND weekly_quota_floor_basis_points % 100 = 0
        )
);

CREATE TABLE IF NOT EXISTS session_account_affinities (
    session_id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL,
    last_seen_unix_seconds INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS session_account_affinities_last_seen_lookup
    ON session_account_affinities (last_seen_unix_seconds);

PRAGMA user_version = 13;
