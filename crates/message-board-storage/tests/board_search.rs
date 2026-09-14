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
            description: description("project description"),
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
            description: description("board description"),
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
            description: description("topic description"),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    fixture
}

fn query(value: &str) -> SearchQuery {
    value.to_owned().try_into().unwrap()
}

fn messages(scope: MessageListScope, text: &str) -> MessageSearchRequest {
    MessageSearchRequest {
        query: query(text),
        scope,
        kind: SearchMessageKind::Both,
        include_archived: false,
        page: page(100),
    }
}

#[tokio::test]
async fn searches_keep_text_location_kind_and_reader_state_independent() {
    let mut store = BoardStore::open(&database_path("search-literal"))
        .await
        .unwrap();
    let selected = location(&mut store, "Navigation").await;
    let outside = location(&mut store, "Other").await;
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: selected.topic_id.clone(),
        },
        actor("root-author"),
        "root without keyword",
        vec![],
    )
    .await
    .message;
    let thread = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message_id.clone(),
        },
        actor("thread-author"),
        "Navigation 100%_ É",
        vec![],
    )
    .await
    .message;
    let main = post(
        &mut store,
        Placement::Topic {
            topic_id: selected.topic_id.clone(),
        },
        actor("main-author"),
        "NAVIGATION plain é",
        vec![],
    )
    .await
    .message;
    post(
        &mut store,
        Placement::Topic {
            topic_id: outside.topic_id,
        },
        actor("outside-author"),
        "Navigation outside",
        vec![],
    )
    .await;
    post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message_id.clone(),
        },
        actor("reference-author"),
        "reference only",
        vec![ReferenceTarget::Message {
            message_id: main.message_id.clone(),
        }],
    )
    .await;
    let scope = MessageListScope::Topic {
        topic_id: selected.topic_id.clone(),
    };
    let response = store
        .search_messages(messages(scope.clone(), " navigation "))
        .await
        .unwrap();
    assert_eq!(
        response
            .page
            .records
            .iter()
            .map(|hit| &hit.message.message_id)
            .collect::<Vec<_>>(),
        vec![&main.message_id, &thread.message_id]
    );
    assert!(
        response
            .page
            .records
            .iter()
            .all(|hit| hit.project_id == selected.project_id)
    );
    let mut request = messages(scope.clone(), "navigation");
    request.kind = SearchMessageKind::TopLevel;
    assert_eq!(
        store
            .search_messages(request.clone())
            .await
            .unwrap()
            .page
            .records[0]
            .message
            .message_id,
        main.message_id
    );
    request.kind = SearchMessageKind::Thread;
    assert_eq!(
        store.search_messages(request).await.unwrap().page.records[0]
            .message
            .message_id,
        thread.message_id
    );
    for literal in ["%_", "É"] {
        let records = store
            .search_messages(messages(scope.clone(), literal))
            .await
            .unwrap()
            .page
            .records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].message.message_id, thread.message_id);
    }
    let lowercase_unicode = store
        .search_messages(messages(scope.clone(), "é"))
        .await
        .unwrap()
        .page
        .records;
    assert_eq!(lowercase_unicode.len(), 1);
    assert_eq!(lowercase_unicode[0].message.message_id, main.message_id);
    let thread_scope = MessageListScope::Thread {
        root_message_id: root.message_id.clone(),
    };
    assert_eq!(
        store
            .search_messages(messages(thread_scope.clone(), "navigation"))
            .await
            .unwrap()
            .page
            .records
            .len(),
        1
    );
    let mut invalid = messages(thread_scope, "navigation");
    invalid.kind = SearchMessageKind::TopLevel;
    assert_eq!(
        store.search_messages(invalid).await.unwrap_err().kind,
        BoardFailureKind::InvalidField
    );

    let mut discovery = DiscoverySearchRequest {
        query: query("NAV"),
        scope: DiscoveryScope::AllProjects,
        kind: DiscoveryKind::All,
        include_archived: false,
        page: page(1),
    };
    let first = store
        .search_discovery(discovery.clone())
        .await
        .unwrap()
        .page;
    assert!(
        matches!(&first.records[0], DiscoveryHit::Project { project } if project.project_id == selected.project_id)
    );
    discovery.page.cursor = first.next_cursor;
    let second = store
        .search_discovery(discovery.clone())
        .await
        .unwrap()
        .page;
    assert!(
        matches!(&second.records[0], DiscoveryHit::Board { project, board } if project.project_id == selected.project_id && board.board_id == selected.board_id)
    );
    discovery.page.cursor = second.next_cursor;
    let third = store
        .search_discovery(discovery.clone())
        .await
        .unwrap()
        .page;
    assert!(
        matches!(&third.records[0], DiscoveryHit::Topic { project, board, topic } if project.project_id == selected.project_id && board.board_id == selected.board_id && topic.topic_id == selected.topic_id)
    );
    assert!(third.next_cursor.is_none());
    discovery.query = query("other");
    assert_eq!(
        store.search_discovery(discovery).await.unwrap_err().kind,
        BoardFailureKind::InvalidCursor
    );
    assert!(
        store
            .list_inbox_projects(InboxProjectsRequest {
                reader: actor("reader"),
                unread_only: false,
                page: page(100)
            })
            .await
            .unwrap()
            .page
            .records
            .is_empty()
    );
    assert!(
        !store
            .show_thread(ThreadShowRequest {
                root_message_id: root.message_id,
                reader: Some(actor("reader"))
            })
            .await
            .unwrap()
            .watch_status
            .unwrap()
            .watching
    );
}

#[tokio::test]
async fn archive_removes_remaining_search_hits_without_invalidating_the_cursor() {
    let mut store = BoardStore::open(&database_path("search-archive"))
        .await
        .unwrap();
    let archived = location(&mut store, "archived").await;
    let active = location(&mut store, "active").await;
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: archived.topic_id,
        },
        actor("root-author"),
        "root",
        vec![],
    )
    .await
    .message;
    let older = post(
        &mut store,
        Placement::Thread {
            root_message_id: root.message_id.clone(),
        },
        actor("thread-author"),
        "needle older",
        vec![],
    )
    .await
    .message;
    let newer = post(
        &mut store,
        Placement::Topic {
            topic_id: active.topic_id,
        },
        actor("new-author"),
        "needle newer",
        vec![],
    )
    .await
    .message;
    let mut request = messages(MessageListScope::AllProjects, "needle");
    request.page = page(1);
    let first = store.search_messages(request.clone()).await.unwrap().page;
    assert_eq!(first.records[0].message.message_id, newer.message_id);
    request.page.cursor = first.next_cursor;
    let mut discovery_cursor_request = DiscoverySearchRequest {
        query: query("archived"),
        scope: DiscoveryScope::AllProjects,
        kind: DiscoveryKind::All,
        include_archived: false,
        page: page(1),
    };
    let discovery_first = store
        .search_discovery(discovery_cursor_request.clone())
        .await
        .unwrap()
        .page;
    assert!(matches!(
        &discovery_first.records[0],
        DiscoveryHit::Project { .. }
    ));
    discovery_cursor_request.page.cursor = Some(discovery_first.next_cursor.unwrap());
    store
        .archive_board(BoardArchiveRequest {
            board_id: archived.board_id.clone(),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    let discovery_remaining = store
        .search_discovery(discovery_cursor_request.clone())
        .await
        .unwrap()
        .page;
    assert!(discovery_remaining.records.is_empty());
    assert!(discovery_remaining.next_cursor.is_none());
    discovery_cursor_request.include_archived = true;
    assert_eq!(
        store
            .search_discovery(discovery_cursor_request)
            .await
            .unwrap_err()
            .kind,
        BoardFailureKind::InvalidCursor
    );
    let remaining = store.search_messages(request.clone()).await.unwrap().page;
    assert!(remaining.records.is_empty());
    assert!(remaining.next_cursor.is_none());
    request.include_archived = true;
    assert_eq!(
        store.search_messages(request).await.unwrap_err().kind,
        BoardFailureKind::InvalidCursor
    );
    let mut explicit = messages(
        MessageListScope::Thread {
            root_message_id: root.message_id,
        },
        "needle",
    );
    assert!(
        store
            .search_messages(explicit.clone())
            .await
            .unwrap()
            .page
            .records
            .is_empty()
    );
    explicit.include_archived = true;
    assert_eq!(
        store.search_messages(explicit).await.unwrap().page.records[0]
            .message
            .message_id,
        older.message_id
    );
    let mut discovery = DiscoverySearchRequest {
        query: query("archived"),
        scope: DiscoveryScope::AllProjects,
        kind: DiscoveryKind::All,
        include_archived: false,
        page: page(100),
    };
    assert_eq!(
        store
            .search_discovery(discovery.clone())
            .await
            .unwrap()
            .page
            .records
            .len(),
        1
    );
    discovery.include_archived = true;
    assert_eq!(
        store
            .search_discovery(discovery)
            .await
            .unwrap()
            .page
            .records
            .len(),
        3
    );
}

#[tokio::test]
async fn large_message_search_pages_respect_bytes_and_report_scan_cost() {
    let mut store = BoardStore::open(&database_path("search-byte-budget"))
        .await
        .unwrap();
    let selected = location(&mut store, "Large search").await;
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: selected.topic_id.clone(),
        },
        actor("root"),
        "seed",
        vec![],
    )
    .await
    .message;
    let body = format!("needle {}", "x".repeat(MAX_MESSAGE_TEXT_BYTES - 7));
    for _ in 0..32 {
        post(
            &mut store,
            Placement::Thread {
                root_message_id: root.message_id.clone(),
            },
            actor("writer"),
            &body,
            vec![],
        )
        .await;
    }
    let scope = MessageListScope::Topic {
        topic_id: selected.topic_id,
    };
    let started = std::time::Instant::now();
    let empty = store
        .search_messages(messages(scope.clone(), "no-matching-substring"))
        .await
        .unwrap();
    assert!(empty.page.records.is_empty());
    let no_match_elapsed = started.elapsed();
    let mut request = messages(scope, "needle");
    let mut ids = std::collections::HashSet::new();
    let mut page_count = 0;
    let started = std::time::Instant::now();
    loop {
        let response = store.search_messages(request.clone()).await.unwrap();
        assert!(serde_json::to_vec(&response).unwrap().len() < 1024 * 1024);
        assert!(!response.page.records.is_empty());
        page_count += 1;
        for record in response.page.records {
            assert!(ids.insert(record.message.message_id));
        }
        let Some(cursor) = response.page.next_cursor else {
            break;
        };
        request.page.cursor = Some(cursor);
    }
    assert_eq!(ids.len(), 32);
    assert!(page_count > 1);
    eprintln!(
        "2 MiB thread history: no-match={}us, matching-page-chain={}us, pages={page_count}",
        no_match_elapsed.as_micros(),
        started.elapsed().as_micros()
    );
}

#[tokio::test]
async fn discovery_pages_bound_full_parent_context_without_losing_topics() {
    let mut store = BoardStore::open(&database_path("discovery-byte-budget"))
        .await
        .unwrap();
    let selected = location(&mut store, "needle-large").await;
    let full_description = description(&"x".repeat(MAX_DESCRIPTION_BYTES));
    store
        .update_project(ProjectUpdateRequest {
            project_id: selected.project_id.clone(),
            name: name("needle-large"),
            description: full_description.clone(),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    store
        .update_board(BoardUpdateRequest {
            board_id: selected.board_id.clone(),
            name: name("needle-large"),
            description: full_description.clone(),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    for index in 0..24 {
        store
            .create_topic(TopicCreateRequest {
                topic_id: TopicId::generate(),
                board_id: selected.board_id.clone(),
                name: name(&format!("needle {index}")),
                description: full_description.clone(),
                actor: actor("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
    }
    let mut request = DiscoverySearchRequest {
        query: query("needle"),
        scope: DiscoveryScope::Project {
            project_id: selected.project_id,
        },
        kind: DiscoveryKind::Topic,
        include_archived: false,
        page: page(100),
    };
    let mut ids = std::collections::HashSet::new();
    let mut page_count = 0;
    loop {
        let response = store.search_discovery(request.clone()).await.unwrap();
        assert!(serde_json::to_vec(&response).unwrap().len() < 1024 * 1024);
        page_count += 1;
        for hit in response.page.records {
            let DiscoveryHit::Topic {
                project,
                board,
                topic,
            } = hit
            else {
                panic!("non-topic result")
            };
            assert_eq!(project.description, full_description);
            assert_eq!(board.description, full_description);
            assert!(ids.insert(topic.topic_id));
        }
        let Some(cursor) = response.page.next_cursor else {
            break;
        };
        request.page.cursor = Some(cursor);
    }
    assert_eq!(ids.len(), 25);
    assert!(page_count > 1);
}
