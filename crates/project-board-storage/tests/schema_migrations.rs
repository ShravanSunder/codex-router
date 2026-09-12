#![allow(clippy::unwrap_used)]
use project_board_storage::BoardStore;
use sqlx::{Connection, Row, SqliteConnection};

#[tokio::test]
async fn fresh_schema_reopens_with_single_seed_and_foreign_keys() {
    let path = std::env::temp_dir().join(format!("board-schema-{}.sqlite", uuid::Uuid::now_v7()));
    let store = BoardStore::open(&path).await.unwrap();
    store.close().await.unwrap();
    let mut store = BoardStore::open(&path).await.unwrap();
    assert!(store.foreign_keys_enabled().await.unwrap());
    store.close().await.unwrap();
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let row = sqlx::query("SELECT count(*) AS count FROM activity_checkpoint")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("count"), 1);
    connection.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn changed_schema_is_rejected_without_replacing_data() {
    let path = std::env::temp_dir().join(format!(
        "board-schema-changed-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    BoardStore::open(&path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("INSERT INTO board_projects(project_id,name,description) VALUES('retained','project','description')").execute(&mut connection).await.unwrap();
    sqlx::query("DROP INDEX board_projects_name_unique")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert!(BoardStore::open(&path).await.is_err());
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let name: String =
        sqlx::query_scalar("SELECT name FROM board_projects WHERE project_id='retained'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(name, "project");
    connection.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn untracked_schema_is_not_adopted() {
    let path = std::env::temp_dir().join(format!(
        "board-schema-untracked-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let mut connection = SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TABLE unexpected (value TEXT)")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert!(BoardStore::open(&path).await.is_err());
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn extra_checkpoint_row_is_rejected() {
    let path =
        std::env::temp_dir().join(format!("board-checkpoint-{}.sqlite", uuid::Uuid::now_v7()));
    BoardStore::open(&path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("INSERT INTO activity_checkpoint(singleton,last_sequence,cursor_key) VALUES(2,0,randomblob(32))").execute(&mut connection).await.unwrap();
    connection.close().await.unwrap();
    assert!(BoardStore::open(&path).await.is_err());
    std::fs::remove_file(path).unwrap();
}
