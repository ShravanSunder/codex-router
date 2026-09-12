#![allow(clippy::unwrap_used)]
#[path = "board_behavior_support/mod.rs"]
mod board_behavior_support;
use board_behavior_support::*;
use project_board::*;
use project_board_storage::BoardStore;
use sqlx::{Connection, SqliteConnection};

async fn raw_connection(path: &std::path::Path) -> SqliteConnection {
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    connection
}

#[tokio::test]
async fn missing_resource_uses_the_canonical_inspection_action() {
    let path = database_path("missing-resource-error");
    let mut store = BoardStore::open(&path).await.unwrap();
    let failure = store
        .show_project(ProjectShowRequest {
            project_id: ProjectId::generate(),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::ResourceNotFound);
    assert_eq!(failure.stage, BoardFailureStage::Inspection);
    assert_eq!(failure.next_action, BoardNextAction::InspectResource);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn project_main_tracking_rejects_negative_and_future_boundaries() {
    let path = database_path("invalid-main-boundary");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let reader = actor("reader");
    store
        .fetch_inbox(InboxFetchRequest {
            project_id: fixture.project_id.clone(),
            reader: reader.clone(),
            page: page(10),
        })
        .await
        .unwrap();
    let mut connection = raw_connection(&path).await;
    sqlx::query("UPDATE project_reader_state SET main_start=-1 WHERE project_id=?")
        .bind(fixture.project_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&path).await.unwrap();
    let failure = store
        .fetch_inbox(InboxFetchRequest {
            project_id: fixture.project_id.clone(),
            reader: reader.clone(),
            page: page(10),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    let mut connection = raw_connection(&path).await;
    sqlx::query("UPDATE project_reader_state SET main_start=(SELECT last_sequence+1 FROM activity_checkpoint WHERE singleton=1) WHERE project_id=?")
        .bind(fixture.project_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&path).await.unwrap();
    let failure = store
        .list_inbox_projects(InboxProjectsRequest {
            reader,
            unread_only: false,
            page: page(10),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn watch_status_rejects_negative_and_future_start_boundaries() {
    let path = database_path("invalid-watch-boundary");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let watcher = actor("watcher");
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id,
        },
        watcher.clone(),
        "root",
        vec![],
    )
    .await;
    store.close().await.unwrap();

    let mut connection = raw_connection(&path).await;
    sqlx::query("UPDATE thread_watches SET starts_after_activity=-1 WHERE root_id=?")
        .bind(root.message.message_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&path).await.unwrap();
    let failure = store
        .show_thread(ThreadShowRequest {
            root_message_id: root.message.message_id.clone(),
            reader: Some(watcher.clone()),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();

    let mut connection = raw_connection(&path).await;
    sqlx::query("UPDATE thread_watches SET starts_after_activity=(SELECT last_sequence+1 FROM activity_checkpoint WHERE singleton=1) WHERE root_id=?")
        .bind(root.message.message_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&path).await.unwrap();
    let failure = store
        .show_thread(ThreadShowRequest {
            root_message_id: root.message.message_id,
            reader: Some(watcher),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn scoped_bookmarks_reject_negative_and_future_boundaries() {
    let path = database_path("invalid-bookmark-boundary");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let reader = actor("reader");
    store
        .fetch_inbox(InboxFetchRequest {
            project_id: fixture.project_id.clone(),
            reader: reader.clone(),
            page: page(10),
        })
        .await
        .unwrap();
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("author"),
        "root",
        vec![],
    )
    .await;
    store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: reader.clone(),
            acting_for: None,
            scope: ReadScope::Topic {
                topic_id: fixture.topic_id.clone(),
            },
            through_activity_sequence: root.message.activity_sequence,
        })
        .await
        .unwrap();
    store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    let reply = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id.clone(),
        },
        actor("author"),
        "reply",
        vec![],
    )
    .await;
    store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: reader.clone(),
            acting_for: None,
            scope: ReadScope::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            through_activity_sequence: reply.message.activity_sequence,
        })
        .await
        .unwrap();
    let mut connection = raw_connection(&path).await;
    sqlx::query("UPDATE topic_read_bookmarks SET through_activity=-1 WHERE topic_id=?")
        .bind(fixture.topic_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert_eq!(
        store
            .fetch_inbox(InboxFetchRequest {
                project_id: fixture.project_id.clone(),
                reader: reader.clone(),
                page: page(10),
            })
            .await
            .unwrap_err()
            .kind,
        BoardFailureKind::InvalidRecord
    );

    let mut connection = raw_connection(&path).await;
    sqlx::query("UPDATE topic_read_bookmarks SET through_activity=? WHERE topic_id=?")
        .bind(i64::try_from(reply.message.activity_sequence.get()).unwrap())
        .bind(fixture.topic_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert_eq!(
        store
            .fetch_inbox(InboxFetchRequest {
                project_id: fixture.project_id.clone(),
                reader: reader.clone(),
                page: page(10),
            })
            .await
            .unwrap_err()
            .kind,
        BoardFailureKind::InvalidRecord
    );

    let mut connection = raw_connection(&path).await;
    sqlx::query("UPDATE topic_read_bookmarks SET through_activity=? WHERE topic_id=?")
        .bind(i64::try_from(root.message.activity_sequence.get()).unwrap())
        .bind(fixture.topic_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("UPDATE thread_read_bookmarks SET through_activity=(SELECT last_sequence+1 FROM activity_checkpoint WHERE singleton=1) WHERE root_id=?")
        .bind(root.message.message_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert_eq!(
        store
            .list_inbox_projects(InboxProjectsRequest {
                reader,
                unread_only: false,
                page: page(10),
            })
            .await
            .unwrap_err()
            .kind,
        BoardFailureKind::InvalidRecord
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
