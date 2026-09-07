//! Normal Codex state is opened read-only; no alternate history database is created.
use crate::{StoredThreadQuery, stored_thread_page_query};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
};
use std::{path::Path, time::Duration};

pub struct StoredThreadCatalog {
    pool: SqlitePool,
}
impl StoredThreadCatalog {
    pub async fn open(codex_home: &Path) -> Result<Self, sqlx::Error> {
        let options = SqliteConnectOptions::new()
            .filename(codex_home.join("state_5.sqlite"))
            .read_only(true)
            .create_if_missing(false)
            .busy_timeout(Duration::from_millis(0))
            .pragma("query_only", "ON");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }
    pub async fn read_page(
        &self,
        query: &StoredThreadQuery,
    ) -> Result<Vec<SqliteRow>, sqlx::Error> {
        stored_thread_page_query(query)
            .build()
            .fetch_all(&self.pool)
            .await
    }
    pub async fn close(self) {
        self.pool.close().await;
    }
}
