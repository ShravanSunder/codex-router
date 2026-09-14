#![allow(clippy::unwrap_used)]
#[path = "board_behavior_support/mod.rs"]
mod board_behavior_support;
use board_behavior_support::*;
use message_board::*;
use message_board_storage::BoardStore;

async fn location(store: &mut BoardStore, label: &str) -> Fixture {
    let fixture = Fixture {
        project_id: ProjectId::generate(),
        board_id: BoardId::generate(),
        topic_id: TopicId::generate(),
    };
    store
        .create_project(ProjectCreateRequest {
            project_id: fixture.project_id.clone(),
            name: name(label),
            description: description(""),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    store
        .create_board(BoardCreateRequest {
            board_id: fixture.board_id.clone(),
            project_id: fixture.project_id.clone(),
            name: name(label),
            description: description(""),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    store
        .create_topic(TopicCreateRequest {
            topic_id: fixture.topic_id.clone(),
            board_id: fixture.board_id.clone(),
            name: name(label),
            description: description(""),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    fixture
}

fn fetch(scope: InboxScope, mode: InboxReadMode, limit: u32) -> InboxFetchRequest {
    InboxFetchRequest {
        scope,
        reader: actor("reader"),
        read_mode: mode,
        page: page(limit),
    }
}

fn message_ids(records: &[InboxActivity]) -> Vec<MessageId> {
    records
        .iter()
        .filter_map(|record| match record {
            InboxActivity::MessageCreated { message, .. } => Some(message.message_id.clone()),
            InboxActivity::ThreadStateChanged { .. } => None,
        })
        .collect()
}

#[tokio::test]
async fn topic_filter_keeps_global_watches_without_subscribing_to_topic_threads() {
    let mut store = BoardStore::open(&database_path("filtered-inbox"))
        .await
        .unwrap();
    let selected = location(&mut store, "selected").await;
    let outside = location(&mut store, "outside").await;
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: selected.topic_id.clone(),
        },
        actor("root-author"),
        "old root",
        vec![],
    )
    .await
    .message;
    let watched_root = post(
        &mut store,
        Placement::Topic {
            topic_id: outside.topic_id.clone(),
        },
        actor("outside-author"),
        "watched root",
        vec![],
    )
    .await
    .message;
    store
        .watch_thread(ThreadWatchRequest {
            root_message_id: watched_root.message_id.clone(),
            actor: actor("reader"),
            acting_for: None,
        })
        .await
        .unwrap();
    let watched_message = post(
        &mut store,
        Placement::Thread {
            root_message_id: watched_root.message_id.clone(),
        },
        actor("thread-author"),
        "watched activity",
        vec![],
    )
    .await
    .message;
    let scope = InboxScope::Topic {
        topic_id: selected.topic_id.clone(),
    };
    let initial = store
        .fetch_inbox(fetch(scope.clone(), InboxReadMode::Unread, 10))
        .await
        .unwrap()
        .page;
    assert!(matches!(
        initial.initialization,
        InboxInitializationStatus::Initialized { .. }
    ));
    assert_eq!(
        message_ids(&initial.records),
        vec![watched_message.message_id.clone()]
    );

    let another_topic = TopicId::generate();
    store
        .create_topic(TopicCreateRequest {
            topic_id: another_topic.clone(),
            board_id: selected.board_id.clone(),
            name: name("unrelated"),
            description: description(""),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    let main = post(
        &mut store,
        Placement::Topic {
            topic_id: selected.topic_id.clone(),
        },
        actor("main-author"),
        "selected top-level",
        vec![ReferenceTarget::Thread {
            root_message_id: watched_root.message_id.clone(),
        }],
    )
    .await
    .message;
    let unrelated_main = post(
        &mut store,
        Placement::Topic {
            topic_id: another_topic,
        },
        actor("unrelated-author"),
        "not selected",
        vec![],
    )
    .await
    .message;
    post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message_id.clone(),
        },
        actor("unwatched-author"),
        "unwatched thread in selected topic",
        vec![],
    )
    .await;
    store
        .resolve_thread(ThreadResolveRequest {
            root_message_id: watched_root.message_id.clone(),
            actor: actor("resolver"),
            acting_for: None,
        })
        .await
        .unwrap();

    let mut request = fetch(scope.clone(), InboxReadMode::Unread, 1);
    let mut records = Vec::new();
    loop {
        let response = store.fetch_inbox(request.clone()).await.unwrap().page;
        assert!(response.records.len() <= 1);
        assert_eq!(response.scope, scope);
        records.extend(response.records);
        let Some(cursor) = response.next_cursor else {
            break;
        };
        request.page.cursor = Some(cursor);
    }
    assert_eq!(
        message_ids(&records),
        vec![watched_message.message_id, main.message_id.clone()]
    );
    assert_eq!(records.len(), 3);
    assert!(
        matches!(&records[2], InboxActivity::ThreadStateChanged { project_id, board_id, topic_id, root_message_id, .. }
        if *project_id == outside.project_id && *board_id == outside.board_id && *topic_id == outside.topic_id && *root_message_id == watched_root.message_id)
    );
    let watch = store
        .show_thread(ThreadShowRequest {
            root_message_id: root.message_id.clone(),
            reader: Some(actor("reader")),
        })
        .await
        .unwrap();
    assert!(!watch.watch_status.unwrap().watching);

    let latest = store
        .fetch_inbox(fetch(
            InboxScope::Board {
                board_id: selected.board_id,
            },
            InboxReadMode::Latest,
            100,
        ))
        .await
        .unwrap()
        .page;
    assert_eq!(latest.ordering, MessageOrdering::NewestFirst);
    assert!(matches!(
        latest.initialization,
        InboxInitializationStatus::NotApplicable
    ));
    let latest_ids = message_ids(&latest.records);
    assert!(latest_ids.contains(&unrelated_main.message_id));
    assert!(latest_ids.contains(&root.message_id));
    assert!(!latest_ids.contains(&watched_root.message_id));
    assert_eq!(latest.records.len(), 4);
}

#[tokio::test]
async fn latest_is_nonmutating_and_continues_normally_when_watches_change() {
    let path = database_path("latest-watches");
    let mut store = BoardStore::open(&path).await.unwrap();
    let selected = location(&mut store, "selected").await;
    let outside = location(&mut store, "outside").await;
    let outside_root = post(
        &mut store,
        Placement::Topic {
            topic_id: outside.topic_id,
        },
        actor("outside-author"),
        "root",
        vec![],
    )
    .await
    .message;
    let older = post(
        &mut store,
        Placement::Topic {
            topic_id: selected.topic_id.clone(),
        },
        actor("older-author"),
        "older",
        vec![],
    )
    .await
    .message;
    let newly_watched = post(
        &mut store,
        Placement::Thread {
            root_message_id: outside_root.message_id.clone(),
        },
        actor("thread-author"),
        "between pages",
        vec![],
    )
    .await
    .message;
    let newer = post(
        &mut store,
        Placement::Topic {
            topic_id: selected.topic_id.clone(),
        },
        actor("newer-author"),
        "newer",
        vec![],
    )
    .await
    .message;
    let passed_position = post(
        &mut store,
        Placement::Thread {
            root_message_id: outside_root.message_id.clone(),
        },
        actor("thread-author"),
        "newly eligible above the cursor",
        vec![],
    )
    .await
    .message;
    let scope = InboxScope::Project {
        project_id: selected.project_id,
    };
    let mut request = fetch(scope.clone(), InboxReadMode::Latest, 1);
    let first = store.fetch_inbox(request.clone()).await.unwrap().page;
    assert_eq!(message_ids(&first.records), vec![newer.message_id]);
    let summaries = store
        .list_inbox_projects(InboxProjectsRequest {
            reader: actor("reader"),
            unread_only: false,
            page: page(100),
        })
        .await
        .unwrap();
    assert!(summaries.page.records.is_empty());
    let later_arrival = post(
        &mut store,
        Placement::Topic {
            topic_id: selected.topic_id,
        },
        actor("later-author"),
        "after the captured upper bound",
        vec![],
    )
    .await
    .message;
    request.page.cursor = first.next_cursor;
    store
        .watch_thread(ThreadWatchRequest {
            root_message_id: outside_root.message_id,
            actor: actor("reader"),
            acting_for: None,
        })
        .await
        .unwrap();
    let second = store.fetch_inbox(request.clone()).await.unwrap().page;
    assert_eq!(message_ids(&second.records), vec![newly_watched.message_id]);
    request.page.cursor = second.next_cursor;
    let third = store.fetch_inbox(request).await.unwrap().page;
    assert_eq!(message_ids(&third.records), vec![older.message_id]);
    let fresh = store
        .fetch_inbox(fetch(scope.clone(), InboxReadMode::Latest, 100))
        .await
        .unwrap()
        .page;
    let fresh_ids = message_ids(&fresh.records);
    assert_eq!(fresh_ids[0], later_arrival.message_id);
    assert!(fresh_ids.contains(&passed_position.message_id));
    store.close().await.unwrap();
    let mut reopened = BoardStore::open(&path).await.unwrap();
    let unread = reopened
        .fetch_inbox(fetch(scope, InboxReadMode::Unread, 100))
        .await
        .unwrap()
        .page;
    assert!(matches!(
        unread.initialization,
        InboxInitializationStatus::Initialized { .. }
    ));
    assert!(unread.records.is_empty());
}
