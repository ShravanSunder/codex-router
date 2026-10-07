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

pub(crate) async fn prepare_schema(
    database_path: &Path,
) -> Result<AccountSchemaPreparation, StateSchemaPreparationError> {
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::ZERO)
        .pragma("query_only", "ON");
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(|source| StateSchemaPreparationError::UnreadableStore { source })?;

    let preparation = inspect_connection(&mut connection).await;
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

async fn inspect_connection(
    connection: &mut SqliteConnection,
) -> Result<AccountSchemaPreparation, StateSchemaPreparationError> {
    match crate::account_migrations::migration_history::inspect_for_preparation(connection)
        .await
        .map_err(map_history_error)?
    {
        SchemaPreparationHistory::Current => {
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
