use std::path::Path;

use sqlx::Connection;

use super::MIGRATOR;
use super::migrate_and_rollback_for_test;
use super::migrate_with_process_checkpoint_for_test;
use crate::sqlite::AsyncSqliteStateStore;

mod fixtures;

use fixtures::*;

mod history_contracts;
mod legacy_shapes;
mod presence_fixtures;
mod schema_contracts;
mod transaction_recovery;

use presence_fixtures::*;

#[tokio::test]
async fn migration_process_helper() {
    let Ok(database_path) = std::env::var("CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_DATABASE") else {
        return;
    };
    let commit_before_checkpoint =
        std::env::var_os("CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_COMMIT_BEFORE_CHECKPOINT").is_some();
    let pool = open_test_pool(Path::new(&database_path)).await;
    migrate_with_process_checkpoint_for_test(&pool, commit_before_checkpoint)
        .await
        .expect("migration child should reach and leave checkpoint");
    pool.close().await;
}
