use super::*;
use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn empty_topic_subscription_covers_its_first_later_root_with_a_pending_window() {
    let path = std::env::temp_dir().join(format!(
        "thread-subscriptions-empty-topic-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let mut store = BoardStore::open(&path).await.unwrap();
    let now = Utc::now();
    let project_id = ProjectId::generate();
    let board_id = BoardId::generate();
    let topic_id = TopicId::generate();
    store
        .create_project(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: name("Empty topic project"),
            description: description("Boundary zero subscription proof"),
            actor: human("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    store
        .create_board(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id,
            name: name("Empty topic board"),
            description: description("Boundary zero subscription proof"),
            actor: human("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    store
        .create_topic(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id,
            name: name("Empty topic"),
            description: description("Boundary zero subscription proof"),
            actor: human("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    let reader = session("empty-topic-reader");

    let subscription = store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: reader.clone(),
                scope: SubscriptionScope::topic(topic_id.clone()),
                policy: SubscriptionPolicyPatch {
                    mode: Some(SubscriptionMode::Poll),
                    timing: SubscriptionTimingPatch {
                        quiet_seconds: Some(0),
                        cap_seconds: Some(0),
                    },
                    ..SubscriptionPolicyPatch::default()
                },
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(subscription.state(), SubscriptionState::Active);

    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let topic_watch_boundary: i64 = sqlx::query_scalar(
        "SELECT starts_after_activity FROM topic_watches WHERE topic_id=? AND active=1",
    )
    .bind(topic_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(topic_watch_boundary, 0);
    connection.close().await.unwrap();

    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: topic_id.clone(),
        },
        human("first-root-author"),
        "first root after subscription",
        now,
    )
    .await;
    let watch_status = store
        .show_thread(ThreadShowRequest {
            root_message_id: root.message.message_id.clone(),
            reader: Some(reader.clone()),
        })
        .await
        .unwrap()
        .watch_status
        .unwrap();
    assert!(watch_status.watching);
    assert_eq!(
        watch_status
            .starts_after_activity_sequence
            .map(|sequence| sequence.get()),
        Some(0)
    );

    let subscription = store
        .get_thread_subscription_record(&reader, &SubscriptionScope::topic(topic_id.clone()))
        .await
        .unwrap()
        .unwrap();
    let pending_window = subscription
        .roots()
        .iter()
        .find(|window| window.root_message_id() == &root.message.message_id);
    assert_eq!(pending_window.map(|window| window.pending_count()), Some(1));

    let due_roots = store.due_subscription_roots(&reader, now).await.unwrap();
    assert_eq!(due_roots, vec![root.message.message_id.clone()]);
    let (notice, _) = store
        .select_subscription_notice(&reader, &due_roots, now, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        notice
            .roots
            .first()
            .map(|root_notice| root_notice.root_id.clone()),
        Some(root.message.message_id.clone())
    );

    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
