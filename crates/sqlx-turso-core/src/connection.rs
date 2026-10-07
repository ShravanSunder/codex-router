use std::{collections::VecDeque, fmt, future};

use sqlx_core::{
    common::StatementCache, connection::Connection, error::Error, transaction::Transaction,
};

use crate::{
    Turso, TursoConnectOptions, TursoStatement, engine_connection::EngineConnection,
    error::map_turso_error,
};

/// One owned Turso engine connection, with the Sync handle when the store is synced
pub struct TursoConnection {
    options: TursoConnectOptions,
    raw: turso::Connection,
    #[cfg(feature = "sync")]
    sync: Option<turso::sync::Database>,
    statements: StatementCache<TursoStatement>,
    transaction_state: TransactionState,
}

impl fmt::Debug for TursoConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TursoConnection")
            .field("options", &self.options)
            .field("transaction_state", &self.transaction_state)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct TransactionState {
    depth: usize,
    pending_rollback_depths: VecDeque<usize>,
    rollback_failed: bool,
}

impl TransactionState {
    fn depth(&self) -> usize {
        self.depth
    }

    fn increment_depth(&mut self) {
        self.depth += 1;
    }

    fn decrement_depth(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
        }
    }

    fn mark_rollback_needed(&mut self) {
        if self.depth == 0 {
            return;
        }

        self.pending_rollback_depths.push_back(self.depth);
        self.decrement_depth();
    }

    fn pop_pending_rollback(&mut self) -> Option<usize> {
        self.pending_rollback_depths.pop_front()
    }

    fn has_failed_rollback(&self) -> bool {
        self.rollback_failed
    }

    fn mark_rollback_failed(&mut self) {
        self.rollback_failed = true;
        self.pending_rollback_depths.clear();
    }
}

impl TursoConnection {
    pub(crate) fn new(options: TursoConnectOptions, connection: EngineConnection) -> Self {
        Self {
            statements: StatementCache::new(options.get_statement_cache_capacity()),
            options,
            raw: connection.raw,
            #[cfg(feature = "sync")]
            sync: connection.sync,
            transaction_state: TransactionState::default(),
        }
    }

    pub(crate) fn transaction_depth(&self) -> usize {
        self.transaction_state.depth()
    }

    pub(crate) fn increment_transaction_depth(&mut self) {
        self.transaction_state.increment_depth();
    }

    pub(crate) fn decrement_transaction_depth(&mut self) {
        self.transaction_state.decrement_depth();
    }

    pub(crate) fn mark_rollback_needed(&mut self) {
        self.transaction_state.mark_rollback_needed();
    }

    pub(crate) async fn clear_pending_rollback(&mut self) -> Result<(), Error> {
        if self.transaction_state.has_failed_rollback() {
            return Err(Error::WorkerCrashed);
        }

        while let Some(depth) = self.transaction_state.pop_pending_rollback() {
            let sql = sqlx_core::transaction::rollback_ansi_transaction_sql(depth);
            if let Err(error) = self.raw().execute(sql.as_str(), ()).await {
                if depth == 1 && rollback_error_is_inactive_transaction(&error) {
                    continue;
                }

                self.transaction_state.mark_rollback_failed();
                return Err(map_turso_error(error));
            }
        }

        Ok(())
    }

    /// Returns the options used to create this connection
    pub fn options(&self) -> &TursoConnectOptions {
        &self.options
    }

    pub(crate) fn raw(&self) -> &turso::Connection {
        &self.raw
    }

    /// Pushes local changes to the Sync remote
    ///
    /// Bound the wait with a timeout. A push that times out or is dropped may still have been
    /// applied remotely, so callers retry with the same request identity rather than assume it
    /// failed.
    #[cfg(feature = "sync")]
    pub async fn sync_push(&self) -> Result<(), Error> {
        self.sync_database()?.push().await.map_err(map_turso_error)
    }

    /// Pulls remote changes into this connection's database
    ///
    /// Returns whether any change was applied. The same connection and its cached statements
    /// read the pulled rows.
    #[cfg(feature = "sync")]
    pub async fn sync_pull(&self) -> Result<bool, Error> {
        self.sync_database()?.pull().await.map_err(map_turso_error)
    }

    /// Checkpoints the synced database's write-ahead log
    #[cfg(feature = "sync")]
    pub async fn sync_checkpoint(&self) -> Result<(), Error> {
        self.sync_database()?
            .checkpoint()
            .await
            .map_err(map_turso_error)
    }

    /// Returns Sync statistics for this connection's database
    #[cfg(feature = "sync")]
    pub async fn sync_stats(&self) -> Result<turso::sync::DatabaseSyncStats, Error> {
        self.sync_database()?.stats().await.map_err(map_turso_error)
    }

    #[cfg(feature = "sync")]
    fn sync_database(&self) -> Result<&turso::sync::Database, Error> {
        self.sync
            .as_ref()
            .ok_or_else(|| crate::TursoAdapterError::NotSyncConnection.into())
    }

    pub(crate) fn cached_statement(&mut self, sql: &str) -> Option<TursoStatement> {
        self.statements.get_mut(sql).cloned()
    }

    pub(crate) fn cache_statement(&mut self, sql: &str, statement: TursoStatement) {
        if self.statements.is_enabled() {
            self.statements.insert(sql, statement);
        }
    }
}

fn rollback_error_is_inactive_transaction(error: &turso::Error) -> bool {
    error
        .to_string()
        .contains("cannot rollback - no transaction is active")
}

impl Connection for TursoConnection {
    type Database = Turso;
    type Options = TursoConnectOptions;

    async fn close(self) -> Result<(), Error> {
        Ok(())
    }

    async fn close_hard(self) -> Result<(), Error> {
        Ok(())
    }

    async fn ping(&mut self) -> Result<(), Error> {
        let _ = self.raw();
        Ok(())
    }

    async fn begin(&mut self) -> Result<Transaction<'_, Self::Database>, Error> {
        Transaction::begin(self, None).await
    }

    fn shrink_buffers(&mut self) {}

    async fn flush(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn should_flush(&self) -> bool {
        false
    }

    fn cached_statements_size(&self) -> usize
    where
        Self::Database: sqlx_core::database::HasStatementCache,
    {
        self.statements.len()
    }

    fn clear_cached_statements(&mut self) -> impl Future<Output = Result<(), Error>> + Send + '_
    where
        Self::Database: sqlx_core::database::HasStatementCache,
    {
        self.statements.clear();
        future::ready(Ok(()))
    }
}

impl AsRef<TursoConnectOptions> for TursoConnection {
    fn as_ref(&self) -> &TursoConnectOptions {
        &self.options
    }
}

#[cfg(all(test, feature = "sync"))]
mod tests {
    use sqlx_core::{
        connection::{ConnectOptions, Connection},
        error::Error,
        executor::Executor,
        row::Row,
    };

    use crate::{TursoAdapterError, TursoConnectOptions, TursoSyncOptions};

    #[tokio::test]
    async fn sync_operations_on_a_local_connection_are_rejected() -> sqlx_core::Result<()> {
        // Arrange
        let connection = TursoConnectOptions::new().connect().await?;

        // Act
        let error = connection
            .sync_push()
            .await
            .expect_err("not a synced store");

        // Assert
        assert!(matches!(
            error,
            Error::Configuration(ref source)
                if source.downcast_ref::<TursoAdapterError>()
                    == Some(&TursoAdapterError::NotSyncConnection)
        ));
        connection.close().await
    }

    #[tokio::test]
    async fn synced_store_executes_locally_without_bootstrap() -> sqlx_core::Result<()> {
        // Arrange: an unreachable remote, so every step below is local-only
        let directory = tempfile::tempdir()?;
        let mut connection = TursoConnectOptions::new()
            .filename(directory.path().join("synced.db"))
            .create_if_missing(true)
            .with_sync_options(
                TursoSyncOptions::new("http://127.0.0.1:9")
                    .with_bootstrap_if_empty(false)
                    .with_client_name("sqlx-turso-test"),
            )
            .connect()
            .await?;

        // Act
        (&mut connection)
            .execute("CREATE TABLE local_rows (id INTEGER PRIMARY KEY)")
            .await?;
        (&mut connection)
            .execute("INSERT INTO local_rows (id) VALUES (1)")
            .await?;
        let row = (&mut connection)
            .fetch_one("SELECT COUNT(*) AS count FROM local_rows")
            .await?;
        let _stats = connection.sync_stats().await?;
        connection.sync_checkpoint().await?;

        // Assert
        assert_eq!(row.try_get::<i64, _>("count")?, 1);
        Ok(())
    }
}
