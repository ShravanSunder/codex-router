//! Non-creating inspection of the exact owned interaction schema and migration lineage.
use super::{HistoryStorageFailure, MIGRATOR, read_metadata, read_records};
use sqlx::SqliteConnection;

#[derive(Eq, PartialEq)]
pub(super) enum SchemaAdmission {
    Pristine,
    Owned,
}

struct SchemaObjectRow {
    name: String,
    object_type: String,
    sql: Option<String>,
}

struct MigrationProofRow {
    version: i64,
    success: i64,
    checksum: Vec<u8>,
    version_storage: String,
    success_storage: String,
    checksum_storage: String,
}

#[derive(sqlx::FromRow)]
struct SchemaColumnRow {
    name: String,
    column_type: String,
    required: i64,
    primary_key: i64,
}

fn normalize_schema_sql(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace() && *character != ';')
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn expected_table_sql(table_name: &str) -> Result<String, HistoryStorageFailure> {
    let prefix = format!("CREATE TABLE {table_name} (");
    let statement =
        include_str!("../../interaction-migrations/202610080001_interaction_history.sql")
            .split(';')
            .find(|statement| statement.trim_start().starts_with(&prefix))
            .ok_or(HistoryStorageFailure::InvalidSchema)?;
    Ok(normalize_schema_sql(statement))
}

pub(super) async fn inspect_schema(
    connection: &mut SqliteConnection,
) -> Result<SchemaAdmission, HistoryStorageFailure> {
    let objects = sqlx::query_as!(SchemaObjectRow,
        "SELECT name AS \"name!\", type AS \"object_type!\", sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
        .fetch_all(&mut *connection).await.map_err(|_| HistoryStorageFailure::InvalidSchema)?;
    let user_version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| HistoryStorageFailure::InvalidSchema)?;
    let application_id: i64 = sqlx::query_scalar("PRAGMA application_id")
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| HistoryStorageFailure::InvalidSchema)?;
    if user_version != 0 || application_id != 0 {
        return Err(HistoryStorageFailure::InvalidSchema);
    }
    if objects.is_empty() {
        return Ok(SchemaAdmission::Pristine);
    }
    let expected_names = [
        "_sqlx_migrations",
        "interaction_history_import",
        "typed_interaction_history",
    ];
    if objects.len() != expected_names.len()
        || objects
            .iter()
            .zip(expected_names)
            .any(|(object, expected)| object.name != expected || object.object_type != "table")
    {
        return Err(HistoryStorageFailure::InvalidSchema);
    }
    for object in objects
        .iter()
        .filter(|object| object.name != "_sqlx_migrations")
    {
        let actual = object
            .sql
            .as_deref()
            .ok_or(HistoryStorageFailure::InvalidSchema)?;
        if normalize_schema_sql(actual) != expected_table_sql(&object.name)? {
            return Err(HistoryStorageFailure::InvalidSchema);
        }
    }
    // SQLx's pinned SQLite describer cannot describe PRAGMA virtual tables.
    // Follow the existing provider/account schema-inspection boundary; all owned rows use checked queries.
    let columns = sqlx::query_as::<_, SchemaColumnRow>(
        "SELECT name, type AS column_type, \"notnull\" AS required, pk AS primary_key FROM pragma_table_info('_sqlx_migrations') ORDER BY cid")
        .fetch_all(&mut *connection).await.map_err(|_| HistoryStorageFailure::InvalidSchema)?;
    let expected_columns = [
        ("version", "BIGINT", 0, 1),
        ("description", "TEXT", 1, 0),
        ("installed_on", "TIMESTAMP", 1, 0),
        ("success", "BOOLEAN", 1, 0),
        ("checksum", "BLOB", 1, 0),
        ("execution_time", "BIGINT", 1, 0),
    ];
    if columns.len() != expected_columns.len()
        || columns.iter().zip(expected_columns).any(
            |(actual, (name, column_type, required, primary_key))| {
                actual.name != name
                    || actual.column_type != column_type
                    || actual.required != required
                    || actual.primary_key != primary_key
            },
        )
    {
        return Err(HistoryStorageFailure::InvalidSchema);
    }
    let migrations = sqlx::query_as!(MigrationProofRow,
        "SELECT version AS \"version!\", success AS \"success!: i64\", checksum, typeof(version) AS \"version_storage!: String\", typeof(success) AS \"success_storage!: String\", typeof(checksum) AS \"checksum_storage!: String\" FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| HistoryStorageFailure::InvalidSchema)?;
    if migrations.is_empty()
        || migrations.len() > MIGRATOR.iter().count()
        || migrations
            .iter()
            .zip(MIGRATOR.iter())
            .any(|(actual, expected)| {
                actual.version != expected.version
                    || actual.success != 1
                    || actual.version_storage != "integer"
                    || actual.success_storage != "integer"
                    || actual.checksum_storage != "blob"
                    || actual.checksum.as_slice() != expected.checksum.as_ref()
            })
    {
        return Err(HistoryStorageFailure::InvalidSchema);
    }
    read_metadata(connection).await?;
    read_records(connection).await?;
    Ok(SchemaAdmission::Owned)
}
