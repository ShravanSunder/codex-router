//! Own the database connection and fail closed before exposing an invalid schema.
use sqlx::{
    Connection, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{path::Path, time::Duration};

#[derive(Debug, thiserror::Error)]
pub enum BoardStorageError {
    #[error("Board storage is unavailable: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Board schema or migration history is invalid; existing data was not replaced")]
    InvalidSchema,
    #[error("Stored board record is invalid")]
    InvalidRecord,
}

pub struct BoardStore {
    pub(crate) connection: SqliteConnection,
    pub(crate) cursor_key: [u8; 32],
}
impl BoardStore {
    pub async fn open(path: &Path) -> Result<Self, BoardStorageError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(false)
            .journal_mode(SqliteJournalMode::Delete)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(1));
        let mut connection = SqliteConnection::connect_with(&options).await?;
        crate::board_schema_migrations::initialize(&mut connection).await?;
        let key: Vec<u8> =
            sqlx::query_scalar!("SELECT cursor_key FROM activity_checkpoint WHERE singleton=1")
                .fetch_one(&mut connection)
                .await?;
        let cursor_key = key
            .try_into()
            .map_err(|_| BoardStorageError::InvalidSchema)?;
        Ok(Self {
            connection,
            cursor_key,
        })
    }
    pub async fn close(self) -> Result<(), BoardStorageError> {
        self.connection.close().await?;
        Ok(())
    }
    pub async fn foreign_keys_enabled(&mut self) -> Result<bool, BoardStorageError> {
        let value: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut self.connection)
            .await?;
        Ok(value == 1)
    }
}
