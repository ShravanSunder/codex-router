#![allow(clippy::unwrap_used)]
use super::board_behavior_support::*;
use project_board::*;
use project_board_storage::BoardStore;
use std::sync::Arc;
use tokio::sync::{Barrier, Mutex, Notify};

fn post_request(
    message_id: MessageId,
    placement: Placement,
    identity: Identity,
) -> MessagePostRequest {
    MessagePostRequest {
        message_id,
        placement,
        actor: identity,
        acting_for: None,
        text: text("concurrent message"),
        references: no_references(),
    }
}

fn take_owned_store(store: Arc<Mutex<BoardStore>>) -> BoardStore {
    Arc::into_inner(store).unwrap().into_inner()
}

#[tokio::test]
async fn competing_top_level_posts_for_one_actor_commit_only_once() {
    let path = database_path("concurrent-cooldown");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let store = Arc::new(Mutex::new(store));
    let start = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for message_id in [MessageId::generate(), MessageId::generate()] {
        let task_store = Arc::clone(&store);
        let task_start = Arc::clone(&start);
        let topic_id = fixture.topic_id.clone();
        tasks.push(tokio::spawn(async move {
            task_start.wait().await;
            task_store
                .lock()
                .await
                .post_message(post_request(
                    message_id,
                    Placement::Topic { topic_id },
                    actor("same-actor"),
                ))
                .await
        }));
    }
    start.wait().await;
    let first = tasks.remove(0).await.unwrap();
    let second = tasks.remove(0).await.unwrap();
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let failure = first.err().or_else(|| second.err()).unwrap();
    assert_eq!(failure.kind, BoardFailureKind::TopLevelMessageCooldown);
    let mut store = take_owned_store(store);
    let messages = store
        .list_messages(MessageListRequest {
            scope: MessageListScope::Topic {
                topic_id: fixture.topic_id,
            },
            selection: MessageSelection::Latest,
            page: page(10),
        })
        .await
        .unwrap();
    assert_eq!(messages.page.records.len(), 1);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn archive_and_post_race_observes_both_valid_serial_orders() {
    run_archive_post_order(true).await;
    run_archive_post_order(false).await;
}

async fn run_archive_post_order(archive_first: bool) {
    let path = database_path(if archive_first {
        "archive-first"
    } else {
        "post-first"
    });
    let mut initial_store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut initial_store).await;
    let message_id = MessageId::generate();
    let store = Arc::new(Mutex::new(initial_store));
    let first_locked = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());

    let first_store = Arc::clone(&store);
    let first_locked_signal = Arc::clone(&first_locked);
    let first_release = Arc::clone(&release_first);
    let board_id = fixture.board_id.clone();
    let topic_id = fixture.topic_id.clone();
    let first_message_id = message_id.clone();
    let first = tokio::spawn(async move {
        let mut guard = first_store.lock().await;
        first_locked_signal.notify_one();
        first_release.notified().await;
        if archive_first {
            guard
                .archive_board(BoardArchiveRequest {
                    board_id,
                    actor: actor("archiver"),
                    acting_for: None,
                })
                .await
                .map(|_| true)
        } else {
            guard
                .post_message(post_request(
                    first_message_id,
                    Placement::Topic { topic_id },
                    actor("poster"),
                ))
                .await
                .map(|_| true)
        }
    });
    first_locked.notified().await;

    let second_store = Arc::clone(&store);
    let second_board_id = fixture.board_id.clone();
    let second_topic_id = fixture.topic_id.clone();
    let second_message_id = message_id.clone();
    let second = tokio::spawn(async move {
        let mut guard = second_store.lock().await;
        if archive_first {
            guard
                .post_message(post_request(
                    second_message_id,
                    Placement::Topic {
                        topic_id: second_topic_id,
                    },
                    actor("poster"),
                ))
                .await
                .map(|_| true)
        } else {
            guard
                .archive_board(BoardArchiveRequest {
                    board_id: second_board_id,
                    actor: actor("archiver"),
                    acting_for: None,
                })
                .await
                .map(|_| true)
        }
    });
    release_first.notify_one();
    assert!(first.await.unwrap().is_ok());
    let second_result = second.await.unwrap();
    if archive_first {
        assert_eq!(
            second_result.unwrap_err().kind,
            BoardFailureKind::ArchivedBoard
        );
    } else {
        assert!(second_result.is_ok());
    }
    let mut store = take_owned_store(store);
    let shown = store.show_message(MessageShowRequest { message_id }).await;
    assert_eq!(shown.is_ok(), !archive_first);
    assert_eq!(
        store
            .show_board(BoardShowRequest {
                board_id: fixture.board_id
            })
            .await
            .unwrap()
            .board
            .state,
        BoardState::Archived
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn resolve_and_thread_post_race_observes_both_valid_serial_orders() {
    run_resolve_post_order(true).await;
    run_resolve_post_order(false).await;
}

async fn run_resolve_post_order(resolve_first: bool) {
    let path = database_path(if resolve_first {
        "resolve-first"
    } else {
        "thread-post-first"
    });
    let mut initial_store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut initial_store).await;
    let root = post(
        &mut initial_store,
        Placement::Topic {
            topic_id: fixture.topic_id,
        },
        actor("root-author"),
        "root",
        vec![],
    )
    .await;
    let reply_id = MessageId::generate();
    let root_id = root.message.message_id;
    let store = Arc::new(Mutex::new(initial_store));
    let first_locked = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());

    let first_store = Arc::clone(&store);
    let first_locked_signal = Arc::clone(&first_locked);
    let first_release = Arc::clone(&release_first);
    let first_root_id = root_id.clone();
    let first_reply_id = reply_id.clone();
    let first = tokio::spawn(async move {
        let mut guard = first_store.lock().await;
        first_locked_signal.notify_one();
        first_release.notified().await;
        if resolve_first {
            guard
                .resolve_thread(ThreadResolveRequest {
                    root_message_id: first_root_id,
                    actor: actor("resolver"),
                    acting_for: None,
                })
                .await
                .map(|_| true)
        } else {
            guard
                .post_message(post_request(
                    first_reply_id,
                    Placement::Thread {
                        root_message_id: first_root_id,
                    },
                    actor("replier"),
                ))
                .await
                .map(|_| true)
        }
    });
    first_locked.notified().await;
    let second_store = Arc::clone(&store);
    let second_root_id = root_id.clone();
    let second_reply_id = reply_id.clone();
    let second = tokio::spawn(async move {
        let mut guard = second_store.lock().await;
        if resolve_first {
            guard
                .post_message(post_request(
                    second_reply_id,
                    Placement::Thread {
                        root_message_id: second_root_id,
                    },
                    actor("replier"),
                ))
                .await
                .map(|_| true)
        } else {
            guard
                .resolve_thread(ThreadResolveRequest {
                    root_message_id: second_root_id,
                    actor: actor("resolver"),
                    acting_for: None,
                })
                .await
                .map(|_| true)
        }
    });
    release_first.notify_one();
    assert!(first.await.unwrap().is_ok());
    let second_result = second.await.unwrap();
    if resolve_first {
        assert_eq!(
            second_result.unwrap_err().kind,
            BoardFailureKind::ThreadResolved
        );
    } else {
        assert!(second_result.is_ok());
    }
    let mut store = take_owned_store(store);
    assert_eq!(
        store
            .show_message(MessageShowRequest {
                message_id: reply_id
            })
            .await
            .is_ok(),
        !resolve_first
    );
    assert_eq!(
        store
            .show_thread(ThreadShowRequest {
                root_message_id: root_id,
                reader: None
            })
            .await
            .unwrap()
            .thread
            .state,
        ThreadState::Resolved
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn rewatch_boundaries_and_acknowledgements_keep_summary_equal_to_inbox() {
    let path = database_path("rewatch-summary");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let alice = actor("alice");
    let bob = actor("bob");
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id,
        },
        alice.clone(),
        "root",
        vec![],
    )
    .await;
    store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: bob.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id.clone(),
        },
        alice.clone(),
        "first",
        vec![],
    )
    .await;
    store
        .unwatch_thread(ThreadUnwatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: bob.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    assert_summary_matches_inbox(&mut store, &fixture.project_id, &bob, false).await;
    store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: bob.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    assert_summary_matches_inbox(&mut store, &fixture.project_id, &bob, false).await;
    let second = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id.clone(),
        },
        alice,
        "second",
        vec![],
    )
    .await;
    store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: bob.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    assert_summary_matches_inbox(&mut store, &fixture.project_id, &bob, true).await;
    store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: bob.clone(),
            acting_for: None,
            scope: ReadScope::Thread {
                root_message_id: root.message.message_id,
            },
            through_activity_sequence: second.message.activity_sequence,
        })
        .await
        .unwrap();
    assert_summary_matches_inbox(&mut store, &fixture.project_id, &bob, false).await;
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

async fn assert_summary_matches_inbox(
    store: &mut BoardStore,
    project_id: &ProjectId,
    reader: &Identity,
    expected_unread: bool,
) {
    let inbox_has_records = !store
        .fetch_inbox(InboxFetchRequest {
            project_id: project_id.clone(),
            reader: reader.clone(),
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records
        .is_empty();
    let summary = store
        .list_inbox_projects(InboxProjectsRequest {
            reader: reader.clone(),
            unread_only: false,
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records
        .into_iter()
        .find(|summary| summary.project_id == *project_id)
        .unwrap();
    assert_eq!(inbox_has_records, expected_unread);
    assert_eq!(summary.has_unread, inbox_has_records);
}
