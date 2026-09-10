use sqlx::SqliteConnection;

use crate::sqlite::StateStoreError;

mod definitions;
mod validation;

use definitions::*;
use validation::*;

pub(crate) enum LeaseShape {
    Missing,
    Current,
    VersionSeven,
}

pub(crate) struct LegacyShape {
    pub(crate) lease_shape: LeaseShape,
    pub(crate) missing_event_columns: Vec<&'static str>,
    pub(crate) missing_rollup_columns: Vec<&'static str>,
    pub(crate) policy_needs_rebuild: bool,
}

pub(crate) async fn validate_legacy_schema(
    connection: &mut SqliteConnection,
    version: i64,
) -> Result<LegacyShape, StateStoreError> {
    reject_leftover_replacement_tables(connection).await?;
    if version == 0 {
        validate_version_zero_prefix(connection).await?;
        return Ok(LegacyShape {
            lease_shape: LeaseShape::Missing,
            missing_event_columns: Vec::new(),
            missing_rollup_columns: Vec::new(),
            policy_needs_rebuild: false,
        });
    }
    if !(7..=13).contains(&version) {
        return Err(StateStoreError::UnsupportedSchemaVersion { version });
    }
    validate_legacy_base_presence(connection, version).await?;

    for (table_name, columns) in BASE_TABLES {
        if table_exists(connection, table_name).await? {
            validate_table(connection, table_name, columns).await?;
        }
    }
    let lease_shape = validate_lease_shape(connection, version).await?;
    validate_optional_exact_table(
        connection,
        "quota_history_observations",
        QUOTA_HISTORY_OBSERVATIONS,
    )
    .await?;
    validate_optional_exact_table(
        connection,
        "route_band_account_states",
        ROUTE_BAND_ACCOUNT_STATES,
    )
    .await?;
    let missing_event_columns = validate_history_table(
        connection,
        "active_session_events",
        ACTIVE_SESSION_EVENTS,
        &[
            "logical_session_id",
            "session_started_unix_seconds",
            "session_ended_unix_seconds",
            "transport_kind",
        ],
    )
    .await?;
    let missing_rollup_columns = validate_history_table(
        connection,
        "active_session_rollups",
        ACTIVE_SESSION_ROLLUPS,
        &["completed_sessions", "stale_purged_sessions"],
    )
    .await?;

    validate_named_indexes(connection).await?;
    let policy_needs_rebuild = validate_policy_guard(connection, version).await?;
    validate_affinity_guard(connection, version).await?;

    Ok(LegacyShape {
        lease_shape,
        missing_event_columns,
        missing_rollup_columns,
        policy_needs_rebuild,
    })
}

async fn validate_legacy_base_presence(
    connection: &mut SqliteConnection,
    version: i64,
) -> Result<(), StateStoreError> {
    let accounts = table_exists(connection, "accounts").await?;
    let quota_snapshots = table_exists(connection, "quota_snapshots").await?;
    let affinity_pins = table_exists(connection, "affinity_pins").await?;
    let selector_quota_windows = table_exists(connection, "selector_quota_windows").await?;
    let quota_refresh_status = table_exists(connection, "quota_refresh_status").await?;
    let previous_response_affinity_owners =
        table_exists(connection, "previous_response_affinity_owners").await?;

    if accounts
        && quota_snapshots
        && affinity_pins
        && selector_quota_windows
        && quota_refresh_status
        && previous_response_affinity_owners
    {
        return Ok(());
    }
    if version != 10 {
        return incompatible_schema();
    }

    // These are the two incomplete v10 layouts constructed by the historical async fixtures.
    let is_missing_projection_shape = accounts
        && quota_snapshots
        && !affinity_pins
        && selector_quota_windows
        && quota_refresh_status
        && !previous_response_affinity_owners;
    if is_missing_projection_shape {
        return Ok(());
    }

    let has_no_base_tables = !accounts
        && !quota_snapshots
        && !affinity_pins
        && !selector_quota_windows
        && !quota_refresh_status
        && !previous_response_affinity_owners;
    let is_partial_history_shape = has_no_base_tables
        && table_exists(connection, "active_client_leases").await?
        && table_exists(connection, "active_session_events").await?
        && table_exists(connection, "active_session_rollups").await?;
    if is_partial_history_shape {
        return Ok(());
    }

    incompatible_schema()
}

pub(crate) async fn validate_target_schema(
    connection: &mut SqliteConnection,
) -> Result<(), StateStoreError> {
    for (table_name, columns) in BASE_TABLES {
        validate_table(connection, table_name, columns).await?;
    }
    for (table_name, columns) in [
        ("quota_history_observations", QUOTA_HISTORY_OBSERVATIONS),
        ("active_client_leases", ACTIVE_CLIENT_LEASES),
        ("route_band_account_states", ROUTE_BAND_ACCOUNT_STATES),
        ("active_session_events", ACTIVE_SESSION_EVENTS),
        ("active_session_rollups", ACTIVE_SESSION_ROLLUPS),
        ("account_routing_policies", ACCOUNT_ROUTING_POLICIES),
        ("session_account_affinities", SESSION_ACCOUNT_AFFINITIES),
    ] {
        validate_table(connection, table_name, columns).await?;
    }
    validate_policy_constraint(connection, false).await?;
    validate_all_required_indexes(connection).await
}

pub(crate) async fn validate_required_read_only_objects(
    connection: &mut SqliteConnection,
) -> Result<(), StateStoreError> {
    for table_name in [
        "accounts",
        "quota_snapshots",
        "selector_quota_windows",
        "quota_refresh_status",
        "quota_history_observations",
        "active_client_leases",
        "route_band_account_states",
        "active_session_events",
        "active_session_rollups",
        "account_routing_policies",
        "session_account_affinities",
    ] {
        if !table_exists(connection, table_name).await? {
            return Err(StateStoreError::MissingReadOnlySchemaObject {
                object_kind: "table",
                object_name: table_name,
            });
        }
    }
    for (table_name, column_name) in [
        ("active_client_leases", "process_run_id"),
        ("active_client_leases", "active_pressure"),
        ("active_session_events", "logical_session_id"),
        ("active_session_events", "session_started_unix_seconds"),
        ("active_session_events", "session_ended_unix_seconds"),
        ("active_session_events", "transport_kind"),
        ("active_session_rollups", "completed_sessions"),
        ("active_session_rollups", "stale_purged_sessions"),
        ("account_routing_policies", "account_id"),
        (
            "account_routing_policies",
            "weekly_quota_floor_basis_points",
        ),
        ("session_account_affinities", "session_id"),
        ("session_account_affinities", "account_id"),
        ("session_account_affinities", "last_seen_unix_seconds"),
    ] {
        if !load_columns(connection, table_name)
            .await?
            .contains_key(column_name)
        {
            return Err(StateStoreError::MissingReadOnlySchemaObject {
                object_kind: "column",
                object_name: column_name,
            });
        }
    }
    Ok(())
}

async fn validate_version_zero_prefix(
    connection: &mut SqliteConnection,
) -> Result<(), StateStoreError> {
    let mut missing_seen = false;
    for (table_name, columns) in BASE_TABLES {
        if table_exists(connection, table_name).await? {
            if missing_seen {
                return incompatible_schema();
            }
            validate_table(connection, table_name, columns).await?;
        } else {
            missing_seen = true;
        }
    }
    for table_name in [
        "quota_history_observations",
        "active_client_leases",
        "route_band_account_states",
        "active_session_events",
        "active_session_rollups",
        "account_routing_policies",
        "session_account_affinities",
    ] {
        if table_exists(connection, table_name).await? {
            return incompatible_schema();
        }
    }
    Ok(())
}

async fn validate_lease_shape(
    connection: &mut SqliteConnection,
    version: i64,
) -> Result<LeaseShape, StateStoreError> {
    if !table_exists(connection, "active_client_leases").await? {
        return Ok(LeaseShape::Missing);
    }
    if table_matches(connection, "active_client_leases", ACTIVE_CLIENT_LEASES).await? {
        return Ok(LeaseShape::Current);
    }
    if version == 7
        && table_matches(connection, "active_client_leases", ACTIVE_CLIENT_LEASES_V7).await?
    {
        reject_unexpected_dependents(
            connection,
            "active_client_leases",
            &["active_client_leases_account_lookup"],
        )
        .await?;
        return Ok(LeaseShape::VersionSeven);
    }
    incompatible_schema()
}

async fn validate_history_table(
    connection: &mut SqliteConnection,
    table_name: &'static str,
    final_columns: &'static [ColumnSpec],
    optional_columns: &[&'static str],
) -> Result<Vec<&'static str>, StateStoreError> {
    if !table_exists(connection, table_name).await? {
        return Ok(Vec::new());
    }
    let actual = load_columns(connection, table_name).await?;
    let mut missing = Vec::new();
    for expected in final_columns {
        match actual.get(expected.name) {
            Some(column) => validate_column(column, expected)?,
            None if optional_columns.contains(&expected.name) => missing.push(expected.name),
            None => return incompatible_schema(),
        }
    }
    if actual.len() + missing.len() != final_columns.len() {
        return incompatible_schema();
    }
    Ok(missing)
}

async fn validate_policy_guard(
    connection: &mut SqliteConnection,
    version: i64,
) -> Result<bool, StateStoreError> {
    let exists = table_exists(connection, "account_routing_policies").await?;
    match version {
        7..=10 if exists => incompatible_schema(),
        7..=10 => Ok(false),
        11 if !exists => incompatible_schema(),
        11 => {
            validate_table(
                connection,
                "account_routing_policies",
                ACCOUNT_ROUTING_POLICIES,
            )
            .await?;
            validate_policy_constraint(connection, true).await?;
            reject_unexpected_dependents(connection, "account_routing_policies", &[]).await?;
            Ok(true)
        }
        12 | 13 if !exists => incompatible_schema(),
        12 | 13 => {
            validate_table(
                connection,
                "account_routing_policies",
                ACCOUNT_ROUTING_POLICIES,
            )
            .await?;
            validate_policy_constraint(connection, false).await?;
            Ok(false)
        }
        _ => incompatible_schema(),
    }
}

async fn validate_affinity_guard(
    connection: &mut SqliteConnection,
    version: i64,
) -> Result<(), StateStoreError> {
    let exists = table_exists(connection, "session_account_affinities").await?;
    match version {
        7..=12 if exists => incompatible_schema(),
        7..=12 => Ok(()),
        13 if !exists => incompatible_schema(),
        13 => {
            validate_table(
                connection,
                "session_account_affinities",
                SESSION_ACCOUNT_AFFINITIES,
            )
            .await
        }
        _ => incompatible_schema(),
    }
}

async fn validate_policy_constraint(
    connection: &mut SqliteConnection,
    old_constraint: bool,
) -> Result<(), StateStoreError> {
    let sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'account_routing_policies'",
    )
    .fetch_one(connection)
    .await
    .map_err(super::sqlite::sqlx_error)?;
    let tokens = schema_tokens(&sql);
    let maximum = if old_constraint { "1000" } else { "1500" };
    let expected_check = [
        "check",
        "(",
        "weekly_quota_floor_basis_points",
        "between",
        "100",
        "and",
        maximum,
        "and",
        "weekly_quota_floor_basis_points",
        "%",
        "100",
        "=",
        "0",
        ")",
    ];
    let Some(check_index) = tokens.iter().position(|token| token == "check") else {
        return incompatible_schema();
    };
    if tokens
        .get(check_index..)
        .is_some_and(|suffix| suffix.starts_with(&expected_check.map(str::to_owned)))
        && tokens
            .get(check_index + expected_check.len())
            .map(String::as_str)
            == Some(")")
        && check_index + expected_check.len() + 1 == tokens.len()
    {
        Ok(())
    } else {
        incompatible_schema()
    }
}
