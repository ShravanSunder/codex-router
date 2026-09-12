#![allow(clippy::unwrap_used)]
//! Real file-backed SQLx migration proofs over populated board-domain state.
use super::*;
use crate::BoardStore;
use sqlx::SqlSafeStr;
use sqlx::migrate::{Migration, MigrationType, Migrator};
use std::borrow::Cow;

#[path = "board_migration_test_support.rs"]
mod support;
use support::*;

const ADDITIVE: &str = "ALTER TABLE board_projects ADD COLUMN migration_note TEXT; CREATE INDEX board_projects_migration_note ON board_projects(migration_note);";
const REBUILD: &str = "CREATE TABLE replacement_projects(project_id TEXT PRIMARY KEY NOT NULL,name TEXT NOT NULL,description TEXT NOT NULL) STRICT; INSERT INTO replacement_projects(project_id,name,description) SELECT project_id,name,description FROM board_projects; DROP TABLE board_projects; ALTER TABLE replacement_projects RENAME TO board_projects; CREATE UNIQUE INDEX board_projects_name_unique ON board_projects(name);";

fn migrator(version: i64, description: &'static str, extra: &str) -> Migrator {
    Migrator {
        migrations: Cow::Owned(vec![
            Migration::new(
                202609120001,
                "project board".into(),
                MigrationType::Simple,
                BASELINE.into_sql_str(),
                false,
            ),
            Migration::new(
                version,
                description.into(),
                MigrationType::Simple,
                sqlx::AssertSqlSafe(extra.to_owned()).into_sql_str(),
                false,
            ),
        ]),
        ..Migrator::DEFAULT
    }
}

#[tokio::test]
async fn additive_migration_preserves_semantic_state_and_history() {
    let mut fixture = PopulatedBoard::create("additive").await;
    let before = fixture.snapshot().await;
    let mut connection = fixture.close_and_connect_for_migration().await;
    let expected = format!("{BASELINE} {ADDITIVE}");
    initialize_with(
        &mut connection,
        &migrator(202609120002, "test-only additive", ADDITIVE),
        &expected,
    )
    .await
    .unwrap();
    let mut store = store_from_connection(connection).await;
    assert_eq!(fixture.snapshot_with(&mut store).await, before);
    assert_eq!(
        migration_versions(&mut store.connection).await,
        vec![202609120001, 202609120002]
    );
    assert!(index_exists(&mut store.connection, "board_projects_migration_note").await);
    assert!(
        foreign_key_violations(&mut store.connection)
            .await
            .is_empty()
    );
    fixture.finish(store).await;
}

#[tokio::test]
async fn populated_parent_rebuild_preserves_exact_rows_domain_reads_and_relationships() {
    let mut fixture = PopulatedBoard::create("rebuild").await;
    let before = fixture.snapshot().await;
    let before_rows = fixture.raw_rows().await;
    let mut connection = fixture.close_and_connect_for_migration().await;
    let expected = format!("{BASELINE} {REBUILD}");
    initialize_with(
        &mut connection,
        &migrator(202609120002, "test-only rebuild", REBUILD),
        &expected,
    )
    .await
    .unwrap();
    let mut store = store_from_connection(connection).await;
    assert_eq!(fixture.snapshot_with(&mut store).await, before);
    assert_eq!(all_domain_rows(&mut store.connection).await, before_rows);
    assert_eq!(
        migration_versions(&mut store.connection).await,
        vec![202609120001, 202609120002]
    );
    for index in [
        "board_projects_name_unique",
        "project_boards_key_2",
        "board_topics_key_2",
        "board_messages_key_1",
    ] {
        assert!(
            index_exists(&mut store.connection, index).await,
            "missing {index}"
        );
    }
    assert!(
        foreign_key_violations(&mut store.connection)
            .await
            .is_empty()
    );
    fixture.finish(store).await;
}

#[tokio::test]
async fn failed_rebuild_reopens_separately_with_original_exact_state_and_history() {
    let mut fixture = PopulatedBoard::create("failed-rebuild").await;
    let before = fixture.snapshot().await;
    let before_rows = fixture.raw_rows().await;
    let before_schema = fixture.schema_objects().await;
    let mut connection = fixture.close_and_connect_for_migration().await;
    let failing = format!("{REBUILD} INSERT INTO missing_table VALUES(1);");
    assert!(
        initialize_with(
            &mut connection,
            &migrator(202609120002, "test-only failing rebuild", &failing),
            BASELINE
        )
        .await
        .is_err()
    );
    connection.close().await.unwrap();
    let mut reopened = BoardStore::open(&fixture.path).await.unwrap();
    assert_eq!(fixture.snapshot_with(&mut reopened).await, before);
    assert_eq!(all_domain_rows(&mut reopened.connection).await, before_rows);
    assert_eq!(
        definitions(&mut reopened.connection).await.unwrap(),
        before_schema
    );
    assert_eq!(
        migration_versions(&mut reopened.connection).await,
        vec![202609120001]
    );
    fixture.finish(reopened).await;
}

#[tokio::test]
async fn broken_relationship_rejects_migration_and_reopens_unchanged() {
    let mut fixture = PopulatedBoard::create("invalid-relationship").await;
    let before_rows = fixture.raw_rows().await;
    let invalid = format!(
        "UPDATE project_boards SET project_id='{}' WHERE board_id='{}';",
        project_board::ProjectId::generate().as_str(),
        fixture.first_board_id.as_str()
    );
    let mut connection = fixture.close_and_connect_for_migration().await;
    assert!(
        initialize_with(
            &mut connection,
            &migrator(202609120002, "test-only invalid relationship", &invalid),
            BASELINE
        )
        .await
        .is_err()
    );
    connection.close().await.unwrap();
    let mut reopened = BoardStore::open(&fixture.path).await.unwrap();
    assert_eq!(all_domain_rows(&mut reopened.connection).await, before_rows);
    assert_eq!(
        migration_versions(&mut reopened.connection).await,
        vec![202609120001]
    );
    assert!(
        foreign_key_violations(&mut reopened.connection)
            .await
            .is_empty()
    );
    fixture.finish(reopened).await;
}

#[tokio::test]
async fn foreign_key_enablement_rejects_an_active_transaction() {
    let mut connection = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("BEGIN").execute(&mut connection).await.unwrap();
    assert!(matches!(
        enable_foreign_keys(&mut connection).await,
        Err(BoardStorageError::InvalidSchema)
    ));
    sqlx::query("ROLLBACK")
        .execute(&mut connection)
        .await
        .unwrap();
}
