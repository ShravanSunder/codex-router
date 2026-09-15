#![allow(clippy::unwrap_used)]
use message_board::*;
use message_board_storage::BoardStore;
use sqlx::Connection;

fn human(name: &str) -> Identity {
    Identity::Human {
        human_id: HumanId::try_from(name.to_owned()).unwrap(),
    }
}

fn session(name: &str) -> Identity {
    Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from("550e8400-e29b-41d4-a716-446655440000".to_owned())
                    .unwrap(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned()).unwrap(),
            },
            session_id: SessionId::try_from(name.to_owned()).unwrap(),
        },
    }
}

fn maximum_escape_heavy_session(index: usize) -> Identity {
    let prefix = format!("{index:04}");
    let session_id = format!("{prefix}{}", "\u{1}".repeat(4_096 - prefix.len()));
    session(&session_id)
}

fn page(limit: u32) -> PageRequest {
    PageRequest {
        limit: PageLimit::try_from(limit).unwrap(),
        cursor: None,
    }
}

struct Fixture {
    store: BoardStore,
    path: std::path::PathBuf,
    project_id: ProjectId,
    board_id: BoardId,
    topic_id: TopicId,
}

impl Fixture {
    async fn open(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "board-participants-{label}-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let mut store = BoardStore::open(&path).await.unwrap();
        let project_id = ProjectId::generate();
        let board_id = BoardId::generate();
        let topic_id = TopicId::generate();
        store
            .create_project(ProjectCreateRequest {
                project_id: project_id.clone(),
                name: ResourceName::try_from("Project".to_owned()).unwrap(),
                description: Description::try_from("Participants".to_owned()).unwrap(),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_board(BoardCreateRequest {
                board_id: board_id.clone(),
                project_id: project_id.clone(),
                name: ResourceName::try_from("Board".to_owned()).unwrap(),
                description: Description::try_from("Participants".to_owned()).unwrap(),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_topic(TopicCreateRequest {
                topic_id: topic_id.clone(),
                board_id: board_id.clone(),
                name: ResourceName::try_from("Topic".to_owned()).unwrap(),
                description: Description::try_from("Participants".to_owned()).unwrap(),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        Self {
            store,
            path,
            project_id,
            board_id,
            topic_id,
        }
    }

    async fn create(
        &mut self,
        actor: Identity,
        role: Option<ParticipantRole>,
        watch: bool,
    ) -> ThreadCreateResult {
        self.store
            .create_thread(ThreadCreateRequest {
                message_id: MessageId::generate(),
                topic_id: self.topic_id.clone(),
                actor,
                acting_for: None,
                text: MessageText::try_from("Root".to_owned()).unwrap(),
                references: MessageReferences::try_from(Vec::new()).unwrap(),
                role,
                watch,
            })
            .await
            .unwrap()
    }

    async fn finish(self) {
        self.store.close().await.unwrap();
        std::fs::remove_file(self.path).unwrap();
    }
}

#[tokio::test]
async fn thread_list_pages_escape_heavy_holders_with_complete_cursor_traversal() {
    let mut fixture = Fixture::open("thread-list-frame-budget").await;
    let mut expected_roots = Vec::new();
    for index in 0..100 {
        let created = fixture
            .create(
                maximum_escape_heavy_session(index),
                Some(ParticipantRole::Orchestrator),
                false,
            )
            .await;
        expected_roots.push(created.message.message_id);
    }
    expected_roots.sort();

    let mut cursor = None;
    let mut observed_roots = Vec::new();
    loop {
        let result = fixture
            .store
            .list_threads(ThreadListRequest {
                project_id: fixture.project_id.clone(),
                reader: human("inspector"),
                watched_only: false,
                page: PageRequest {
                    limit: PageLimit::try_from(100).unwrap(),
                    cursor,
                },
            })
            .await
            .unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() < 1_048_576);
        assert!(
            result
                .page
                .records
                .iter()
                .all(|thread| thread.orchestrator.is_some())
        );
        observed_roots.extend(
            result
                .page
                .records
                .into_iter()
                .map(|thread| thread.root_message_id),
        );
        let Some(next_cursor) = result.page.next_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }
    assert_eq!(observed_roots, expected_roots);
    fixture.finish().await;
}

#[tokio::test]
async fn create_and_repeat_join_preserve_one_valid_participant_and_explicit_watch() {
    let mut fixture = Fixture::open("create-rejoin").await;
    let creator = session("creator");
    let created = fixture
        .create(creator.clone(), Some(ParticipantRole::Advisor), false)
        .await;
    assert!(!created.watch_status.watching);
    let ThreadCreatorParticipation::Joined { participant } = created.creator_participation else {
        panic!("session creator must Join atomically");
    };
    assert_eq!(participant.role, ParticipantRole::Advisor);
    assert!(created.orchestrator.is_none());

    let first_join = fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: created.message.message_id.clone(),
            actor: creator.clone(),
            role: ParticipantRole::Reviewer,
            watch: true,
            replace: None,
            note: Some(ParticipantNote::try_from("kept note".to_owned()).unwrap()),
        })
        .await
        .unwrap();
    assert!(first_join.watch_status.watching);
    let second_join = fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: created.message.message_id.clone(),
            actor: creator,
            role: ParticipantRole::Participant,
            watch: false,
            replace: None,
            note: None,
        })
        .await
        .unwrap();
    assert_eq!(second_join.participant.role, ParticipantRole::Participant);
    assert_eq!(
        second_join.participant.note.as_ref().unwrap().as_str(),
        "kept note"
    );
    assert!(!second_join.watch_status.watching);
    let root_message_id = created.message.message_id;
    let listed = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: root_message_id.clone(),
            page: page(100),
        })
        .await
        .unwrap();
    assert_eq!(listed.page.records.len(), 1);
    let left = fixture
        .store
        .leave_thread(ThreadLeaveRequest {
            root_message_id: root_message_id.clone(),
            actor: second_join.participant.identity.clone(),
            to: None,
            resolve: false,
        })
        .await
        .unwrap();
    assert_eq!(
        left.participant.closed_reason,
        Some(ParticipantClosedReason::Left)
    );
    assert_eq!(
        left.participant.closed_at_activity,
        Some(left.participant.last_seen_activity)
    );
    let rejoined = fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id,
            actor: second_join.participant.identity,
            role: ParticipantRole::Advisor,
            watch: false,
            replace: None,
            note: None,
        })
        .await
        .unwrap();
    assert!(rejoined.participant.is_open());
    assert_eq!(rejoined.participant.role, ParticipantRole::Advisor);
    assert_eq!(rejoined.participant.note.unwrap().as_str(), "kept note");
    fixture.finish().await;
}

#[tokio::test]
async fn human_create_without_role_has_no_participant_and_session_topic_post_is_mutation_free() {
    let mut fixture = Fixture::open("human-create").await;
    let missing_role = fixture
        .store
        .create_thread(ThreadCreateRequest {
            message_id: MessageId::generate(),
            topic_id: fixture.topic_id.clone(),
            actor: session("missing-role"),
            acting_for: None,
            text: MessageText::try_from("Root".to_owned()).unwrap(),
            references: MessageReferences::try_from(Vec::new()).unwrap(),
            role: None,
            watch: false,
        })
        .await
        .unwrap_err();
    assert_eq!(missing_role.kind, BoardFailureKind::InvalidField);
    let created = fixture.create(human("owner"), None, true).await;
    assert!(created.watch_status.watching);
    assert_eq!(
        created.creator_participation,
        ThreadCreatorParticipation::NotJoined
    );
    let listed = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: created.message.message_id,
            page: page(100),
        })
        .await
        .unwrap();
    assert!(listed.page.records.is_empty());
    let failure = fixture
        .store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic {
                topic_id: fixture.topic_id.clone(),
            },
            actor: session("unjoined"),
            acting_for: None,
            text: MessageText::try_from("must create".to_owned()).unwrap(),
            references: MessageReferences::try_from(Vec::new()).unwrap(),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::SessionTopicPost);
    assert_eq!(failure.next_action, BoardNextAction::CreateThread);
    fixture.finish().await;
}

#[tokio::test]
async fn replace_handover_and_human_resolve_are_atomic_sequence_transitions() {
    let mut fixture = Fixture::open("replace-resolve").await;
    let first = session("first");
    let second = session("second");
    let third = session("third");
    let created = fixture
        .create(first.clone(), Some(ParticipantRole::Orchestrator), true)
        .await;
    let root = created.message.message_id;
    fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: root.clone(),
            actor: second.clone(),
            role: ParticipantRole::Reviewer,
            watch: true,
            replace: None,
            note: None,
        })
        .await
        .unwrap();
    let self_replace = fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: root.clone(),
            actor: first.clone(),
            role: ParticipantRole::Orchestrator,
            watch: true,
            replace: Some(first.clone()),
            note: None,
        })
        .await
        .unwrap_err();
    assert_eq!(self_replace.kind, BoardFailureKind::SelfReplace);
    let stale_replace = fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: root.clone(),
            actor: third.clone(),
            role: ParticipantRole::Orchestrator,
            watch: false,
            replace: Some(session("stale-holder")),
            note: None,
        })
        .await
        .unwrap_err();
    assert_eq!(stale_replace.kind, BoardFailureKind::StaleOrchestrator);
    let still_first = fixture
        .store
        .show_thread(ThreadShowRequest {
            root_message_id: root.clone(),
            reader: None,
        })
        .await
        .unwrap();
    assert_eq!(still_first.thread.orchestrator.unwrap().identity, first);

    let replaced = fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: root.clone(),
            actor: third.clone(),
            role: ParticipantRole::Orchestrator,
            watch: false,
            replace: Some(first.clone()),
            note: None,
        })
        .await
        .unwrap();
    assert_eq!(replaced.orchestrator.unwrap().identity, third);
    let listed_threads = fixture
        .store
        .list_threads(ThreadListRequest {
            project_id: fixture.project_id.clone(),
            reader: human("inspector"),
            watched_only: false,
            page: page(100),
        })
        .await
        .unwrap();
    assert_eq!(
        listed_threads.page.records[0]
            .orchestrator
            .as_ref()
            .unwrap()
            .identity,
        third
    );
    let records = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: root.clone(),
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records;
    let old = records
        .iter()
        .find(|participant| participant.identity == first)
        .unwrap();
    assert_eq!(old.closed_reason, Some(ParticipantClosedReason::Replaced));
    assert_eq!(old.closed_at_activity, Some(old.last_seen_activity));

    let left = fixture
        .store
        .leave_thread(ThreadLeaveRequest {
            root_message_id: root.clone(),
            actor: replaced.participant.identity,
            to: Some(second.clone()),
            resolve: false,
        })
        .await
        .unwrap();
    assert_eq!(left.orchestrator.unwrap().identity, second);
    assert!(!left.watch_status.watching);
    let resolved = fixture
        .store
        .resolve_thread(ThreadResolveRequest {
            root_message_id: root.clone(),
            actor: human("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    assert_eq!(resolved.thread.state, ThreadState::Resolved);
    assert!(resolved.thread.orchestrator.is_none());
    let records = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: root.clone(),
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records;
    assert!(records.iter().all(|participant| !participant.is_open()));
    assert!(records.iter().any(|participant| {
        participant.identity == second
            && participant.closed_reason == Some(ParticipantClosedReason::Resolved)
    }));
    fixture
        .store
        .unresolve_thread(ThreadUnresolveRequest {
            root_message_id: root.clone(),
            actor: human("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    let records_after_unresolve = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: root,
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records;
    assert!(
        records_after_unresolve
            .iter()
            .all(|participant| !participant.is_open())
    );
    fixture.finish().await;
}

#[tokio::test]
async fn session_post_and_listen_require_join_while_humans_remain_exempt() {
    let mut fixture = Fixture::open("gates").await;
    let created = fixture.create(human("owner"), None, false).await;
    let root = created.message.message_id;
    let unjoined = session("unjoined");
    let post_failure = fixture
        .store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.clone(),
            },
            actor: unjoined.clone(),
            acting_for: None,
            text: MessageText::try_from("blocked".to_owned()).unwrap(),
            references: MessageReferences::try_from(Vec::new()).unwrap(),
        })
        .await
        .unwrap_err();
    assert_eq!(post_failure.kind, BoardFailureKind::ParticipantRequired);
    let resolve_failure = fixture
        .store
        .resolve_thread(ThreadResolveRequest {
            root_message_id: root.clone(),
            actor: unjoined.clone(),
            acting_for: None,
        })
        .await
        .unwrap_err();
    assert_eq!(resolve_failure.kind, BoardFailureKind::ParticipantRequired);
    let joined_non_orchestrator = session("joined-non-orchestrator");
    fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: root.clone(),
            actor: joined_non_orchestrator.clone(),
            role: ParticipantRole::Reviewer,
            watch: false,
            replace: None,
            note: None,
        })
        .await
        .unwrap();
    let resolve_failure = fixture
        .store
        .resolve_thread(ThreadResolveRequest {
            root_message_id: root.clone(),
            actor: joined_non_orchestrator,
            acting_for: None,
        })
        .await
        .unwrap_err();
    assert_eq!(resolve_failure.kind, BoardFailureKind::OrchestratorRequired);
    let listen_failure = fixture
        .store
        .prepare_thread_listen(&ThreadListenRequest {
            reader: unjoined,
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![root.clone()],
            },
            mode: ThreadListenMode::Once {
                max_wait_seconds: 1,
            },
            acknowledge: false,
            from_activity_sequence: None,
        })
        .await
        .unwrap_err();
    assert_eq!(listen_failure.kind, BoardFailureKind::ParticipantRequired);
    let human_reply = fixture
        .store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.clone(),
            },
            actor: human("human-reader"),
            acting_for: None,
            text: MessageText::try_from("human reply".to_owned()).unwrap(),
            references: MessageReferences::try_from(Vec::new()).unwrap(),
        })
        .await
        .unwrap();
    assert!(!human_reply.watch_status.watching);
    fixture
        .store
        .prepare_thread_listen(&ThreadListenRequest {
            reader: human("human-reader"),
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![root],
            },
            mode: ThreadListenMode::Once {
                max_wait_seconds: 1,
            },
            acknowledge: false,
            from_activity_sequence: None,
        })
        .await
        .unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn listen_refuses_every_missing_session_participant_without_creating_watches() {
    let mut fixture = Fixture::open("listen-all-missing").await;
    let first_root = fixture
        .create(human("owner-one"), None, false)
        .await
        .message
        .message_id;
    let second_root = fixture
        .create(human("owner-two"), None, false)
        .await
        .message
        .message_id;
    let reader = session("unjoined-listener");

    let failure = fixture
        .store
        .prepare_thread_listen(&ThreadListenRequest {
            reader: reader.clone(),
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![first_root.clone(), second_root.clone()],
            },
            mode: ThreadListenMode::Once {
                max_wait_seconds: 1,
            },
            acknowledge: false,
            from_activity_sequence: None,
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::ParticipantRequired);
    let BoardErrorDetails::ParticipantRefusal { refusal } = failure.details else {
        panic!("missing participant refusal details");
    };
    assert_eq!(refusal.root_message_id, first_root);
    assert_eq!(
        refusal.missing_root_message_ids,
        vec![first_root.clone(), second_root.clone()]
    );
    for root_message_id in [first_root, second_root] {
        let thread = fixture
            .store
            .show_thread(ThreadShowRequest {
                root_message_id,
                reader: Some(reader.clone()),
            })
            .await
            .unwrap();
        assert!(!thread.watch_status.unwrap().watching);
    }
    fixture.finish().await;
}

#[tokio::test]
async fn concurrent_orchestrator_joins_commit_one_holder_and_report_the_winner() {
    let mut fixture = Fixture::open("concurrent-holder").await;
    let created = fixture.create(human("owner"), None, false).await;
    let root = created.message.message_id;
    fixture.store.close().await.unwrap();
    let mut first_store = BoardStore::open(&fixture.path).await.unwrap();
    let mut second_store = BoardStore::open(&fixture.path).await.unwrap();
    let first = session("first-racer");
    let second = session("second-racer");
    let first_request = ThreadJoinRequest {
        root_message_id: root.clone(),
        actor: first.clone(),
        role: ParticipantRole::Orchestrator,
        watch: false,
        replace: None,
        note: None,
    };
    let second_request = ThreadJoinRequest {
        root_message_id: root.clone(),
        actor: second.clone(),
        role: ParticipantRole::Orchestrator,
        watch: false,
        replace: None,
        note: None,
    };
    let (first_result, second_result) = tokio::join!(
        first_store.join_thread(first_request),
        second_store.join_thread(second_request)
    );
    let successes = usize::from(first_result.is_ok()) + usize::from(second_result.is_ok());
    assert_eq!(successes, 1);
    let failure = first_result.err().or_else(|| second_result.err()).unwrap();
    assert_eq!(failure.kind, BoardFailureKind::OrchestratorAlreadyExists);
    assert_eq!(failure.next_action, BoardNextAction::ReplaceOrchestrator);
    let shown = first_store
        .show_thread(ThreadShowRequest {
            root_message_id: root,
            reader: None,
        })
        .await
        .unwrap();
    assert!(matches!(
        shown.thread.orchestrator.unwrap().identity,
        identity if identity == first || identity == second
    ));
    first_store.close().await.unwrap();
    second_store.close().await.unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[tokio::test]
async fn emitted_batch_advances_last_seen_monotonically_and_lifecycle_is_not_acknowledgeable() {
    let mut fixture = Fixture::open("listen-presence").await;
    let created = fixture.create(human("owner"), None, false).await;
    let root = created.message.message_id;
    let reader = session("listener");
    let joined = fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: root.clone(),
            actor: reader.clone(),
            role: ParticipantRole::Reviewer,
            watch: true,
            replace: None,
            note: None,
        })
        .await
        .unwrap();
    let join_sequence = joined.participant.joined_at_activity;
    let external_message = fixture
        .store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.clone(),
            },
            actor: human("external"),
            acting_for: None,
            text: MessageText::try_from("external".to_owned()).unwrap(),
            references: MessageReferences::try_from(Vec::new()).unwrap(),
        })
        .await
        .unwrap();
    let context = fixture
        .store
        .prepare_thread_listen(&ThreadListenRequest {
            reader: reader.clone(),
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![root.clone()],
            },
            mode: ThreadListenMode::Once {
                max_wait_seconds: 1,
            },
            acknowledge: false,
            from_activity_sequence: Some(join_sequence),
        })
        .await
        .unwrap();
    let own_message = fixture
        .store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.clone(),
            },
            actor: reader.clone(),
            acting_for: None,
            text: MessageText::try_from("own newer presence".to_owned()).unwrap(),
            references: MessageReferences::try_from(Vec::new()).unwrap(),
        })
        .await
        .unwrap();
    let batch = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, 900 * 1024)
        .await
        .unwrap();
    assert_eq!(batch.batches.len(), 1);
    assert_eq!(
        batch.batches[0].delivered_through,
        external_message.message.activity_sequence
    );
    let participant = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: root.clone(),
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records
        .into_iter()
        .find(|participant| participant.identity == reader)
        .unwrap();
    assert_eq!(
        participant.last_seen_activity, own_message.message.activity_sequence,
        "an older emitted Batch cannot reduce last-seen"
    );
    let acknowledgement = fixture
        .store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: human("owner"),
            acting_for: None,
            scope: ReadScope::Thread {
                root_message_id: root.clone(),
            },
            through_activity_sequence: join_sequence,
        })
        .await
        .unwrap_err();
    assert_eq!(
        acknowledgement.kind,
        BoardFailureKind::InvalidAcknowledgement
    );
    fixture
        .store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.clone(),
            actor: human("observer"),
            acting_for: None,
        })
        .await
        .unwrap();
    fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: root,
            actor: session("later-participant"),
            role: ParticipantRole::Advisor,
            watch: false,
            replace: None,
            note: None,
        })
        .await
        .unwrap();
    fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: fixture.project_id.clone(),
            },
            read_mode: InboxReadMode::Latest,
            reader: human("observer"),
            page: page(100),
        })
        .await
        .expect("Participant lifecycle Activity is excluded from inbox decoding");
    fixture.finish().await;
}

#[tokio::test]
async fn participant_cursor_is_thread_bound_and_unknown_stored_role_is_rejected() {
    let mut fixture = Fixture::open("cursor-corruption").await;
    let first = fixture.create(human("owner"), None, false).await;
    let second = fixture.create(human("owner-two"), None, false).await;
    let first_root = first.message.message_id;
    let second_root = second.message.message_id;
    let foreign_sequence = fixture
        .store
        .join_thread(ThreadJoinRequest {
            root_message_id: second_root.clone(),
            actor: session("other-thread"),
            role: ParticipantRole::Participant,
            watch: false,
            replace: None,
            note: None,
        })
        .await
        .unwrap()
        .participant
        .joined_at_activity;
    for name in ["one", "two"] {
        fixture
            .store
            .join_thread(ThreadJoinRequest {
                root_message_id: first_root.clone(),
                actor: session(name),
                role: ParticipantRole::Participant,
                watch: false,
                replace: None,
                note: None,
            })
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
        .join_thread(ThreadJoinRequest {
            root_message_id: second_root,
            actor: session("foreign-participant"),
            role: ParticipantRole::Participant,
            watch: false,
            replace: None,
            note: None,
        })
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
        .leave_thread(ThreadLeaveRequest {
            root_message_id: root_message_id.clone(),
            actor: orchestrator.clone(),
            to: None,
            resolve: true,
        })
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
        .leave_thread(ThreadLeaveRequest {
            root_message_id: root_message_id.clone(),
            actor: orchestrator,
            to: None,
            resolve: true,
        })
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
