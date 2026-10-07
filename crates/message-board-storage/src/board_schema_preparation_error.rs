use crate::BoardStorageError;

#[derive(Debug, thiserror::Error)]
pub enum BoardSchemaPreparationError {
    #[error("board database could not be read")]
    UnreadableStore {
        #[source]
        source: sqlx::Error,
    },
    #[error("board database has no recognized native migration history")]
    UnrecognizedNativeHistory,
    #[error("board database migration history is invalid")]
    InvalidMigrationHistory,
    #[error("board database migration version is invalid")]
    InvalidMigrationVersion,
    #[error("board database contains an unsuccessful migration")]
    DirtyMigration,
    #[error("board database migration checksum does not match this image")]
    ChecksumMismatch,
    #[error("board database schema is newer than this image")]
    SchemaNewerThanImage,
    #[error("board database contains an unknown migration")]
    UnknownAppliedMigration,
    #[error("board database migration history is not an exact migration prefix")]
    InvalidAppliedOrder,
    #[error("current board schema is invalid")]
    InvalidCurrentSchema {
        #[source]
        source: BoardStorageError,
    },
}
