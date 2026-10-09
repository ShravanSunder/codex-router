use crate::automation_migration_history::{MigrationHistoryError, SchemaPreparationHistory};
use crate::{AutomationSchemaPreparationError, StorageError};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{future::Future, num::NonZeroI64, path::Path, time::Duration};

/// A positive migration version embedded in the automation image.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AutomationMigrationVersion(NonZeroI64);

impl AutomationMigrationVersion {
    /// Constructs a migration version only when it is positive.
    #[must_use]
    pub const fn new(version: i64) -> Option<Self> {
        if version <= 0 {
            return None;
        }
        match NonZeroI64::new(version) {
            Some(version) => Some(Self(version)),
            None => None,
        }
    }

    /// Returns the numeric migration version.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0.get()
    }
}

/// A validated, nonempty and ordered remainder of the embedded migration set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingAutomationMigrationVersions(Vec<AutomationMigrationVersion>);

impl PendingAutomationMigrationVersions {
    /// Returns the remaining versions in the embedded migration order.
    #[must_use]
    pub fn as_slice(&self) -> &[AutomationMigrationVersion] {
        &self.0
    }

    fn from_ordered_suffix(
        versions: Vec<AutomationMigrationVersion>,
    ) -> Result<Self, AutomationSchemaPreparationError> {
        if versions.is_empty()
            || versions.windows(2).any(|pair| match pair {
                [first, second] => first >= second,
                _ => false,
            })
        {
            return Err(AutomationSchemaPreparationError::InvalidAppliedOrder);
        }
        Ok(Self(versions))
    }
}

/// The automation database's read-only schema-preparation result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutomationSchemaPreparation {
    /// The database already has this image's complete, valid schema.
    Current,
    /// The database has a valid applied prefix and these migrations remain.
    Pending {
        /// The ordered, nonempty remainder of this image's migration set.
        migrations: PendingAutomationMigrationVersions,
    },
}

pub(crate) async fn prepare_schema(
    database_path: &Path,
) -> Result<AutomationSchemaPreparation, AutomationSchemaPreparationError> {
    prepare_schema_with_checkpoint(database_path, || async {}).await
}

pub(super) async fn prepare_schema_with_checkpoint<FCheckpoint, FCheckpointFuture>(
    database_path: &Path,
    after_history_read: FCheckpoint,
) -> Result<AutomationSchemaPreparation, AutomationSchemaPreparationError>
where
    FCheckpoint: FnOnce() -> FCheckpointFuture + Send,
    FCheckpointFuture: Future<Output = ()> + Send,
{
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .read_only(true)
        .create_if_missing(false)
        .foreign_keys(true)
        .busy_timeout(Duration::ZERO)
        .pragma("query_only", "ON");
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(|source| AutomationSchemaPreparationError::UnreadableStore { source })?;

    let preparation = match connection.begin().await {
        Ok(mut transaction) => {
            let inspection = inspect_connection(&mut transaction, after_history_read).await;
            let rollback = transaction
                .rollback()
                .await
                .map_err(|source| AutomationSchemaPreparationError::UnreadableStore { source });
            match inspection {
                Ok(preparation) => rollback.map(|()| preparation),
                Err(error) => Err(error),
            }
        }
        Err(source) => Err(AutomationSchemaPreparationError::UnreadableStore { source }),
    };
    let close_result = connection.close().await;
    match preparation {
        Ok(preparation) => {
            close_result
                .map_err(|source| AutomationSchemaPreparationError::UnreadableStore { source })?;
            Ok(preparation)
        }
        Err(error) => Err(error),
    }
}

async fn inspect_connection<FCheckpoint, FCheckpointFuture>(
    connection: &mut SqliteConnection,
    after_history_read: FCheckpoint,
) -> Result<AutomationSchemaPreparation, AutomationSchemaPreparationError>
where
    FCheckpoint: FnOnce() -> FCheckpointFuture + Send,
    FCheckpointFuture: Future<Output = ()> + Send,
{
    match crate::automation_migration_history::inspect_for_preparation(connection)
        .await
        .map_err(map_history_error)?
    {
        SchemaPreparationHistory::Current => {
            after_history_read().await;
            crate::schema_validation::validate_target_schema(connection)
                .await
                .map_err(invalid_current_schema)?;
            Ok(AutomationSchemaPreparation::Current)
        }
        SchemaPreparationHistory::Pending(versions) => {
            let versions = versions
                .into_iter()
                .map(|version| {
                    AutomationMigrationVersion::new(version)
                        .ok_or(AutomationSchemaPreparationError::InvalidMigrationVersion)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let migrations = PendingAutomationMigrationVersions::from_ordered_suffix(versions)?;
            Ok(AutomationSchemaPreparation::Pending { migrations })
        }
    }
}

fn invalid_current_schema(source: StorageError) -> AutomationSchemaPreparationError {
    AutomationSchemaPreparationError::InvalidCurrentSchema { source }
}

fn map_history_error(error: MigrationHistoryError) -> AutomationSchemaPreparationError {
    match error {
        MigrationHistoryError::TableCheck(source) | MigrationHistoryError::RowsRead(source) => {
            AutomationSchemaPreparationError::UnreadableStore { source }
        }
        MigrationHistoryError::UnrecognizedNativeHistory => {
            AutomationSchemaPreparationError::UnrecognizedNativeHistory
        }
        MigrationHistoryError::InvalidHistory => {
            AutomationSchemaPreparationError::InvalidMigrationHistory
        }
        MigrationHistoryError::InvalidVersion => {
            AutomationSchemaPreparationError::InvalidMigrationVersion
        }
        MigrationHistoryError::DirtyMigration => AutomationSchemaPreparationError::DirtyMigration,
        MigrationHistoryError::ChecksumMismatch => {
            AutomationSchemaPreparationError::ChecksumMismatch
        }
        MigrationHistoryError::SchemaNewerThanImage => {
            AutomationSchemaPreparationError::SchemaNewerThanImage
        }
        MigrationHistoryError::UnknownAppliedMigration => {
            AutomationSchemaPreparationError::UnknownAppliedMigration
        }
        MigrationHistoryError::InvalidAppliedOrder => {
            AutomationSchemaPreparationError::InvalidAppliedOrder
        }
    }
}
