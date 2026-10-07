use thiserror::Error;

use crate::sqlite::StateStoreError;

/// A closed reason that an existing account database cannot be prepared.
#[derive(Debug, Error)]
pub enum StateSchemaPreparationError {
    /// The account database could not be opened or read without writes.
    #[error("account database is unavailable for schema preparation")]
    UnreadableStore {
        /// The underlying SQLite failure.
        #[source]
        source: sqlx::Error,
    },
    /// The database does not have native SQLx migration history.
    #[error("account database has no recognized native migration history")]
    UnrecognizedNativeHistory,
    /// The database contains a migration that this image does not know.
    #[error("account database schema is newer than this image")]
    SchemaNewerThanImage,
    /// A native migration is recorded as unsuccessful.
    #[error("account database contains a dirty migration")]
    DirtyMigration,
    /// A stored migration checksum does not match this image.
    #[error("account database migration checksum does not match this image")]
    ChecksumMismatch,
    /// Applied migrations are not exactly an ordered prefix of this image.
    #[error("account database has invalid applied migration order")]
    InvalidAppliedOrder,
    /// A stored migration version is not present in this image.
    #[error("account database contains an unknown applied migration")]
    UnknownAppliedMigration,
    /// A stored migration version is not a positive SQLite integer.
    #[error("account database contains an invalid migration version")]
    InvalidMigrationVersion,
    /// A native migration history row has an invalid stored value.
    #[error("account database contains malformed migration history")]
    InvalidMigrationHistory,
    /// The fully migrated database does not have the current schema.
    #[error("account database current schema is invalid")]
    InvalidCurrentSchema {
        /// The existing owner validator's failure.
        #[source]
        source: StateStoreError,
    },
}
