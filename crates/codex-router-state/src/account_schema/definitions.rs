pub(super) const INCOMPATIBLE_SCHEMA_MESSAGE: &str = "incompatible account database schema";

#[derive(Clone, Copy)]
pub(super) struct ColumnSpec {
    pub(super) name: &'static str,
    pub(super) declared_type: &'static str,
    pub(super) not_null: bool,
    pub(super) primary_key_position: i64,
    pub(super) accepted_defaults: &'static [Option<&'static str>],
}

pub(super) const NO_DEFAULT: &[Option<&str>] = &[None];
pub(super) const NO_OR_EMPTY_TEXT_DEFAULT: &[Option<&str>] = &[None, Some("''")];
pub(super) const NO_OR_ZERO_DEFAULT: &[Option<&str>] = &[None, Some("0")];
pub(super) const NO_OR_UNKNOWN_DEFAULT: &[Option<&str>] = &[None, Some("'unknown'")];

macro_rules! column {
    ($name:literal, $declared_type:literal, $not_null:literal, $pk:literal) => {
        ColumnSpec {
            name: $name,
            declared_type: $declared_type,
            not_null: $not_null,
            primary_key_position: $pk,
            accepted_defaults: NO_DEFAULT,
        }
    };
}

pub(super) const ACCOUNTS: &[ColumnSpec] = &[
    column!("account_id", "TEXT", true, 1),
    column!("label", "TEXT", true, 0),
    column!("status", "TEXT", true, 0),
    column!("active_credential_generation", "INTEGER", false, 0),
];
pub(super) const QUOTA_SNAPSHOTS: &[ColumnSpec] = &[
    column!("account_id", "TEXT", true, 1),
    column!("source", "TEXT", true, 0),
    column!("observed_unix_seconds", "INTEGER", true, 0),
    column!("route_band", "TEXT", true, 2),
    column!("remaining_headroom", "INTEGER", true, 0),
    column!("reset_unix_seconds", "INTEGER", false, 0),
    column!("reset_credits_available", "INTEGER", false, 0),
    column!("stale_penalty", "INTEGER", true, 0),
];
pub(super) const AFFINITY_PINS: &[ColumnSpec] = &[
    column!("affinity_key", "TEXT", true, 1),
    column!("account_id", "TEXT", true, 0),
];
pub(super) const SELECTOR_QUOTA_WINDOWS: &[ColumnSpec] = &[
    column!("account_id", "TEXT", true, 1),
    column!("route_band", "TEXT", true, 2),
    column!("limit_window_seconds", "INTEGER", true, 3),
    column!("status", "TEXT", true, 0),
    column!("remaining_headroom", "INTEGER", true, 0),
    column!("reset_unix_seconds", "INTEGER", false, 0),
    column!("effective", "INTEGER", true, 0),
    column!("observed_unix_seconds", "INTEGER", true, 0),
];
pub(super) const QUOTA_REFRESH_STATUS: &[ColumnSpec] = &[
    column!("account_id", "TEXT", true, 1),
    column!("route_band", "TEXT", true, 2),
    column!("last_success_unix_seconds", "INTEGER", false, 0),
    column!("last_attempt_unix_seconds", "INTEGER", false, 0),
    column!("last_error_class", "TEXT", false, 0),
    column!("stale_after_unix_seconds", "INTEGER", false, 0),
];
pub(super) const PREVIOUS_RESPONSE_AFFINITY_OWNERS: &[ColumnSpec] = &[
    column!("affinity_key_hash", "TEXT", true, 1),
    column!("route_band", "TEXT", true, 2),
    column!("account_id", "TEXT", true, 3),
    column!("credential_generation", "INTEGER", true, 0),
    column!("source_transport", "TEXT", true, 0),
    column!("created_unix_seconds", "INTEGER", true, 0),
];
pub(super) const QUOTA_HISTORY_OBSERVATIONS: &[ColumnSpec] = &[
    column!("id", "INTEGER", false, 1),
    column!("account_id", "TEXT", true, 0),
    column!("account_label", "TEXT", true, 0),
    column!("route_band", "TEXT", true, 0),
    column!("limit_window_seconds", "INTEGER", true, 0),
    column!("observed_unix_seconds", "INTEGER", true, 0),
    column!("remaining_headroom", "INTEGER", true, 0),
    column!("reset_unix_seconds", "INTEGER", false, 0),
    column!("window_status", "TEXT", true, 0),
    column!("effective", "INTEGER", true, 0),
    column!("refresh_source", "TEXT", true, 0),
    column!("refresh_success", "INTEGER", true, 0),
    column!("refresh_error_class", "TEXT", false, 0),
    column!("reset_credits_available", "INTEGER", false, 0),
];
pub(super) const ACTIVE_CLIENT_LEASES: &[ColumnSpec] = &[
    column!("route_band", "TEXT", true, 1),
    column!("process_run_id", "TEXT", true, 2),
    column!("reservation_id", "TEXT", true, 3),
    column!("account_id", "TEXT", true, 0),
    column!("acquired_unix_seconds", "INTEGER", true, 0),
    column!("active_pressure", "INTEGER", true, 0),
];
pub(super) const ACTIVE_CLIENT_LEASES_V7: &[ColumnSpec] = &[
    column!("route_band", "TEXT", true, 1),
    column!("reservation_id", "TEXT", true, 2),
    column!("account_id", "TEXT", true, 0),
    column!("acquired_unix_seconds", "INTEGER", true, 0),
];
pub(super) const ROUTE_BAND_ACCOUNT_STATES: &[ColumnSpec] = &[
    column!("account_id", "TEXT", true, 1),
    column!("route_band", "TEXT", true, 2),
    column!("state", "TEXT", true, 0),
    column!("reason_code", "TEXT", true, 0),
    column!("observed_unix_seconds", "INTEGER", true, 0),
    column!("expires_unix_seconds", "INTEGER", false, 0),
];
pub(super) const ACTIVE_SESSION_EVENTS: &[ColumnSpec] = &[
    column!("id", "INTEGER", false, 1),
    column!("account_id", "TEXT", true, 0),
    column!("route_band", "TEXT", true, 0),
    column!("process_run_id", "TEXT", true, 0),
    ColumnSpec {
        name: "logical_session_id",
        declared_type: "TEXT",
        not_null: true,
        primary_key_position: 0,
        accepted_defaults: NO_OR_EMPTY_TEXT_DEFAULT,
    },
    column!("reservation_id", "TEXT", true, 0),
    column!("event_kind", "TEXT", true, 0),
    column!("event_unix_seconds", "INTEGER", true, 0),
    ColumnSpec {
        name: "session_started_unix_seconds",
        declared_type: "INTEGER",
        not_null: true,
        primary_key_position: 0,
        accepted_defaults: NO_OR_ZERO_DEFAULT,
    },
    column!("session_ended_unix_seconds", "INTEGER", false, 0),
    ColumnSpec {
        name: "transport_kind",
        declared_type: "TEXT",
        not_null: true,
        primary_key_position: 0,
        accepted_defaults: NO_OR_UNKNOWN_DEFAULT,
    },
];
pub(super) const ACTIVE_SESSION_ROLLUPS: &[ColumnSpec] = &[
    column!("account_id", "TEXT", true, 1),
    column!("route_band", "TEXT", true, 2),
    column!("bucket_start_unix_seconds", "INTEGER", true, 3),
    column!("bucket_end_unix_seconds", "INTEGER", true, 4),
    column!("active_session_seconds", "INTEGER", true, 0),
    column!("max_concurrent_sessions", "INTEGER", true, 0),
    ColumnSpec {
        name: "completed_sessions",
        declared_type: "INTEGER",
        not_null: true,
        primary_key_position: 0,
        accepted_defaults: NO_OR_ZERO_DEFAULT,
    },
    ColumnSpec {
        name: "stale_purged_sessions",
        declared_type: "INTEGER",
        not_null: true,
        primary_key_position: 0,
        accepted_defaults: NO_OR_ZERO_DEFAULT,
    },
];
pub(super) const ACCOUNT_ROUTING_POLICIES: &[ColumnSpec] = &[
    column!("account_id", "TEXT", true, 1),
    column!("weekly_quota_floor_basis_points", "INTEGER", true, 0),
];
pub(super) const SESSION_ACCOUNT_AFFINITIES: &[ColumnSpec] = &[
    column!("session_id", "TEXT", true, 1),
    column!("account_id", "TEXT", true, 0),
    column!("last_seen_unix_seconds", "INTEGER", true, 0),
];

pub(super) const BASE_TABLES: &[(&str, &[ColumnSpec])] = &[
    ("accounts", ACCOUNTS),
    ("quota_snapshots", QUOTA_SNAPSHOTS),
    ("affinity_pins", AFFINITY_PINS),
    ("selector_quota_windows", SELECTOR_QUOTA_WINDOWS),
    ("quota_refresh_status", QUOTA_REFRESH_STATUS),
    (
        "previous_response_affinity_owners",
        PREVIOUS_RESPONSE_AFFINITY_OWNERS,
    ),
];
