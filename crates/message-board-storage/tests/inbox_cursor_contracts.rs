#![allow(clippy::unwrap_used)]
#[path = "board_behavior_support/mod.rs"]
mod board_behavior_support;
use board_behavior_support::*;
use message_board::*;
use message_board_storage::BoardStore;

#[tokio::test]
async fn inbox_cursors_bind_scope_mode_reader_and_store_without_initialization_on_error() {
    let mut store = BoardStore::open(&database_path("inbox-cursor-binding"))
        .await
        .unwrap();
    let fixture = create_fixture(&mut store).await;
    for author in ["first", "second"] {
        post(
            &mut store,
            Placement::Topic {
                topic_id: fixture.topic_id.clone(),
            },
            actor(author),
            author,
            vec![],
        )
        .await;
    }
    let request = InboxFetchRequest {
        scope: InboxScope::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        reader: actor("reader"),
        read_mode: InboxReadMode::Latest,
        page: page(1),
    };
    let first = store.fetch_inbox(request.clone()).await.unwrap().page;
    let cursor = first.next_cursor.unwrap();
    let mut continuation = request.clone();
    continuation.page.cursor = Some(cursor);
    let mut wrong_scope = continuation.clone();
    wrong_scope.scope = InboxScope::Project {
        project_id: fixture.project_id.clone(),
    };
    let mut wrong_mode = continuation.clone();
    wrong_mode.read_mode = InboxReadMode::Unread;
    let mut wrong_reader = continuation.clone();
    wrong_reader.reader = actor("other-reader");
    let mut malformed = request;
    malformed.read_mode = InboxReadMode::Unread;
    malformed.page.cursor = Some("not-a-cursor".to_owned());
    for invalid in [wrong_scope, wrong_mode, wrong_reader, malformed] {
        assert_eq!(
            store.fetch_inbox(invalid).await.unwrap_err().kind,
            BoardFailureKind::InvalidCursor
        );
    }
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
        store
            .list_inbox_projects(InboxProjectsRequest {
                reader: actor("other-reader"),
                unread_only: false,
                page: page(100)
            })
            .await
            .unwrap()
            .page
            .records
            .is_empty()
    );

    let mut foreign = BoardStore::open(&database_path("foreign-inbox-cursor"))
        .await
        .unwrap();
    create_fixture_with_ids(&mut foreign, &fixture).await;
    // Match the scope and upper sequence so only the foreign cursor key rejects this request.
    for author in ["first", "second"] {
        post(
            &mut foreign,
            Placement::Topic {
                topic_id: fixture.topic_id.clone(),
            },
            actor(author),
            author,
            vec![],
        )
        .await;
    }
    assert_eq!(
        foreign
            .fetch_inbox(continuation.clone())
            .await
            .unwrap_err()
            .kind,
        BoardFailureKind::InvalidCursor
    );
    assert_eq!(
        store
            .fetch_inbox(continuation)
            .await
            .unwrap()
            .page
            .records
            .len(),
        1
    );

    let mut unread = InboxFetchRequest {
        scope: InboxScope::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        reader: actor("reader"),
        read_mode: InboxReadMode::Unread,
        page: page(1),
    };
    assert!(
        store
            .fetch_inbox(unread.clone())
            .await
            .unwrap()
            .page
            .records
            .is_empty()
    );
    let third = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("third"),
        "third",
        vec![],
    )
    .await
    .message;
    let fourth = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("fourth"),
        "fourth",
        vec![],
    )
    .await
    .message;
    let unread_first = store.fetch_inbox(unread.clone()).await.unwrap().page;
    assert!(
        matches!(unread_first.records.as_slice(), [InboxActivity::MessageCreated { message, .. }] if message.message_id == third.message_id)
    );
    unread.page.cursor = Some(unread_first.next_cursor.unwrap());
    let after_upper = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id,
        },
        actor("after-upper"),
        "after upper",
        vec![],
    )
    .await
    .message;
    let unread_continued = store.fetch_inbox(unread.clone()).await.unwrap().page;
    assert!(
        matches!(unread_continued.records.as_slice(), [InboxActivity::MessageCreated { message, .. }] if message.message_id == fourth.message_id)
    );
    assert!(unread_continued.next_cursor.is_none());
    unread.page = page(100);
    let fresh = store.fetch_inbox(unread).await.unwrap().page;
    assert_eq!(fresh.records.len(), 3);
    assert!(fresh.records.iter().any(|record| matches!(record, InboxActivity::MessageCreated { message, .. } if message.message_id == after_upper.message_id)));
}

#[tokio::test]
async fn first_unread_fetch_and_post_assign_activity_to_history_or_unread_atomically() {
    let path = database_path("inbox-first-use-race");
    let mut reader_store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut reader_store).await;
    let mut writer_store = BoardStore::open(&path).await.unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let reader_barrier = barrier.clone();
    let request = InboxFetchRequest {
        scope: InboxScope::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        reader: actor("reader"),
        read_mode: InboxReadMode::Unread,
        page: page(100),
    };
    let reading = async {
        reader_barrier.wait().await;
        reader_store
            .fetch_inbox(request.clone())
            .await
            .unwrap()
            .page
    };
    let writing = async {
        barrier.wait().await;
        post(
            &mut writer_store,
            Placement::Topic {
                topic_id: fixture.topic_id,
            },
            actor("writer"),
            "racing first use",
            vec![],
        )
        .await
        .message
    };
    let (initial, message) = tokio::join!(reading, writing);
    let InboxInitializationStatus::Initialized {
        starts_after_activity_sequence,
        ..
    } = initial.initialization
    else {
        panic!("first fetch was not initialized")
    };
    let later = reader_store.fetch_inbox(request).await.unwrap().page;
    let included = later.records.iter().any(|record| matches!(record, InboxActivity::MessageCreated { message: item, .. } if item.message_id == message.message_id));
    assert_eq!(
        included,
        message.activity_sequence.get() > starts_after_activity_sequence.get()
    );
    assert_eq!(later.records.len(), usize::from(included));
}
