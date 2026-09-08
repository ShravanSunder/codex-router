//! Database connection lifecycle; schema setup precedes every repository operation.
use sqlx::{
    Connection, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{path::Path, time::Duration};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("automation storage unavailable")]
    Database(#[from] sqlx::Error),
    #[error("automation database has unsupported schema version {0}")]
    UnsupportedVersion(i64),
    #[error("automation database contains an unexpected schema; existing data was not replaced")]
    InvalidSchema,
    #[error("operation identity already belongs to a different request")]
    OperationConflict,
    #[error("automation record is invalid or inconsistent")]
    InvalidRecord,
    #[error("instruction was not found in this automation database")]
    InstructionNotFound,
    #[error("instruction changed; inspect the current revision before editing")]
    RevisionConflict,
}

pub struct AutomationStore {
    pub(crate) connection: SqliteConnection,
}
impl AutomationStore {
    pub async fn open(path: &Path) -> Result<Self, StorageError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Delete)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(1));
        let mut connection = SqliteConnection::connect_with(&options).await?;
        crate::schema_initialization::initialize(&mut connection).await?;
        Ok(Self { connection })
    }
    pub async fn close(self) -> Result<(), StorageError> {
        self.connection.close().await?;
        Ok(())
    }
}
