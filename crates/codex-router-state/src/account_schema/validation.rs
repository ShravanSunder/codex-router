use std::collections::BTreeMap;

use sqlx::Row;
use sqlx::SqliteConnection;

use super::definitions::*;
use crate::sqlite::StateStoreError;

pub(super) async fn validate_named_indexes(
    connection: &mut SqliteConnection,
) -> Result<(), StateStoreError> {
    for (name, table, columns) in required_indexes() {
        if index_exists(connection, name).await? {
            validate_index(connection, name, table, columns).await?;
        }
    }
    Ok(())
}

pub(super) async fn validate_all_required_indexes(
    connection: &mut SqliteConnection,
) -> Result<(), StateStoreError> {
    for (name, table, columns) in required_indexes() {
        validate_index(connection, name, table, columns).await?;
    }
    Ok(())
}

pub(super) fn required_indexes() -> &'static [(&'static str, &'static str, &'static [&'static str])]
{
    &[
        (
            "quota_history_window_lookup",
            "quota_history_observations",
            &[
                "account_id",
                "route_band",
                "limit_window_seconds",
                "observed_unix_seconds",
            ],
        ),
        (
            "active_client_leases_account_lookup",
            "active_client_leases",
            &["route_band", "account_id", "acquired_unix_seconds"],
        ),
        (
            "active_session_events_route_lookup",
            "active_session_events",
            &["route_band", "account_id", "event_unix_seconds"],
        ),
        (
            "active_session_events_terminal_interval_lookup",
            "active_session_events",
            &["route_band", "event_kind", "session_started_unix_seconds"],
        ),
        (
            "active_session_events_reservation_terminal_lookup",
            "active_session_events",
            &[
                "route_band",
                "process_run_id",
                "reservation_id",
                "event_kind",
                "event_unix_seconds",
            ],
        ),
        (
            "session_account_affinities_last_seen_lookup",
            "session_account_affinities",
            &["last_seen_unix_seconds"],
        ),
    ]
}

pub(super) async fn validate_optional_exact_table(
    connection: &mut SqliteConnection,
    table_name: &'static str,
    columns: &'static [ColumnSpec],
) -> Result<(), StateStoreError> {
    if table_exists(connection, table_name).await? {
        validate_table(connection, table_name, columns).await?;
    }
    Ok(())
}

pub(super) async fn validate_table(
    connection: &mut SqliteConnection,
    table_name: &'static str,
    expected: &'static [ColumnSpec],
) -> Result<(), StateStoreError> {
    if !table_matches(connection, table_name, expected).await? {
        return incompatible_schema();
    }
    validate_table_constraints(connection, table_name).await
}

pub(super) async fn table_matches(
    connection: &mut SqliteConnection,
    table_name: &'static str,
    expected: &'static [ColumnSpec],
) -> Result<bool, StateStoreError> {
    if !table_exists(connection, table_name).await? {
        return Ok(false);
    }
    let actual = load_columns(connection, table_name).await?;
    if actual.len() != expected.len() {
        return Ok(false);
    }
    for column in expected {
        let Some(actual_column) = actual.get(column.name) else {
            return Ok(false);
        };
        if validate_column(actual_column, column).is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) struct ActualColumn {
    declared_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
}

pub(super) async fn load_columns(
    connection: &mut SqliteConnection,
    table_name: &'static str,
) -> Result<BTreeMap<String, ActualColumn>, StateStoreError> {
    let query = format!("PRAGMA table_info(\"{table_name}\")");
    let rows = sqlx::query(sqlx::AssertSqlSafe(query))
        .fetch_all(connection)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("name"),
                ActualColumn {
                    declared_type: row.get::<String, _>("type"),
                    not_null: row.get::<i64, _>("notnull") != 0,
                    default_value: row.get::<Option<String>, _>("dflt_value"),
                    primary_key_position: row.get::<i64, _>("pk"),
                },
            )
        })
        .collect())
}

pub(super) fn validate_column(
    actual: &ActualColumn,
    expected: &ColumnSpec,
) -> Result<(), StateStoreError> {
    let normalized_default = actual.default_value.as_deref().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.eq_ignore_ascii_case("NULL")).then_some(trimmed)
    });
    let default_matches = expected.accepted_defaults.contains(&normalized_default);
    if actual
        .declared_type
        .eq_ignore_ascii_case(expected.declared_type)
        && actual.not_null == expected.not_null
        && actual.primary_key_position == expected.primary_key_position
        && default_matches
    {
        Ok(())
    } else {
        incompatible_schema()
    }
}

pub(super) async fn table_exists(
    connection: &mut SqliteConnection,
    table_name: &str,
) -> Result<bool, StateStoreError> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
    )
    .bind(table_name)
    .fetch_one(connection)
    .await
    .map_err(crate::sqlite::sqlx_error)
}

pub(super) async fn index_exists(
    connection: &mut SqliteConnection,
    index_name: &str,
) -> Result<bool, StateStoreError> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
    )
    .bind(index_name)
    .fetch_one(connection)
    .await
    .map_err(crate::sqlite::sqlx_error)
}

pub(super) async fn validate_index(
    connection: &mut SqliteConnection,
    index_name: &'static str,
    expected_table: &'static str,
    expected_columns: &'static [&'static str],
) -> Result<(), StateStoreError> {
    let row = sqlx::query("SELECT tbl_name FROM sqlite_master WHERE type = 'index' AND name = ?1")
        .bind(index_name)
        .fetch_optional(&mut *connection)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    let Some(row) = row else {
        return incompatible_schema();
    };
    if row.get::<String, _>(0) != expected_table {
        return incompatible_schema();
    }
    let properties =
        sqlx::query("SELECT [unique], origin, partial FROM pragma_index_list(?1) WHERE name = ?2")
            .bind(expected_table)
            .bind(index_name)
            .fetch_one(&mut *connection)
            .await
            .map_err(crate::sqlite::sqlx_error)?;
    if properties.get::<i64, _>(0) != 0
        || properties.get::<String, _>(1) != "c"
        || properties.get::<i64, _>(2) != 0
    {
        return incompatible_schema();
    }
    let query = format!("PRAGMA index_xinfo(\"{index_name}\")");
    let rows = sqlx::query(sqlx::AssertSqlSafe(query))
        .fetch_all(connection)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    let key_rows = rows
        .into_iter()
        .filter(|row| row.get::<i64, _>("key") == 1)
        .collect::<Vec<_>>();
    if key_rows.len() != expected_columns.len() {
        return incompatible_schema();
    }
    for (position, (row, expected_column)) in key_rows.iter().zip(expected_columns).enumerate() {
        if row.get::<i64, _>("seqno") != i64::try_from(position).unwrap_or(i64::MAX)
            || row.get::<i64, _>("cid") < 0
            || row.get::<String, _>("name") != *expected_column
            || row.get::<i64, _>("desc") != 0
            || !row.get::<String, _>("coll").eq_ignore_ascii_case("BINARY")
        {
            return incompatible_schema();
        }
    }
    Ok(())
}

pub(super) async fn validate_table_constraints(
    connection: &mut SqliteConnection,
    table_name: &'static str,
) -> Result<(), StateStoreError> {
    let sql: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1")
            .bind(table_name)
            .fetch_one(&mut *connection)
            .await
            .map_err(crate::sqlite::sqlx_error)?;
    let tokens = schema_tokens(&sql);
    let forbidden = tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "references" | "unique" | "without" | "strict" | "collate"
        )
    }) || tokens
        .windows(2)
        .any(|window| window == ["foreign", "key"] || window == ["on", "conflict"]);
    let check_count = tokens
        .iter()
        .filter(|token| token.as_str() == "check")
        .count();
    let autoincrement_count = tokens
        .iter()
        .filter(|token| token.as_str() == "autoincrement")
        .count();
    let expected_autoincrement = matches!(
        table_name,
        "quota_history_observations" | "active_session_events"
    );
    if forbidden
        || (table_name == "account_routing_policies" && check_count != 1)
        || (table_name != "account_routing_policies" && check_count != 0)
        || (expected_autoincrement && autoincrement_count != 1)
        || (!expected_autoincrement && autoincrement_count != 0)
    {
        return incompatible_schema();
    }

    let indexes = sqlx::query("SELECT [unique], origin FROM pragma_index_list(?1)")
        .bind(table_name)
        .fetch_all(connection)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    if indexes
        .iter()
        .any(|row| row.get::<i64, _>(0) != 0 && row.get::<String, _>(1) != "pk")
    {
        return incompatible_schema();
    }
    Ok(())
}

pub(super) fn schema_tokens(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut characters = sql.chars().peekable();
    let mut quoted = None;
    while let Some(character) = characters.next() {
        if let Some(quote) = quoted {
            current.push(character);
            if character == quote {
                if characters.peek() == Some(&quote) {
                    current.push(characters.next().unwrap_or(quote));
                } else {
                    quoted = None;
                    tokens.push(std::mem::take(&mut current));
                }
            }
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            current.push(character);
            quoted = Some(character);
        } else if character.is_ascii_alphanumeric() || character == '_' {
            current.push(character.to_ascii_lowercase());
        } else if matches!(character, '(' | ')' | '%' | '=') {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            tokens.push(character.to_string());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub(super) async fn reject_leftover_replacement_tables(
    connection: &mut SqliteConnection,
) -> Result<(), StateStoreError> {
    for table_name in ["active_client_leases_v8", "account_routing_policies_v12"] {
        if table_exists(connection, table_name).await? {
            return incompatible_schema();
        }
    }
    Ok(())
}

pub(super) async fn reject_unexpected_dependents(
    connection: &mut SqliteConnection,
    table_name: &'static str,
    permitted_indexes: &[&str],
) -> Result<(), StateStoreError> {
    let rows = sqlx::query(
        "SELECT type, name FROM sqlite_master
         WHERE tbl_name = ?1 AND type IN ('index', 'trigger', 'view')",
    )
    .bind(table_name)
    .fetch_all(connection)
    .await
    .map_err(crate::sqlite::sqlx_error)?;
    for row in rows {
        let object_type = row.get::<String, _>(0);
        let name = row.get::<String, _>(1);
        if object_type == "index"
            && (name.starts_with("sqlite_autoindex_") || permitted_indexes.contains(&name.as_str()))
        {
            continue;
        }
        return incompatible_schema();
    }
    Ok(())
}

pub(super) fn incompatible_schema<T>() -> Result<T, StateStoreError> {
    Err(StateStoreError::Sqlite {
        message: INCOMPATIBLE_SCHEMA_MESSAGE.to_owned(),
    })
}
