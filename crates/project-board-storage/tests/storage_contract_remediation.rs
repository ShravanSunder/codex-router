#![allow(clippy::unwrap_used)]
#[path = "board_behavior_support/mod.rs"]
mod board_behavior_support;
use board_behavior_support::*;
use project_board::*;
use project_board_storage::BoardStore;

#[tokio::test]
async fn thread_roots_and_duplicate_topic_names_use_specific_failure_codes() {
    let path = database_path("specific-failures");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let missing_root = MessageId::generate();
    let failure = store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: missing_root.clone(),
            },
            actor: actor("author"),
            acting_for: None,
            text: text("reply"),
            references: no_references(),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRootMessage);
    assert!(
        matches!(failure.details, BoardErrorDetails::Resource { resource: ResourceIdentity::Thread { root_message_id } } if root_message_id == missing_root)
    );

    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("root-author"),
        "root",
        vec![],
    )
    .await;
    let child = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id,
        },
        actor("reply-author"),
        "child",
        vec![],
    )
    .await;
    let failure = store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: child.message.message_id,
            },
            actor: actor("nested-author"),
            acting_for: None,
            text: text("nested"),
            references: no_references(),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRootMessage);

    let failure = store
        .create_topic(TopicCreateRequest {
            topic_id: TopicId::generate(),
            board_id: fixture.board_id,
            name: name("Implementation"),
            description: description("duplicate"),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidTopicName);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn watch_messages_explain_history_guidance_and_board_lifecycle() {
    let path = database_path("watch-guidance");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id,
        },
        actor("author"),
        "root",
        vec![],
    )
    .await;
    let watcher = actor("watcher");
    let unwatched = store
        .show_thread(ThreadShowRequest {
            root_message_id: root.message.message_id.clone(),
            reader: Some(watcher.clone()),
        })
        .await
        .unwrap();
    let unwatched_message = unwatched.watch_status.unwrap().message;
    assert!(unwatched_message.contains("not watched"));
    assert!(unwatched_message.contains("watch"));
    assert!(unwatched_message.contains("active"));

    let watched = store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: watcher.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    assert!(watched.watch_status.earlier_unwatched_range.is_some());
    assert!(
        watched
            .watch_status
            .message
            .contains("earlierUnwatchedRange")
    );
    assert!(watched.watch_status.message.contains("message list"));
    assert!(watched.watch_status.message.contains("range"));
    assert!(watched.watch_status.message.contains("active"));

    store
        .archive_board(BoardArchiveRequest {
            board_id: fixture.board_id,
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    let archived = store
        .unwatch_thread(ThreadUnwatchRequest {
            root_message_id: root.message.message_id,
            actor: watcher,
            acting_for: None,
        })
        .await
        .unwrap();
    assert!(archived.watch_status.message.contains("read-only"));
    assert!(archived.outcome.contains("read-only"));
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn unresolve_outcomes_explain_when_thread_messages_are_available() {
    let path = database_path("unresolve-outcome");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id,
        },
        actor("author"),
        "root",
        vec![],
    )
    .await;
    store
        .resolve_thread(ThreadResolveRequest {
            root_message_id: root.message.message_id.clone(),
            actor: actor("resolver"),
            acting_for: None,
        })
        .await
        .unwrap();
    let changed = store
        .unresolve_thread(ThreadUnresolveRequest {
            root_message_id: root.message.message_id.clone(),
            actor: actor("resolver"),
            acting_for: None,
        })
        .await
        .unwrap();
    assert!(
        changed
            .outcome
            .contains("thread messages can be added while the board is active")
    );
    let repeated = store
        .unresolve_thread(ThreadUnresolveRequest {
            root_message_id: root.message.message_id,
            actor: actor("resolver"),
            acting_for: None,
        })
        .await
        .unwrap();
    assert!(
        repeated
            .outcome
            .contains("thread messages can be added while the board is active")
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn board_update_preserves_history_watch_bookmark_and_thread_list_filter() {
    let path = database_path("board-update-thread-list");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let reader = actor("reader");
    let first_root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("first-author"),
        "first root",
        vec![],
    )
    .await;
    let second_root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id,
        },
        actor("second-author"),
        "second root",
        vec![],
    )
    .await;
    store
        .watch_thread(ThreadWatchRequest {
            root_message_id: first_root.message.message_id.clone(),
            actor: reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    let reply = post(
        &mut store,
        Placement::Thread {
            root_message_id: first_root.message.message_id.clone(),
        },
        actor("reply-author"),
        "reply",
        vec![],
    )
    .await;
    store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: reader.clone(),
            acting_for: None,
            scope: ReadScope::Thread {
                root_message_id: first_root.message.message_id.clone(),
            },
            through_activity_sequence: reply.message.activity_sequence,
        })
        .await
        .unwrap();

    let updated = store
        .update_board(BoardUpdateRequest {
            board_id: fixture.board_id.clone(),
            name: name("Renamed Engineering"),
            description: description("Updated description"),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap()
        .board;
    assert_eq!(updated.board_id, fixture.board_id);
    assert_eq!(
        store
            .show_message(MessageShowRequest {
                message_id: reply.message.message_id
            })
            .await
            .unwrap()
            .message
            .placement,
        Placement::Thread {
            root_message_id: first_root.message.message_id.clone()
        }
    );
    assert!(
        store
            .show_thread(ThreadShowRequest {
                root_message_id: first_root.message.message_id.clone(),
                reader: Some(reader.clone())
            })
            .await
            .unwrap()
            .watch_status
            .unwrap()
            .watching
    );
    assert!(
        store
            .fetch_inbox(InboxFetchRequest {
                project_id: fixture.project_id.clone(),
                reader: reader.clone(),
                page: page(10)
            })
            .await
            .unwrap()
            .page
            .records
            .is_empty()
    );

    let watched = store
        .list_threads(ThreadListRequest {
            project_id: fixture.project_id.clone(),
            reader: reader.clone(),
            watched_only: true,
            page: page(10),
        })
        .await
        .unwrap()
        .page
        .records;
    assert_eq!(watched.len(), 1);
    assert_eq!(watched[0].root_message_id, first_root.message.message_id);
    let all = store
        .list_threads(ThreadListRequest {
            project_id: fixture.project_id,
            reader,
            watched_only: false,
            page: page(10),
        })
        .await
        .unwrap()
        .page
        .records;
    assert_eq!(all.len(), 2);
    assert!(
        all.iter()
            .any(|thread| thread.root_message_id == second_root.message.message_id)
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
