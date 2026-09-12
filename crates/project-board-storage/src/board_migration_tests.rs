#![allow(clippy::unwrap_used)]
//! Real SQLx migration savepoints preserve parent/child data and outer rollback.
use super::*;
use sqlx::SqlSafeStr;
use sqlx::migrate::{Migration, MigrationType, Migrator};
use std::borrow::Cow;

fn migrator(extra: &str) -> Migrator {
    let migrations = vec![
        Migration::new(
            202609120001,
            "project board".into(),
            MigrationType::Simple,
            BASELINE.into_sql_str(),
            false,
        ),
        Migration::new(
            202609120002,
            "test-only rebuild".into(),
            MigrationType::Simple,
            sqlx::AssertSqlSafe(extra.to_owned()).into_sql_str(),
            false,
        ),
    ];
    Migrator {
        migrations: Cow::Owned(migrations),
        ..Migrator::DEFAULT
    }
}
async fn populated() -> SqliteConnection {
    let mut connection = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    initialize(&mut connection).await.unwrap();
    sqlx::query("INSERT INTO board_projects VALUES('project','Project','description')")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_boards VALUES('board','project','Board','','active')")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    connection
}
const REBUILD: &str = "CREATE TABLE replacement_projects(project_id TEXT PRIMARY KEY NOT NULL,name TEXT NOT NULL,description TEXT NOT NULL,note TEXT) STRICT; INSERT INTO replacement_projects(project_id,name,description) SELECT project_id,name,description FROM board_projects; DROP TABLE board_projects; ALTER TABLE replacement_projects RENAME TO board_projects; CREATE UNIQUE INDEX board_projects_name_unique ON board_projects(name);";

#[tokio::test]
async fn populated_parent_rebuild_preserves_children_and_history() {
    let mut connection = populated().await;
    let expected = format!("{BASELINE} {REBUILD}");
    initialize_with(&mut connection, &migrator(REBUILD), &expected)
        .await
        .unwrap();
    let project: String =
        sqlx::query_scalar("SELECT project_id FROM project_boards WHERE board_id='board'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(project, "project");
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut connection)
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn failed_rebuild_rolls_back_parent_and_sqlx_history() {
    let mut connection = populated().await;
    let failing = format!("{REBUILD} INSERT INTO missing_table VALUES(1);");
    assert!(
        initialize_with(&mut connection, &migrator(&failing), BASELINE)
            .await
            .is_err()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM board_projects WHERE project_id='project'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let history: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE version=202609120002")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(history, 0);
}

#[tokio::test]
async fn broken_relationship_rejects_migration_before_commit() {
    let mut connection = populated().await;
    let invalid = "UPDATE project_boards SET project_id='missing' WHERE board_id='board';";
    assert!(
        initialize_with(&mut connection, &migrator(invalid), BASELINE)
            .await
            .is_err()
    );
    let parent: String =
        sqlx::query_scalar("SELECT project_id FROM project_boards WHERE board_id='board'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(parent, "project");
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut connection)
            .await
            .unwrap()
            .is_empty()
    );
}
