#![allow(clippy::unwrap_used)]
mod board_behavior_support;

use board_behavior_support::*;
use project_board::Placement;
use project_board_storage::BoardStore;
use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn sqlite_rejects_non_boolean_watch_and_summary_values() {
    let path = database_path("boolean-checks");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id,
        },
        actor("author"),
        "Create a watch and its project summary",
        vec![],
    )
    .await;
    store.close().await.unwrap();
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    for valid_value in [0_i64, 1] {
        let watch = sqlx::query("UPDATE thread_watches SET active=?")
            .bind(valid_value)
            .execute(&mut connection)
            .await
            .unwrap();
        let summary = sqlx::query("UPDATE project_reader_state SET has_unread=?")
            .bind(valid_value)
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(watch.rows_affected(), 1);
        assert_eq!(summary.rows_affected(), 1);
    }
    for invalid_value in [-1_i64, 2] {
        assert!(matches!(
            sqlx::query("UPDATE thread_watches SET active=?")
                .bind(invalid_value)
                .execute(&mut connection)
                .await,
            Err(sqlx::Error::Database(_))
        ));
        assert!(matches!(
            sqlx::query("UPDATE project_reader_state SET has_unread=?")
                .bind(invalid_value)
                .execute(&mut connection)
                .await,
            Err(sqlx::Error::Database(_))
        ));
    }
    let active: i64 = sqlx::query_scalar("SELECT active FROM thread_watches")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let has_unread: i64 = sqlx::query_scalar("SELECT has_unread FROM project_reader_state")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(active, 1);
    assert_eq!(has_unread, 1);
    connection.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
