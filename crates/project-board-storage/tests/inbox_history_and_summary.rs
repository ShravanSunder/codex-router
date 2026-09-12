#![allow(clippy::unwrap_used)]
//! Inbox history guidance, watch visibility, and summary-membership behavior.
#[path = "board_behavior_support/mod.rs"]
mod board_behavior_support;
use board_behavior_support::*;
use project_board::*;
use project_board_storage::BoardStore;
use sqlx::{Connection, SqliteConnection};
use std::time::Instant;

async fn create_named_fixture(store: &mut BoardStore, suffix: usize) -> Fixture {
    let fixture = Fixture {
        project_id: ProjectId::generate(),
        board_id: BoardId::generate(),
        topic_id: TopicId::generate(),
    };
    store
        .create_project(ProjectCreateRequest {
            project_id: fixture.project_id.clone(),
            name: name(&format!("Project {suffix}")),
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
            name: name(&format!("Board {suffix}")),
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
            name: name(&format!("Topic {suffix}")),
            description: description(""),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    fixture
}

#[tokio::test]
async fn history_guidance_exists_only_for_queryable_scoped_messages() {
    let path = database_path("scoped-history-guidance");
    let mut store = BoardStore::open(&path).await.unwrap();
    let empty_project = create_named_fixture(&mut store, 1).await;
    let active_project = create_named_fixture(&mut store, 2).await;
    post(
        &mut store,
        Placement::Topic {
            topic_id: active_project.topic_id,
        },
        actor("other-author"),
        "other project",
        vec![],
    )
    .await;

    let empty_initialization = store
        .fetch_inbox(InboxFetchRequest {
            project_id: empty_project.project_id.clone(),
            reader: actor("empty-reader"),
            page: page(10),
        })
        .await
        .unwrap()
        .page
        .initialization;
    assert!(matches!(
        empty_initialization,
        InboxInitializationStatus::Initialized {
            earlier_history_range: None,
            ..
        }
    ));
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: empty_project.topic_id,
        },
        actor("root-author"),
        "root",
        vec![],
    )
    .await;
    let history_initialization = store
        .fetch_inbox(InboxFetchRequest {
            project_id: empty_project.project_id.clone(),
            reader: actor("history-reader"),
            page: page(10),
        })
        .await
        .unwrap()
        .page
        .initialization;
    let history_with_message = match history_initialization {
        InboxInitializationStatus::Initialized {
            earlier_history_range: Some(history),
            message,
            ..
        } => Some((history, message)),
        _ => None,
    };
    let (history, message) = history_with_message.unwrap();
    assert_eq!(
        history.scope,
        MessageListScope::Project {
            project_id: empty_project.project_id
        }
    );
    assert_eq!(
        history.selection,
        MessageSelection::Range {
            from_activity_sequence: ActivitySequence::ZERO,
            to_activity_sequence: root.message.activity_sequence
        }
    );
    assert!(message.contains("earlierHistoryRange"));
    assert!(message.contains("message list"));
    assert!(message.contains("range"));

    let empty_watch = store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: actor("empty-watcher"),
            acting_for: None,
        })
        .await
        .unwrap();
    assert!(empty_watch.watch_status.earlier_unwatched_range.is_none());
    assert!(
        !empty_watch
            .watch_status
            .message
            .contains("earlierUnwatchedRange")
    );
    let reply = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id.clone(),
        },
        actor("reply-author"),
        "reply",
        vec![],
    )
    .await;
    let history_watch = store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id,
            actor: actor("history-watcher"),
            acting_for: None,
        })
        .await
        .unwrap();
    assert_eq!(
        history_watch
            .watch_status
            .earlier_unwatched_range
            .unwrap()
            .selection,
        MessageSelection::Range {
            from_activity_sequence: ActivitySequence::ZERO,
            to_activity_sequence: reply.message.activity_sequence
        }
    );
    assert!(
        history_watch
            .watch_status
            .message
            .contains("earlierUnwatchedRange")
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn inactive_watch_hides_subscription_start_without_rewriting_history() {
    let path = database_path("inactive-watch-shape");
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
    let active = store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: watcher.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    let active_start = active.watch_status.starts_after_activity_sequence.unwrap();
    let reply = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id.clone(),
        },
        actor("reply-author"),
        "reply",
        vec![],
    )
    .await;
    let inactive = store
        .unwatch_thread(ThreadUnwatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: watcher.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    assert_eq!(inactive.watch_status.starts_after_activity_sequence, None);
    assert_eq!(
        inactive
            .watch_status
            .earlier_unwatched_range
            .clone()
            .unwrap()
            .selection,
        MessageSelection::Range {
            from_activity_sequence: ActivitySequence::ZERO,
            to_activity_sequence: reply.message.activity_sequence
        }
    );
    let mut inspection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let stored_start: i64 = sqlx::query_scalar(
        "SELECT watch.starts_after_activity FROM thread_watches watch \
         JOIN board_identities identity ON identity.identity_key=watch.reader_key \
         WHERE watch.root_id=? AND identity.kind='human' AND identity.human_id='watcher'",
    )
    .bind(root.message.message_id.as_str())
    .fetch_one(&mut inspection)
    .await
    .unwrap();
    assert_eq!(stored_start, i64::try_from(active_start.get()).unwrap());
    inspection.close().await.unwrap();
    let shown = store
        .show_thread(ThreadShowRequest {
            root_message_id: root.message.message_id,
            reader: Some(watcher),
        })
        .await
        .unwrap()
        .watch_status
        .unwrap();
    assert!(!shown.watching);
    assert_eq!(shown.starts_after_activity_sequence, None);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn never_tracked_acknowledge_and_unwatch_do_not_create_summary_rows() {
    let path = database_path("no-phantom-summary");
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
    let acknowledging_reader = actor("ack-reader");
    let acknowledgement = store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: acknowledging_reader.clone(),
            acting_for: None,
            scope: ReadScope::Topic {
                topic_id: root.message.topic_id.clone(),
            },
            through_activity_sequence: root.message.activity_sequence,
        })
        .await
        .unwrap();
    assert!(
        !acknowledgement
            .project_unread_summary
            .main_tracking_initialized
    );
    assert!(
        store
            .list_inbox_projects(InboxProjectsRequest {
                reader: acknowledging_reader.clone(),
                unread_only: false,
                page: page(10)
            })
            .await
            .unwrap()
            .page
            .records
            .is_empty()
    );

    let unwatching_reader = actor("unwatch-reader");
    let reply = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id.clone(),
        },
        actor("reply-author"),
        "reply",
        vec![],
    )
    .await;
    let never_watched = store
        .show_thread(ThreadShowRequest {
            root_message_id: root.message.message_id.clone(),
            reader: Some(unwatching_reader.clone()),
        })
        .await
        .unwrap()
        .watch_status
        .unwrap();
    assert_eq!(never_watched.starts_after_activity_sequence, None);
    assert_eq!(
        never_watched.earlier_unwatched_range.unwrap().selection,
        MessageSelection::Range {
            from_activity_sequence: ActivitySequence::ZERO,
            to_activity_sequence: reply.message.activity_sequence
        }
    );
    let unwatched = store
        .unwatch_thread(ThreadUnwatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: unwatching_reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    assert_eq!(unwatched.watch_status.starts_after_activity_sequence, None);
    assert!(unwatched.watch_status.earlier_unwatched_range.is_some());
    assert!(
        store
            .list_inbox_projects(InboxProjectsRequest {
                reader: unwatching_reader.clone(),
                unread_only: false,
                page: page(10)
            })
            .await
            .unwrap()
            .page
            .records
            .is_empty()
    );
    store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: unwatching_reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    assert_eq!(
        store
            .list_inbox_projects(InboxProjectsRequest {
                reader: unwatching_reader,
                unread_only: false,
                page: page(10)
            })
            .await
            .unwrap()
            .page
            .records
            .len(),
        1
    );
    store
        .fetch_inbox(InboxFetchRequest {
            project_id: fixture.project_id,
            reader: acknowledging_reader.clone(),
            page: page(10),
        })
        .await
        .unwrap();
    assert_eq!(
        store
            .list_inbox_projects(InboxProjectsRequest {
                reader: acknowledging_reader,
                unread_only: false,
                page: page(10)
            })
            .await
            .unwrap()
            .page
            .records
            .len(),
        1
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn missing_root_uses_inspect_resource_and_summary_reads_cover_many_projects() {
    let path = database_path("summary-population");
    let mut store = BoardStore::open(&path).await.unwrap();
    let missing = MessageId::generate();
    let failure = store
        .show_thread(ThreadShowRequest {
            root_message_id: missing.clone(),
            reader: None,
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRootMessage);
    assert_eq!(failure.next_action, BoardNextAction::InspectResource);
    assert!(
        matches!(failure.details, BoardErrorDetails::Resource { resource: ResourceIdentity::Thread { root_message_id } } if root_message_id == missing)
    );

    let reader = actor("summary-reader");
    for suffix in 10..18 {
        let fixture = create_named_fixture(&mut store, suffix).await;
        let root = post(
            &mut store,
            Placement::Topic {
                topic_id: fixture.topic_id,
            },
            actor(&format!("author-{suffix}")),
            "root",
            vec![],
        )
        .await;
        store
            .watch_thread(ThreadWatchRequest {
                root_message_id: root.message.message_id,
                actor: reader.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
    }
    let started = Instant::now();
    let summaries = store
        .list_inbox_projects(InboxProjectsRequest {
            reader,
            unread_only: false,
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records;
    let elapsed = started.elapsed();
    assert_eq!(summaries.len(), 8);
    eprintln!(
        "summary read across 8 watched projects: {}us",
        elapsed.as_micros()
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
