use crate::provider_operation_migration_history::{
    MigrationHistoryError, SchemaPreparationHistory,
};
use crate::{ProviderOperationSchemaPreparationError, ProviderOperationStoreError};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{future::Future, num::NonZeroI64, path::Path, time::Duration};

/// A positive migration version embedded in the provider-operation image.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProviderOperationMigrationVersion(NonZeroI64);

impl ProviderOperationMigrationVersion {
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
pub struct PendingProviderOperationMigrationVersions(Vec<ProviderOperationMigrationVersion>);

impl PendingProviderOperationMigrationVersions {
    /// Returns the remaining versions in the embedded migration order.
    #[must_use]
    pub fn as_slice(&self) -> &[ProviderOperationMigrationVersion] {
        &self.0
    }

    fn from_ordered_suffix(
        versions: Vec<ProviderOperationMigrationVersion>,
    ) -> Result<Self, ProviderOperationSchemaPreparationError> {
        if versions.is_empty()
            || versions.windows(2).any(|pair| match pair {
                [first, second] => first >= second,
                _ => false,
            })
        {
            return Err(ProviderOperationSchemaPreparationError::InvalidAppliedOrder);
        }
        Ok(Self(versions))
    }
}

/// The provider-operation database's read-only schema-preparation result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderOperationSchemaPreparation {
    /// The database already has this image's complete, valid schema.
    Current,
    /// The database has a valid applied prefix and these migrations remain.
    Pending {
        /// The ordered, nonempty remainder of this image's migration set.
        migrations: PendingProviderOperationMigrationVersions,
    },
}

pub(crate) async fn prepare_schema(
    database_path: &Path,
) -> Result<ProviderOperationSchemaPreparation, ProviderOperationSchemaPreparationError> {
    prepare_schema_with_checkpoint(database_path, || async {}).await
}

pub(super) async fn prepare_schema_with_checkpoint<FCheckpoint, FCheckpointFuture>(
    database_path: &Path,
    after_history_read: FCheckpoint,
) -> Result<ProviderOperationSchemaPreparation, ProviderOperationSchemaPreparationError>
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
        .map_err(|source| ProviderOperationSchemaPreparationError::UnreadableStore { source })?;

    let preparation = match connection.begin().await {
        Ok(mut transaction) => {
            let inspection = inspect_connection(&mut transaction, after_history_read).await;
            let rollback = transaction.rollback().await.map_err(|source| {
                ProviderOperationSchemaPreparationError::UnreadableStore { source }
            });
            match inspection {
                Ok(preparation) => rollback.map(|()| preparation),
                Err(error) => Err(error),
            }
        }
        Err(source) => Err(ProviderOperationSchemaPreparationError::UnreadableStore { source }),
    };
    let close_result = connection.close().await;
    match preparation {
        Ok(preparation) => {
            close_result.map_err(|source| {
                ProviderOperationSchemaPreparationError::UnreadableStore { source }
            })?;
            Ok(preparation)
        }
        Err(error) => Err(error),
    }
}

async fn inspect_connection<FCheckpoint, FCheckpointFuture>(
    connection: &mut SqliteConnection,
    after_history_read: FCheckpoint,
) -> Result<ProviderOperationSchemaPreparation, ProviderOperationSchemaPreparationError>
where
    FCheckpoint: FnOnce() -> FCheckpointFuture + Send,
    FCheckpointFuture: Future<Output = ()> + Send,
{
    match crate::provider_operation_migration_history::inspect_for_preparation(connection)
        .await
        .map_err(map_history_error)?
    {
        SchemaPreparationHistory::Current => {
            after_history_read().await;
            crate::provider_operation_store::validate_schema(connection)
                .await
                .map_err(invalid_current_schema)?;
            Ok(ProviderOperationSchemaPreparation::Current)
        }
        SchemaPreparationHistory::Pending(versions) => {
            let versions = versions
                .into_iter()
                .map(|version| {
                    ProviderOperationMigrationVersion::new(version)
                        .ok_or(ProviderOperationSchemaPreparationError::InvalidMigrationVersion)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let migrations =
                PendingProviderOperationMigrationVersions::from_ordered_suffix(versions)?;
            Ok(ProviderOperationSchemaPreparation::Pending { migrations })
        }
    }
}

fn invalid_current_schema(
    source: ProviderOperationStoreError,
) -> ProviderOperationSchemaPreparationError {
    ProviderOperationSchemaPreparationError::InvalidCurrentSchema { source }
}

fn map_history_error(error: MigrationHistoryError) -> ProviderOperationSchemaPreparationError {
    match error {
        MigrationHistoryError::TableCheck(source) | MigrationHistoryError::RowsRead(source) => {
            ProviderOperationSchemaPreparationError::UnreadableStore { source }
        }
        MigrationHistoryError::UnrecognizedNativeHistory => {
            ProviderOperationSchemaPreparationError::UnrecognizedNativeHistory
        }
        MigrationHistoryError::InvalidHistory => {
            ProviderOperationSchemaPreparationError::InvalidMigrationHistory
        }
        MigrationHistoryError::InvalidVersion => {
            ProviderOperationSchemaPreparationError::InvalidMigrationVersion
        }
        MigrationHistoryError::DirtyMigration => {
            ProviderOperationSchemaPreparationError::DirtyMigration
        }
        MigrationHistoryError::ChecksumMismatch => {
            ProviderOperationSchemaPreparationError::ChecksumMismatch
        }
        MigrationHistoryError::SchemaNewerThanImage => {
            ProviderOperationSchemaPreparationError::SchemaNewerThanImage
        }
        MigrationHistoryError::UnknownAppliedMigration => {
            ProviderOperationSchemaPreparationError::UnknownAppliedMigration
        }
        MigrationHistoryError::InvalidAppliedOrder => {
            ProviderOperationSchemaPreparationError::InvalidAppliedOrder
        }
    }
}
