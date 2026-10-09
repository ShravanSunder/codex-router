use sqlx::SqliteConnection;

use crate::schema_initialization::MIGRATOR;

#[derive(Debug)]
pub(crate) enum MigrationHistoryError {
    TableCheck(sqlx::Error),
    RowsRead(sqlx::Error),
    UnrecognizedNativeHistory,
    InvalidHistory,
    InvalidVersion,
    DirtyMigration,
    ChecksumMismatch,
    SchemaNewerThanImage,
    UnknownAppliedMigration,
    InvalidAppliedOrder,
}

#[derive(Debug)]
pub(crate) enum SchemaPreparationHistory {
    Current,
    Pending(Vec<i64>),
}

struct StoredMigration {
    version: Option<i64>,
    version_storage: String,
    success: bool,
    success_storage: String,
    success_value_is_boolean: bool,
    checksum: Vec<u8>,
    checksum_storage: String,
}

pub(crate) async fn inspect_for_preparation(
    connection: &mut SqliteConnection,
) -> Result<SchemaPreparationHistory, MigrationHistoryError> {
    if !native_history_table_exists(&mut *connection)
        .await
        .map_err(MigrationHistoryError::TableCheck)?
    {
        return Err(MigrationHistoryError::UnrecognizedNativeHistory);
    }

    let rows = sqlx::query!(
        r#"
        SELECT
            version AS "version: i64",
            typeof(version) AS "version_storage!",
            success AS "success: bool",
            typeof(success) AS "success_storage!",
            CASE
                WHEN typeof(success) = 'integer' AND success IN (0, 1) THEN 1
                ELSE 0
            END AS "success_value_is_boolean: bool",
            checksum AS "checksum: Vec<u8>",
            typeof(checksum) AS "checksum_storage!"
        FROM _sqlx_migrations
        ORDER BY version
        "#
    )
    .fetch_all(connection)
    .await
    .map_err(MigrationHistoryError::RowsRead)?;

    let rows = rows
        .into_iter()
        .map(|row| StoredMigration {
            version: row.version,
            version_storage: row.version_storage,
            success: row.success,
            success_storage: row.success_storage,
            success_value_is_boolean: row.success_value_is_boolean,
            checksum: row.checksum,
            checksum_storage: row.checksum_storage,
        })
        .collect::<Vec<_>>();

    let applied_versions = validate_rows(&rows)?;
    let image_versions = MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .collect::<Vec<_>>();
    let pending_versions = suffix_for_valid_prefix(&applied_versions, &image_versions)?;
    if pending_versions.is_empty() {
        Ok(SchemaPreparationHistory::Current)
    } else {
        Ok(SchemaPreparationHistory::Pending(pending_versions))
    }
}

async fn native_history_table_exists(
    connection: &mut SqliteConnection,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')"
    )
    .fetch_one(connection)
    .await?;
    Ok(exists != 0)
}

fn validate_rows(rows: &[StoredMigration]) -> Result<Vec<i64>, MigrationHistoryError> {
    let migrations = MIGRATOR.iter().collect::<Vec<_>>();
    let newest_image_version = migrations
        .last()
        .map(|migration| migration.version)
        .unwrap_or_default();
    let mut applied_versions = Vec::with_capacity(rows.len());

    for row in rows {
        let Some(version) = row.version else {
            return Err(MigrationHistoryError::InvalidVersion);
        };
        if row.version_storage != "integer" || version <= 0 {
            return Err(MigrationHistoryError::InvalidVersion);
        }
        if row.success_storage != "integer" || !row.success_value_is_boolean {
            return Err(MigrationHistoryError::InvalidHistory);
        }
        if row.checksum_storage != "blob" {
            return Err(MigrationHistoryError::ChecksumMismatch);
        }
        if !row.success {
            return Err(MigrationHistoryError::DirtyMigration);
        }

        let Some(migration) = migrations
            .iter()
            .find(|migration| migration.version == version)
        else {
            if version > newest_image_version {
                return Err(MigrationHistoryError::SchemaNewerThanImage);
            }
            return Err(MigrationHistoryError::UnknownAppliedMigration);
        };
        if row.checksum.as_slice() != migration.checksum.as_ref() {
            return Err(MigrationHistoryError::ChecksumMismatch);
        }
        applied_versions.push(version);
    }

    Ok(applied_versions)
}

fn suffix_for_valid_prefix(
    applied_versions: &[i64],
    image_versions: &[i64],
) -> Result<Vec<i64>, MigrationHistoryError> {
    if applied_versions.len() > image_versions.len()
        || applied_versions
            .iter()
            .zip(image_versions)
            .any(|(applied, image)| applied != image)
    {
        return Err(MigrationHistoryError::InvalidAppliedOrder);
    }
    let Some(pending_versions) = image_versions.get(applied_versions.len()..) else {
        return Err(MigrationHistoryError::InvalidAppliedOrder);
    };
    Ok(pending_versions.to_vec())
}
