//! Owned interaction database initialization, checked rows and row-delta commits.
use super::interaction_history_codec::decode_stored_record;
use super::{InteractionHistoryData, InteractionHistoryError};
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Connection, Sqlite, SqliteConnection, Transaction};
use std::{
    ops::{Deref, DerefMut},
    path::Path,
    time::Duration,
};

#[path = "interaction_history_schema.rs"]
mod interaction_history_schema;
use interaction_history_schema::{SchemaAdmission, inspect_schema};

pub(super) static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./interaction-migrations");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HistoryStorageFailure {
    StorageUnavailable,
    InvalidSchema,
    InvalidStoredRecord,
}

impl HistoryStorageFailure {
    pub(super) fn public_error(self, startup: bool) -> InteractionHistoryError {
        if startup {
            tracing::error!(reason = ?self, "interaction_history_startup_failed");
        } else {
            tracing::error!(reason = ?self, "interaction_history_mutation_failed");
        }
        InteractionHistoryError::Unavailable
    }
}

pub(super) struct HistoryDatabaseState {
    pub(super) connection: SqliteConnection,
    pub(super) cache: InteractionHistoryData,
    pub(super) revision: i64,
}

impl Deref for HistoryDatabaseState {
    type Target = InteractionHistoryData;
    fn deref(&self) -> &Self::Target {
        &self.cache
    }
}
impl DerefMut for HistoryDatabaseState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cache
    }
}

struct StoredInteractionRow {
    request_id: String,
    record_json: String,
    created_at: String,
    request_storage: String,
    record_storage: String,
    timestamp_storage: String,
}

struct HistoryRevisionRow {
    metadata_id: i64,
    pub(super) revision: i64,
    metadata_storage: String,
    revision_storage: String,
}

pub(super) async fn read_revision(
    connection: &mut SqliteConnection,
) -> Result<i64, HistoryStorageFailure> {
    let mut rows = sqlx::query_as!(HistoryRevisionRow,
        "SELECT metadata_id, revision, typeof(metadata_id) AS \"metadata_storage!: String\", typeof(revision) AS \"revision_storage!: String\" FROM interaction_history_revision")
        .fetch_all(connection).await.map_err(|_| HistoryStorageFailure::InvalidSchema)?;
    if rows.len() != 1 {
        return Err(HistoryStorageFailure::InvalidSchema);
    }
    let row = rows.pop().ok_or(HistoryStorageFailure::InvalidSchema)?;
    if row.metadata_id != 1
        || row.revision < 0
        || row.metadata_storage != "integer"
        || row.revision_storage != "integer"
    {
        return Err(HistoryStorageFailure::InvalidSchema);
    }
    Ok(row.revision)
}

pub(super) async fn read_records(
    connection: &mut SqliteConnection,
) -> Result<InteractionHistoryData, HistoryStorageFailure> {
    let rows = sqlx::query_as!(StoredInteractionRow,
        "SELECT request_id, record_json, created_at, typeof(request_id) AS \"request_storage!: String\", typeof(record_json) AS \"record_storage!: String\", typeof(created_at) AS \"timestamp_storage!: String\" FROM typed_interaction_history ORDER BY request_id")
        .fetch_all(connection).await.map_err(|_| HistoryStorageFailure::InvalidStoredRecord)?;
    let mut data = InteractionHistoryData::default();
    for row in rows {
        if row.request_storage != "text"
            || row.record_storage != "text"
            || row.timestamp_storage != "text"
        {
            return Err(HistoryStorageFailure::InvalidStoredRecord);
        }
        let (record, created_at) =
            decode_stored_record(&row.request_id, &row.record_json, &row.created_at)?;
        data.insert_with_timestamp(row.request_id, record, created_at);
    }
    Ok(data)
}

fn format_timestamp(created_at: DateTime<Utc>) -> String {
    created_at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

pub(super) async fn initialize_history(
    database_path: &Path,
) -> Result<HistoryDatabaseState, HistoryStorageFailure> {
    match tokio::fs::metadata(database_path).await {
        Ok(metadata) if metadata.is_file() => {
            let options = SqliteConnectOptions::new()
                .filename(database_path)
                .read_only(true);
            let mut observer = SqliteConnection::connect_with(&options)
                .await
                .map_err(|_| HistoryStorageFailure::InvalidSchema)?;
            inspect_schema(&mut observer).await?;
            observer
                .close()
                .await
                .map_err(|_| HistoryStorageFailure::StorageUnavailable)?;
        }
        Ok(_) => return Err(HistoryStorageFailure::InvalidSchema),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(HistoryStorageFailure::StorageUnavailable),
    }
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(Duration::from_secs(1));
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(|_| HistoryStorageFailure::StorageUnavailable)?;
    let mut transaction = connection
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| HistoryStorageFailure::StorageUnavailable)?;
    let admission = inspect_schema(&mut transaction).await?;
    MIGRATOR
        .run_direct(None, &mut *transaction, false)
        .await
        .map_err(|_| HistoryStorageFailure::InvalidSchema)?;
    if admission == SchemaAdmission::Pristine {
        sqlx::query!(
            "INSERT INTO interaction_history_revision (metadata_id, revision) VALUES (1, 0)"
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| HistoryStorageFailure::StorageUnavailable)?;
    }
    let revision = read_revision(&mut transaction).await?;
    let cache = read_records(&mut transaction).await?;
    transaction
        .commit()
        .await
        .map_err(|_| HistoryStorageFailure::StorageUnavailable)?;
    Ok(HistoryDatabaseState {
        connection,
        cache,
        revision,
    })
}

enum HistoryRowDelta<'record> {
    Insert {
        request_id: &'record str,
        record_json: String,
        created_at: String,
    },
    Update {
        request_id: &'record str,
        record_json: String,
    },
    Delete {
        request_id: &'record str,
    },
}

pub(super) async fn commit_delta(
    transaction: &mut Transaction<'_, Sqlite>,
    previous: &InteractionHistoryData,
    next: &InteractionHistoryData,
    revision: i64,
) -> Result<i64, HistoryStorageFailure> {
    if next.records.len() != next.created_at.len() {
        return Err(HistoryStorageFailure::InvalidStoredRecord);
    }
    let mut changes = Vec::new();
    for request_id in previous.records.keys() {
        if !next.records.contains_key(request_id) {
            changes.push(HistoryRowDelta::Delete { request_id });
        }
    }
    for (request_id, record) in &next.records {
        if request_id != record.request_id() || !record.is_valid_stored_value() {
            return Err(HistoryStorageFailure::InvalidStoredRecord);
        }
        let created_at = next
            .created_at
            .get(request_id)
            .ok_or(HistoryStorageFailure::InvalidStoredRecord)?;
        let record_json = serde_json::to_string(record)
            .map_err(|_| HistoryStorageFailure::InvalidStoredRecord)?;
        match previous.records.get(request_id) {
            Some(previous_record) => {
                if previous.created_at.get(request_id) != Some(created_at) {
                    return Err(HistoryStorageFailure::InvalidStoredRecord);
                }
                if serde_json::to_string(previous_record)
                    .map_err(|_| HistoryStorageFailure::InvalidStoredRecord)?
                    != record_json
                {
                    changes.push(HistoryRowDelta::Update {
                        request_id,
                        record_json,
                    });
                }
            }
            None => changes.push(HistoryRowDelta::Insert {
                request_id,
                record_json,
                created_at: format_timestamp(*created_at),
            }),
        }
    }
    if changes.is_empty() {
        return Ok(revision);
    }
    // Validate revision advancement and the entire staged delta before issuing writes.
    let next_revision = revision
        .checked_add(1)
        .ok_or(HistoryStorageFailure::InvalidSchema)?;
    for change in changes {
        match change {
            HistoryRowDelta::Insert {
                request_id,
                record_json,
                created_at,
            } => {
                sqlx::query!("INSERT INTO typed_interaction_history (request_id, record_json, created_at) VALUES (?, ?, ?)", request_id,record_json,created_at)
                    .execute(&mut **transaction).await.map_err(|_| HistoryStorageFailure::StorageUnavailable)?;
            }
            HistoryRowDelta::Update {
                request_id,
                record_json,
            } => {
                sqlx::query!(
                    "UPDATE typed_interaction_history SET record_json = ? WHERE request_id = ?",
                    record_json,
                    request_id
                )
                .execute(&mut **transaction)
                .await
                .map_err(|_| HistoryStorageFailure::StorageUnavailable)?;
            }
            HistoryRowDelta::Delete { request_id } => {
                sqlx::query!(
                    "DELETE FROM typed_interaction_history WHERE request_id = ?",
                    request_id
                )
                .execute(&mut **transaction)
                .await
                .map_err(|_| HistoryStorageFailure::StorageUnavailable)?;
            }
        }
    }
    let updated = sqlx::query!(
        "UPDATE interaction_history_revision SET revision = ? WHERE metadata_id = 1 AND revision = ?",
        next_revision,
        revision
    )
    .execute(&mut **transaction)
    .await
    .map_err(|_| HistoryStorageFailure::StorageUnavailable)?;
    if updated.rows_affected() != 1 {
        return Err(HistoryStorageFailure::InvalidSchema);
    }
    Ok(next_revision)
}
