use super::*;

#[tokio::test]
async fn participant_cursor_is_thread_bound_and_unknown_stored_role_is_rejected() {
    let mut fixture = Fixture::open("cursor-corruption").await;
    let first = fixture.create(human("owner"), None, false).await;
    let second = fixture.create(human("owner-two"), None, false).await;
    let first_root = first.message.message_id;
    let second_root = second.message.message_id;
    let foreign_sequence = fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: second_root.clone(),
                actor: session("other-thread"),
                role: ParticipantRole::Participant,
                watch: false,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap()
        .participant
        .joined_at_activity;
    for name in ["one", "two"] {
        fixture
            .store
            .join_thread(
                ThreadJoinRequest {
                    mode: None,
                    when_idle: None,
                    root_message_id: first_root.clone(),
                    actor: session(name),
                    role: ParticipantRole::Participant,
                    watch: false,
                    replace: None,
                    note: None,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();
    }
    let first_page = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: first_root.clone(),
            page: page(1),
        })
        .await
        .unwrap();
    let cursor = first_page.page.next_cursor.unwrap();
    let failure = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: second_root,
            page: PageRequest {
                limit: PageLimit::try_from(1).unwrap(),
                cursor: Some(cursor),
            },
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidCursor);
    fixture.store.close().await.unwrap();
    let mut connection =
        sqlx::SqliteConnection::connect(&format!("sqlite://{}", fixture.path.display()))
            .await
            .unwrap();
    sqlx::query("UPDATE thread_participants SET role='driver' WHERE root_id=?")
        .bind(first_root.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&fixture.path).await.unwrap();
    let failure = store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: first_root.clone(),
            page: page(100),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();
    let mut connection =
        sqlx::SqliteConnection::connect(&format!("sqlite://{}", fixture.path.display()))
            .await
            .unwrap();
    sqlx::query("UPDATE thread_participants SET role='participant',joined_at_activity=?,last_seen_activity=? WHERE root_id=?")
        .bind(i64::try_from(foreign_sequence.get()).unwrap())
        .bind(i64::try_from(foreign_sequence.get()).unwrap())
        .bind(first_root.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&fixture.path).await.unwrap();
    let failure = store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: first_root,
            page: page(100),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[tokio::test]
async fn thread_list_rejects_corrupt_projected_orchestrator_records() {
    let mut fixture = Fixture::open("thread-list-orchestrator-corruption").await;
    let first = fixture
        .create(
            session("first-orchestrator"),
            Some(ParticipantRole::Orchestrator),
            false,
        )
        .await;
    let second = fixture.create(human("second-owner"), None, false).await;
    let first_root = first.message.message_id;
    let second_root = second.message.message_id;
    let foreign_sequence = fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: second_root,
                actor: session("foreign-participant"),
                role: ParticipantRole::Participant,
                watch: false,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap()
        .participant
        .joined_at_activity;

    fixture.store.close().await.unwrap();
    let mut connection =
        sqlx::SqliteConnection::connect(&format!("sqlite://{}", fixture.path.display()))
            .await
            .unwrap();
    sqlx::query("UPDATE thread_participants SET role='driver' WHERE root_id=?")
        .bind(first_root.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&fixture.path).await.unwrap();
    let failure = store
        .list_threads(ThreadListRequest {
            project_id: fixture.project_id.clone(),
            reader: human("inspector"),
            watched_only: false,
            page: page(100),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();

    let mut connection =
        sqlx::SqliteConnection::connect(&format!("sqlite://{}", fixture.path.display()))
            .await
            .unwrap();
    sqlx::query(
        "UPDATE thread_participants SET role='orchestrator',closed_reason='left' WHERE root_id=?",
    )
    .bind(first_root.as_str())
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&fixture.path).await.unwrap();
    let failure = store
        .list_threads(ThreadListRequest {
            project_id: fixture.project_id.clone(),
            reader: human("inspector"),
            watched_only: false,
            page: page(100),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();

    let mut connection =
        sqlx::SqliteConnection::connect(&format!("sqlite://{}", fixture.path.display()))
            .await
            .unwrap();
    sqlx::query(
        "UPDATE thread_participants SET closed_reason=NULL,last_seen_activity=? WHERE root_id=?",
    )
    .bind(i64::try_from(foreign_sequence.get()).unwrap())
    .bind(first_root.as_str())
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&fixture.path).await.unwrap();
    let failure = store
        .list_threads(ThreadListRequest {
            project_id: fixture.project_id.clone(),
            reader: human("inspector"),
            watched_only: false,
            page: page(100),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[tokio::test]
async fn archived_leave_resolve_refuses_without_changing_participant_or_watch_state() {
    let mut fixture = Fixture::open("archived-leave-resolve").await;
    let orchestrator = session("archived-orchestrator");
    let created = fixture
        .create(
            orchestrator.clone(),
            Some(ParticipantRole::Orchestrator),
            true,
        )
        .await;
    let root_message_id = created.message.message_id;
    let joined_at_activity = match created.creator_participation {
        ThreadCreatorParticipation::Joined { participant } => participant.joined_at_activity,
        ThreadCreatorParticipation::NotJoined => panic!("session creator must join"),
    };
    fixture
        .store
        .archive_board(BoardArchiveRequest {
            board_id: fixture.board_id.clone(),
            actor: human("archiver"),
            acting_for: None,
        })
        .await
        .unwrap();

    let failure = fixture
        .store
        .leave_thread(
            ThreadLeaveRequest {
                root_message_id: root_message_id.clone(),
                actor: orchestrator.clone(),
                to: None,
                resolve: true,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::ArchivedBoard);
    let thread = fixture
        .store
        .show_thread(ThreadShowRequest {
            root_message_id: root_message_id.clone(),
            reader: Some(orchestrator.clone()),
        })
        .await
        .unwrap();
    assert_eq!(thread.thread.state, ThreadState::Unresolved);
    assert_eq!(thread.thread.orchestrator.unwrap().identity, orchestrator);
    assert!(thread.watch_status.unwrap().watching);
    let participants = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id,
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records;
    assert_eq!(participants.len(), 1);
    assert!(participants[0].is_open());
    assert_eq!(participants[0].joined_at_activity, joined_at_activity);
    fixture.finish().await;
}

#[tokio::test]
async fn leave_resolve_publishes_unread_to_another_active_watcher() {
    let mut fixture = Fixture::open("leave-resolve-unread").await;
    let orchestrator = session("resolving-orchestrator");
    let root_message_id = fixture
        .create(
            orchestrator.clone(),
            Some(ParticipantRole::Orchestrator),
            false,
        )
        .await
        .message
        .message_id;
    let watcher = human("clean-watcher");
    fixture
        .store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root_message_id.clone(),
            actor: watcher.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    let before = fixture
        .store
        .list_inbox_projects(InboxProjectsRequest {
            reader: watcher.clone(),
            unread_only: false,
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records;
    assert!(
        !before
            .iter()
            .find(|summary| summary.project_id == fixture.project_id)
            .unwrap()
            .has_unread
    );

    fixture
        .store
        .leave_thread(
            ThreadLeaveRequest {
                root_message_id: root_message_id.clone(),
                actor: orchestrator,
                to: None,
                resolve: true,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let after = fixture
        .store
        .list_inbox_projects(InboxProjectsRequest {
            reader: watcher.clone(),
            unread_only: false,
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records;
    assert!(
        after
            .iter()
            .find(|summary| summary.project_id == fixture.project_id)
            .unwrap()
            .has_unread
    );
    let unread = fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: fixture.project_id.clone(),
            },
            read_mode: InboxReadMode::Unread,
            reader: watcher,
            page: page(100),
        })
        .await
        .unwrap();
    assert!(matches!(
        unread.page.records.as_slice(),
        [InboxActivity::ThreadStateChanged { root_message_id: unread_root, state: ThreadState::Resolved, .. }]
            if *unread_root == root_message_id
    ));
    fixture.finish().await;
}
