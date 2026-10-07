use sqlx::SqliteConnection;

use super::DIRTY_HISTORY_MESSAGE;
use super::INCOMPATIBLE_HISTORY_MESSAGE;
use super::MIGRATOR;
use super::MigrationAuthority;
use super::native_history_table_exists;
use super::static_sqlite_error;
use crate::sqlite::StateStoreError;

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

#[derive(Clone, Copy)]
enum ValidationPurpose {
    ExistingReadOnlyAuthority,
    SchemaPreparation,
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

enum StoredMigrationHistory {
    Legacy,
    Native(Vec<StoredMigration>),
}

pub(crate) async fn existing_authority(
    connection: &mut SqliteConnection,
) -> Result<MigrationAuthority, StateStoreError> {
    let history = read_history(connection)
        .await
        .map_err(map_existing_history_error)?;
    let StoredMigrationHistory::Native(rows) = history else {
        return Ok(MigrationAuthority::Legacy);
    };
    let versions = validate_rows(&rows, ValidationPurpose::ExistingReadOnlyAuthority)
        .map_err(map_existing_history_error)?;
    if versions.len() == MIGRATOR.iter().count() {
        Ok(MigrationAuthority::NativeCurrent)
    } else {
        Ok(MigrationAuthority::NativeUpgradeRequired)
    }
}

pub(crate) async fn inspect_for_preparation(
    connection: &mut SqliteConnection,
) -> Result<SchemaPreparationHistory, MigrationHistoryError> {
    let history = read_history(connection).await?;
    let StoredMigrationHistory::Native(rows) = history else {
        return Err(MigrationHistoryError::UnrecognizedNativeHistory);
    };
    let applied_versions = validate_rows(&rows, ValidationPurpose::SchemaPreparation)?;
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

async fn read_history(
    connection: &mut SqliteConnection,
) -> Result<StoredMigrationHistory, MigrationHistoryError> {
    if !native_history_table_exists(&mut *connection)
        .await
        .map_err(MigrationHistoryError::TableCheck)?
    {
        return Ok(StoredMigrationHistory::Legacy);
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

    Ok(StoredMigrationHistory::Native(
        rows.into_iter()
            .map(|row| StoredMigration {
                version: row.version,
                version_storage: row.version_storage,
                success: row.success,
                success_storage: row.success_storage,
                success_value_is_boolean: row.success_value_is_boolean,
                checksum: row.checksum,
                checksum_storage: row.checksum_storage,
            })
            .collect(),
    ))
}

fn validate_rows(
    rows: &[StoredMigration],
    purpose: ValidationPurpose,
) -> Result<Vec<i64>, MigrationHistoryError> {
    let migrations = MIGRATOR.iter().collect::<Vec<_>>();
    let newest_image_version = migrations
        .last()
        .map(|migration| migration.version)
        .unwrap_or_default();
    let mut applied_versions = Vec::with_capacity(rows.len());

    for row in rows {
        let Some(version) = row.version else {
            return Err(match purpose {
                ValidationPurpose::ExistingReadOnlyAuthority => {
                    MigrationHistoryError::InvalidHistory
                }
                ValidationPurpose::SchemaPreparation => MigrationHistoryError::InvalidVersion,
            });
        };

        if matches!(purpose, ValidationPurpose::SchemaPreparation) {
            if row.version_storage != "integer" || version <= 0 {
                return Err(MigrationHistoryError::InvalidVersion);
            }
            if row.success_storage != "integer" || !row.success_value_is_boolean {
                return Err(MigrationHistoryError::InvalidHistory);
            }
            if row.checksum_storage != "blob" {
                return Err(MigrationHistoryError::ChecksumMismatch);
            }
        }

        if !row.success {
            return Err(MigrationHistoryError::DirtyMigration);
        }

        let Some(migration) = migrations
            .iter()
            .find(|migration| migration.version == version)
        else {
            if matches!(purpose, ValidationPurpose::SchemaPreparation)
                && version > newest_image_version
            {
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

fn map_existing_history_error(error: MigrationHistoryError) -> StateStoreError {
    match error {
        MigrationHistoryError::TableCheck(error) => crate::sqlite::sqlx_error(error),
        MigrationHistoryError::DirtyMigration => static_sqlite_error(DIRTY_HISTORY_MESSAGE),
        MigrationHistoryError::RowsRead(_)
        | MigrationHistoryError::UnrecognizedNativeHistory
        | MigrationHistoryError::InvalidHistory
        | MigrationHistoryError::InvalidVersion
        | MigrationHistoryError::ChecksumMismatch
        | MigrationHistoryError::SchemaNewerThanImage
        | MigrationHistoryError::UnknownAppliedMigration
        | MigrationHistoryError::InvalidAppliedOrder => {
            static_sqlite_error(INCOMPATIBLE_HISTORY_MESSAGE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_applied_prefix_maps_to_its_literal_image_suffix() {
        let image_versions = MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .collect::<Vec<_>>();

        for prefix_length in 0..=image_versions.len() {
            let applied_prefix = &image_versions[..prefix_length];
            assert_eq!(
                suffix_for_valid_prefix(applied_prefix, &image_versions)
                    .expect("real image prefix should classify"),
                image_versions[prefix_length..]
            );
        }
    }

    #[test]
    fn gaps_and_repeated_versions_are_not_valid_prefixes() {
        let image_versions = MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .collect::<Vec<_>>();
        assert!(image_versions.len() >= 3);

        assert!(matches!(
            suffix_for_valid_prefix(&[image_versions[0], image_versions[2]], &image_versions),
            Err(MigrationHistoryError::InvalidAppliedOrder)
        ));
        assert!(matches!(
            suffix_for_valid_prefix(&[image_versions[0], image_versions[0]], &image_versions),
            Err(MigrationHistoryError::InvalidAppliedOrder)
        ));
    }
}
