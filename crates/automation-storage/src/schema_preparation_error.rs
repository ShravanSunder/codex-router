use crate::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum AutomationSchemaPreparationError {
    #[error("automation database could not be read")]
    UnreadableStore {
        #[source]
        source: sqlx::Error,
    },
    #[error("automation database has no recognized native migration history")]
    UnrecognizedNativeHistory,
    #[error("automation database migration history is invalid")]
    InvalidMigrationHistory,
    #[error("automation database migration version is invalid")]
    InvalidMigrationVersion,
    #[error("automation database contains an unsuccessful migration")]
    DirtyMigration,
    #[error("automation database migration checksum does not match this image")]
    ChecksumMismatch,
    #[error("automation database schema is newer than this image")]
    SchemaNewerThanImage,
    #[error("automation database contains an unknown migration")]
    UnknownAppliedMigration,
    #[error("automation database migration history is not an exact migration prefix")]
    InvalidAppliedOrder,
    #[error("current automation schema is invalid")]
    InvalidCurrentSchema {
        #[source]
        source: StorageError,
    },
}
