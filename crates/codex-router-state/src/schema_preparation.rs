use std::future::Future;
use std::num::NonZeroI64;
use std::path::Path;
use std::time::Duration;

use sqlx::Connection;
use sqlx::SqliteConnection;
use sqlx::sqlite::SqliteConnectOptions;

use crate::account_migrations::migration_history::MigrationHistoryError;
use crate::account_migrations::migration_history::SchemaPreparationHistory;
use crate::account_schema::validate_required_read_only_objects;
use crate::account_schema::validate_target_schema;
use crate::sqlite::StateStoreError;

pub use crate::schema_preparation_error::StateSchemaPreparationError;

/// A positive migration version embedded in the account-state image.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AccountMigrationVersion(NonZeroI64);

impl AccountMigrationVersion {
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
pub struct PendingAccountMigrationVersions(Vec<AccountMigrationVersion>);

impl PendingAccountMigrationVersions {
    /// Returns the remaining versions in the embedded migration order.
    #[must_use]
    pub fn as_slice(&self) -> &[AccountMigrationVersion] {
        &self.0
    }

    fn from_ordered_suffix(
        versions: Vec<AccountMigrationVersion>,
    ) -> Result<Self, StateSchemaPreparationError> {
        if versions.is_empty()
            || versions.windows(2).any(|pair| match pair {
                [first, second] => first >= second,
                _ => false,
            })
        {
            return Err(StateSchemaPreparationError::InvalidAppliedOrder);
        }
        Ok(Self(versions))
    }
}

/// The account database's read-only schema-preparation result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccountSchemaPreparation {
    /// The database already has this image's complete, valid schema.
    Current,
    /// The database has a valid applied prefix and these migrations remain.
    Pending {
        /// The ordered, nonempty remainder of this image's migration set.
        migrations: PendingAccountMigrationVersions,
    },
}

/// An existing database's validated Fresh-only bootstrap class; never a native-history claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountBootstrapKind {
    ExistingEmpty,
    RecognizedLegacy,
}
/// State-owned, nonserialized observation for a Fresh startup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccountStartupSchemaPreparation {
    Native { schema: AccountSchemaPreparation },
    Bootstrap { kind: AccountBootstrapKind },
}

pub(crate) async fn prepare_startup_schema(
    path: &Path,
) -> Result<AccountStartupSchemaPreparation, StateSchemaPreparationError> {
    let mut connection = open_preparation_connection(path).await?;
    let preparation = match connection.begin().await {
        Ok(mut transaction) => {
            let inspection = inspect_startup_connection(&mut transaction).await;
            let rollback = transaction
                .rollback()
                .await
                .map_err(|source| StateSchemaPreparationError::UnreadableStore { source });
            match inspection {
                Ok(prepared) => rollback.map(|()| prepared),
                Err(error) => Err(error),
            }
        }
        Err(source) => Err(StateSchemaPreparationError::UnreadableStore { source }),
    };
    let closed = connection.close().await;
    match preparation {
        Ok(prepared) => {
            closed.map_err(|source| StateSchemaPreparationError::UnreadableStore { source })?;
            Ok(prepared)
        }
        Err(error) => Err(error),
    }
}
async fn inspect_startup_connection(
    connection: &mut SqliteConnection,
) -> Result<AccountStartupSchemaPreparation, StateSchemaPreparationError> {
    if crate::account_migrations::native_history_table_exists(connection)
        .await
        .map_err(|source| StateSchemaPreparationError::UnreadableStore { source })?
    {
        return inspect_connection(connection, || async {})
            .await
            .map(|schema| AccountStartupSchemaPreparation::Native { schema });
    }
    crate::account_migrations::inspect_bootstrap_schema(connection)
        .await
        .map(|kind| AccountStartupSchemaPreparation::Bootstrap { kind })
        .map_err(|source| StateSchemaPreparationError::InvalidBootstrapSchema { source })
}
async fn open_preparation_connection(
    path: &Path,
) -> Result<SqliteConnection, StateSchemaPreparationError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::ZERO)
        .pragma("query_only", "ON");
    SqliteConnection::connect_with(&options)
        .await
        .map_err(|source| StateSchemaPreparationError::UnreadableStore { source })
}

pub(crate) async fn prepare_schema(
    database_path: &Path,
) -> Result<AccountSchemaPreparation, StateSchemaPreparationError> {
    prepare_schema_with_checkpoint(database_path, || async {}).await
}

pub(super) async fn prepare_schema_with_checkpoint<FCheckpoint, FCheckpointFuture>(
    database_path: &Path,
    after_history_read: FCheckpoint,
) -> Result<AccountSchemaPreparation, StateSchemaPreparationError>
where
    FCheckpoint: FnOnce() -> FCheckpointFuture + Send,
    FCheckpointFuture: Future<Output = ()> + Send,
{
    let mut connection = open_preparation_connection(database_path).await?;

    let preparation = match connection.begin().await {
        Ok(mut transaction) => {
            let inspection = inspect_connection(&mut transaction, after_history_read).await;
            let rollback = transaction
                .rollback()
                .await
                .map_err(|source| StateSchemaPreparationError::UnreadableStore { source });
            match inspection {
                Ok(preparation) => rollback.map(|()| preparation),
                Err(error) => Err(error),
            }
        }
        Err(source) => Err(StateSchemaPreparationError::UnreadableStore { source }),
    };
    let close_result = connection.close().await;
    match preparation {
        Ok(preparation) => {
            close_result
                .map_err(|source| StateSchemaPreparationError::UnreadableStore { source })?;
            Ok(preparation)
        }
        Err(error) => Err(error),
    }
}

async fn inspect_connection<FCheckpoint, FCheckpointFuture>(
    connection: &mut SqliteConnection,
    after_history_read: FCheckpoint,
) -> Result<AccountSchemaPreparation, StateSchemaPreparationError>
where
    FCheckpoint: FnOnce() -> FCheckpointFuture + Send,
    FCheckpointFuture: Future<Output = ()> + Send,
{
    match crate::account_migrations::migration_history::inspect_for_preparation(connection)
        .await
        .map_err(map_history_error)?
    {
        SchemaPreparationHistory::Current => {
            after_history_read().await;
            validate_current_schema(connection).await?;
            Ok(AccountSchemaPreparation::Current)
        }
        SchemaPreparationHistory::Pending(versions) => {
            let versions = versions
                .into_iter()
                .map(|version| {
                    AccountMigrationVersion::new(version)
                        .ok_or(StateSchemaPreparationError::InvalidMigrationVersion)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let migrations = PendingAccountMigrationVersions::from_ordered_suffix(versions)?;
            Ok(AccountSchemaPreparation::Pending { migrations })
        }
    }
}

async fn validate_current_schema(
    connection: &mut SqliteConnection,
) -> Result<(), StateSchemaPreparationError> {
    validate_required_read_only_objects(&mut *connection)
        .await
        .map_err(invalid_current_schema)?;
    validate_target_schema(connection)
        .await
        .map_err(invalid_current_schema)
}

fn invalid_current_schema(source: StateStoreError) -> StateSchemaPreparationError {
    StateSchemaPreparationError::InvalidCurrentSchema { source }
}

fn map_history_error(error: MigrationHistoryError) -> StateSchemaPreparationError {
    match error {
        MigrationHistoryError::TableCheck(source) | MigrationHistoryError::RowsRead(source) => {
            StateSchemaPreparationError::UnreadableStore { source }
        }
        MigrationHistoryError::UnrecognizedNativeHistory => {
            StateSchemaPreparationError::UnrecognizedNativeHistory
        }
        MigrationHistoryError::InvalidHistory => {
            StateSchemaPreparationError::InvalidMigrationHistory
        }
        MigrationHistoryError::InvalidVersion => {
            StateSchemaPreparationError::InvalidMigrationVersion
        }
        MigrationHistoryError::DirtyMigration => StateSchemaPreparationError::DirtyMigration,
        MigrationHistoryError::ChecksumMismatch => StateSchemaPreparationError::ChecksumMismatch,
        MigrationHistoryError::SchemaNewerThanImage => {
            StateSchemaPreparationError::SchemaNewerThanImage
        }
        MigrationHistoryError::UnknownAppliedMigration => {
            StateSchemaPreparationError::UnknownAppliedMigration
        }
        MigrationHistoryError::InvalidAppliedOrder => {
            StateSchemaPreparationError::InvalidAppliedOrder
        }
    }
}
