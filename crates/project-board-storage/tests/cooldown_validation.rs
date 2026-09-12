#![allow(clippy::unwrap_used)]

mod board_behavior_support;

use board_behavior_support::{actor, create_fixture, database_path, no_references, text};
use project_board::*;
use project_board_storage::BoardStore;
use sqlx::{Connection, SqliteConnection};
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn malformed_cooldown_timestamps_are_rejected_as_attributed_invalid_records() {
    for (label, stored_timestamp) in [
        ("negative", -1_i64),
        ("subtraction-overflow", i64::MIN),
        ("backwards-clock", i64::MAX),
    ] {
        let (path, fixture) = prepared_cooldown(label).await;
        overwrite_cooldown(&path, stored_timestamp).await;
        let mut store = BoardStore::open(&path).await.unwrap();

        let failure = store
            .post_message(MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Topic {
                    topic_id: fixture.topic_id,
                },
                actor: actor("cooldown-reader"),
                acting_for: None,
                text: text("must reject corrupt cooldown"),
                references: no_references(),
            })
            .await
            .unwrap_err();

        assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
        assert_eq!(failure.stage, BoardFailureStage::Inspection);
        assert_eq!(failure.next_action, BoardNextAction::InspectResource);
        assert_eq!(
            failure.details,
            BoardErrorDetails::Resource {
                resource: ResourceIdentity::Board {
                    board_id: fixture.board_id,
                },
            }
        );
        store.close().await.unwrap();
        std::fs::remove_file(path).unwrap();
    }
}

#[tokio::test]
async fn cooldown_allows_a_top_level_post_at_the_thirty_second_boundary() {
    let (path, fixture) = prepared_cooldown("exact-boundary").await;
    let now = current_time_millis();
    overwrite_cooldown(&path, now.checked_sub(30_000).unwrap()).await;
    let mut store = BoardStore::open(&path).await.unwrap();

    let posted = store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic {
                topic_id: fixture.topic_id,
            },
            actor: actor("cooldown-reader"),
            acting_for: None,
            text: text("allowed at boundary"),
            references: no_references(),
        })
        .await
        .unwrap();

    assert_eq!(posted.message.board_id, fixture.board_id);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

async fn prepared_cooldown(label: &str) -> (std::path::PathBuf, board_behavior_support::Fixture) {
    let path = database_path(label);
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic {
                topic_id: fixture.topic_id.clone(),
            },
            actor: actor("cooldown-reader"),
            acting_for: None,
            text: text("establish cooldown"),
            references: no_references(),
        })
        .await
        .unwrap();
    store.close().await.unwrap();
    (path, fixture)
}

async fn overwrite_cooldown(path: &std::path::Path, stored_timestamp: i64) {
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE actor_board_cooldowns SET last_post_at_ms=?")
        .bind(stored_timestamp)
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
}

fn current_time_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}
