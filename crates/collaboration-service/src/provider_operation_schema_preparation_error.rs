use crate::ProviderOperationStoreError;

#[derive(Debug, thiserror::Error)]
pub enum ProviderOperationSchemaPreparationError {
    #[error("provider operation database could not be read")]
    UnreadableStore {
        #[source]
        source: sqlx::Error,
    },
    #[error("provider operation database has no recognized native migration history")]
    UnrecognizedNativeHistory,
    #[error("provider operation migration history is invalid")]
    InvalidMigrationHistory,
    #[error("provider operation migration version is invalid")]
    InvalidMigrationVersion,
    #[error("provider operation database contains an unsuccessful migration")]
    DirtyMigration,
    #[error("provider operation migration checksum does not match this image")]
    ChecksumMismatch,
    #[error("provider operation schema is newer than this image")]
    SchemaNewerThanImage,
    #[error("provider operation database contains an unknown migration")]
    UnknownAppliedMigration,
    #[error("provider operation migration history is not an exact migration prefix")]
    InvalidAppliedOrder,
    #[error("current provider operation schema is invalid")]
    InvalidCurrentSchema {
        #[source]
        source: ProviderOperationStoreError,
    },
}
