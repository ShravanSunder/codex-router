#![allow(clippy::unwrap_used)]

use project_board::*;
use project_board_storage::BoardStore;
use sqlx::{Connection, SqliteConnection};
mod board_behavior_support;
#[path = "board_behavior_support/concurrency_behaviors.rs"]
mod concurrency_behaviors;
use board_behavior_support::*;

#[tokio::test]
async fn metadata_and_repository_operations_preserve_identity_and_lifecycle() {
    let path = database_path("metadata");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let repository = RepositoryRef::Origin {
        normalized_origin: NormalizedOrigin::try_from("github.com/example/router".to_owned())
            .unwrap(),
    };

    let attached = store
        .attach_repository(RepositoryAttachRequest {
            project_id: fixture.project_id.clone(),
            repository: repository.clone(),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    assert!(attached.attached);
    assert_eq!(
        store
            .attach_repository(RepositoryAttachRequest {
                project_id: fixture.project_id.clone(),
                repository: repository.clone(),
                actor: actor("owner"),
                acting_for: None
            })
            .await
            .unwrap()
            .repository,
        repository
    );
    assert_eq!(
        store
            .list_projects(ProjectListRequest {
                repository: Some(repository.clone()),
                page: page(10)
            })
            .await
            .unwrap()
            .page
            .records
            .len(),
        1
    );
    assert_eq!(
        store
            .list_repositories(RepositoryListRequest {
                project_id: fixture.project_id.clone(),
                page: page(10)
            })
            .await
            .unwrap()
            .page
            .records,
        vec![repository.clone()]
    );

    let renamed = store
        .update_topic(TopicUpdateRequest {
            topic_id: fixture.topic_id.clone(),
            name: name("Delivery"),
            description: description("Renamed"),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap()
        .topic;
    assert_eq!(renamed.topic_id, fixture.topic_id);
    assert_eq!(renamed.name.as_str(), "Delivery");
    store
        .archive_board(BoardArchiveRequest {
            board_id: fixture.board_id.clone(),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    let failure = store
        .update_topic(TopicUpdateRequest {
            topic_id: fixture.topic_id,
            name: name("Blocked"),
            description: description(""),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::ArchivedBoard);
    assert!(
        !store
            .detach_repository(RepositoryDetachRequest {
                project_id: fixture.project_id.clone(),
                repository: repository.clone(),
                actor: actor("owner"),
                acting_for: None
            })
            .await
            .unwrap()
            .attached
    );
    assert!(
        store
            .list_repositories(RepositoryListRequest {
                project_id: fixture.project_id.clone(),
                page: page(10)
            })
            .await
            .unwrap()
            .page
            .records
            .is_empty()
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn immutable_messages_threads_references_and_signed_pagination_share_one_activity_order() {
    let path = database_path("messages");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let alice = actor("alice");
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        alice.clone(),
        "root",
        vec![],
    )
    .await;
    assert!(root.watch_status.watching);
    let reply = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id.clone(),
        },
        actor("bob"),
        "reply",
        vec![
            ReferenceTarget::Message {
                message_id: root.message.message_id.clone(),
            },
            ReferenceTarget::Thread {
                root_message_id: root.message.message_id.clone(),
            },
        ],
    )
    .await;
    assert_eq!(reply.message.references.len(), 2);

    let first = store
        .list_messages(MessageListRequest {
            scope: MessageListScope::Board {
                board_id: fixture.board_id.clone(),
            },
            selection: MessageSelection::Latest,
            page: page(1),
        })
        .await
        .unwrap()
        .page;
    assert_eq!(first.records.len(), 1);
    assert_eq!(first.records[0].message_id, reply.message.message_id);
    let cursor = first.next_cursor.clone().expect("second page");
    let second = store
        .list_messages(MessageListRequest {
            scope: MessageListScope::Board {
                board_id: fixture.board_id.clone(),
            },
            selection: MessageSelection::Latest,
            page: PageRequest {
                limit: PageLimit::try_from(1).unwrap(),
                cursor: Some(cursor.clone()),
            },
        })
        .await
        .unwrap()
        .page;
    assert_eq!(second.records[0].message_id, root.message.message_id);
    let mut tampered = cursor.into_bytes();
    tampered[0] = if tampered[0] == b'a' { b'b' } else { b'a' };
    let failure = store
        .list_messages(MessageListRequest {
            scope: MessageListScope::Board {
                board_id: fixture.board_id.clone(),
            },
            selection: MessageSelection::Latest,
            page: PageRequest {
                limit: PageLimit::try_from(1).unwrap(),
                cursor: Some(String::from_utf8(tampered).unwrap()),
            },
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidCursor);

    let other_path = database_path("cursor-other-database");
    let mut other_store = BoardStore::open(&other_path).await.unwrap();
    create_fixture_with_ids(&mut other_store, &fixture).await;
    let other_root = post(
        &mut other_store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("other-alice"),
        "other root",
        vec![],
    )
    .await;
    post(
        &mut other_store,
        Placement::Thread {
            root_message_id: other_root.message.message_id,
        },
        actor("other-bob"),
        "other reply",
        vec![],
    )
    .await;
    let cross_database_failure = other_store
        .list_messages(MessageListRequest {
            scope: MessageListScope::Board {
                board_id: fixture.board_id.clone(),
            },
            selection: MessageSelection::Latest,
            page: PageRequest {
                limit: PageLimit::try_from(1).unwrap(),
                cursor: first.next_cursor,
            },
        })
        .await
        .unwrap_err();
    assert_eq!(cross_database_failure.kind, BoardFailureKind::InvalidCursor);
    other_store.close().await.unwrap();
    std::fs::remove_file(other_path).unwrap();

    store
        .resolve_thread(ThreadResolveRequest {
            root_message_id: root.message.message_id.clone(),
            actor: alice.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    let failure = store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            actor: alice,
            acting_for: None,
            text: text("blocked"),
            references: no_references(),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::ThreadResolved);
    store
        .unresolve_thread(ThreadUnresolveRequest {
            root_message_id: root.message.message_id.clone(),
            actor: actor("alice"),
            acting_for: None,
        })
        .await
        .unwrap();
    post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id,
        },
        actor("alice"),
        "allowed",
        vec![],
    )
    .await;
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn future_only_watches_scoped_acknowledgements_and_unread_summaries_survive_reopen() {
    let path = database_path("inbox");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let alice = actor("alice");
    let bob = actor("bob");
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        alice.clone(),
        "historical root",
        vec![],
    )
    .await;

    let initial = store
        .fetch_inbox(InboxFetchRequest {
            project_id: fixture.project_id.clone(),
            reader: bob.clone(),
            page: page(10),
        })
        .await
        .unwrap()
        .page;
    assert!(initial.records.is_empty());
    assert!(matches!(
        initial.initialization,
        InboxInitializationStatus::Initialized { .. }
    ));
    let watched = store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: bob.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    assert!(watched.watch_status.earlier_unwatched_range.is_none());

    let reply = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message.message_id.clone(),
        },
        alice.clone(),
        "new reply",
        vec![],
    )
    .await;
    let resolved = store
        .resolve_thread(ThreadResolveRequest {
            root_message_id: root.message.message_id.clone(),
            actor: alice,
            acting_for: None,
        })
        .await
        .unwrap()
        .activity_sequence
        .unwrap();
    let inbox = store
        .fetch_inbox(InboxFetchRequest {
            project_id: fixture.project_id.clone(),
            reader: bob.clone(),
            page: page(1),
        })
        .await
        .unwrap()
        .page;
    assert_eq!(inbox.records.len(), 1);
    let continuation = inbox
        .next_cursor
        .expect("resolved activity remains in snapshot");
    let first_ack = store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: bob.clone(),
            acting_for: None,
            scope: ReadScope::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            through_activity_sequence: reply.message.activity_sequence,
        })
        .await
        .unwrap();
    assert!(first_ack.project_unread_summary.has_unread);
    let continued = store
        .fetch_inbox(InboxFetchRequest {
            project_id: fixture.project_id.clone(),
            reader: bob.clone(),
            page: PageRequest {
                limit: PageLimit::try_from(1).unwrap(),
                cursor: Some(continuation),
            },
        })
        .await
        .unwrap()
        .page;
    assert_eq!(continued.records.len(), 1);
    assert!(matches!(
        continued.records[0],
        InboxActivity::ThreadStateChanged {
            state: ThreadState::Resolved,
            ..
        }
    ));
    assert!(
        store
            .list_inbox_projects(InboxProjectsRequest {
                reader: bob.clone(),
                unread_only: true,
                page: page(10)
            })
            .await
            .unwrap()
            .page
            .records[0]
            .has_unread
    );

    let final_ack = store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: bob.clone(),
            acting_for: None,
            scope: ReadScope::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            through_activity_sequence: resolved,
        })
        .await
        .unwrap();
    assert!(!final_ack.project_unread_summary.has_unread);
    store
        .unresolve_thread(ThreadUnresolveRequest {
            root_message_id: root.message.message_id,
            actor: actor("alice"),
            acting_for: None,
        })
        .await
        .unwrap();
    store.close().await.unwrap();

    let mut store = BoardStore::open(&path).await.unwrap();
    let summaries = store
        .list_inbox_projects(InboxProjectsRequest {
            reader: bob,
            unread_only: true,
            page: page(10),
        })
        .await
        .unwrap()
        .page
        .records;
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].has_unread);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn malformed_stored_primary_id_is_reported_without_echoing_the_raw_value() {
    let path = database_path("invalid-primary-id");
    BoardStore::open(&path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO board_projects(project_id,name,description) VALUES('malformed','Corrupt','')",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    let mut store = BoardStore::open(&path).await.unwrap();
    let failure = store
        .list_projects(ProjectListRequest {
            repository: None,
            page: page(10),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    assert!(!failure.message.contains("malformed"));
    assert!(
        matches!(failure.details, BoardErrorDetails::FieldConstraint { ref field, .. } if field == "projectId")
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn escaped_large_messages_page_without_truncation_or_skipping() {
    let path = database_path("large-pages");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("alice"),
        "root",
        vec![],
    )
    .await;
    let bob = actor("bob");
    store
        .fetch_inbox(InboxFetchRequest {
            project_id: fixture.project_id.clone(),
            reader: bob.clone(),
            page: page(100),
        })
        .await
        .unwrap();
    store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: bob.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    let escaped_body = "\\\"".repeat(32_768);
    for _ in 0..9 {
        post(
            &mut store,
            Placement::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            actor("alice"),
            &escaped_body,
            vec![],
        )
        .await;
    }

    let mut message_cursor = None;
    let mut message_count = 0;
    loop {
        let result = store
            .list_messages(MessageListRequest {
                scope: MessageListScope::Board {
                    board_id: fixture.board_id.clone(),
                },
                selection: MessageSelection::Latest,
                page: PageRequest {
                    limit: PageLimit::try_from(100).unwrap(),
                    cursor: message_cursor,
                },
            })
            .await
            .unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() < 1_048_576);
        message_count += result.page.records.len();
        message_cursor = result.page.next_cursor;
        if message_cursor.is_none() {
            break;
        }
    }
    assert_eq!(message_count, 10);

    let mut inbox_cursor = None;
    let mut inbox_count = 0;
    loop {
        let result = store
            .fetch_inbox(InboxFetchRequest {
                project_id: fixture.project_id.clone(),
                reader: bob.clone(),
                page: PageRequest {
                    limit: PageLimit::try_from(100).unwrap(),
                    cursor: inbox_cursor,
                },
            })
            .await
            .unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() < 1_048_576);
        inbox_count += result.page.records.len();
        inbox_cursor = result.page.next_cursor;
        if inbox_cursor.is_none() {
            break;
        }
    }
    assert_eq!(inbox_count, 9);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
