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
            .create_thread(
                ThreadCreateRequest {
                    message_id: MessageId::generate(),
                    topic_id: self.topic_id.clone(),
                    actor,
                    acting_for: None,
                    text: MessageText::try_from("Root".to_owned()).unwrap(),
                    references: MessageReferences::try_from(Vec::new()).unwrap(),
                    role,
                    watch,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap()
    }

    async fn finish(self) {
        self.store.close().await.unwrap();
        std::fs::remove_file(self.path).unwrap();
    }
}

#[tokio::test]
async fn implementer_seat_is_unique_replaceable_and_visible_on_thread_reads() {
    let mut fixture = Fixture::open("implementer-seat").await;
    let created = fixture
        .create(
            session("coordinator"),
            Some(ParticipantRole::Orchestrator),
            true,
        )
        .await;
    let root = created.message.message_id;
    let first = session("implementer-one");
    let second = session("implementer-two");
    let joined = fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root.clone(),
                actor: first.clone(),
                role: ParticipantRole::Implementer,
                watch: true,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert_eq!(
        joined.implementer.as_ref().map(|holder| &holder.identity),
        Some(&first)
    );
    let refusal = fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root.clone(),
                actor: second.clone(),
                role: ParticipantRole::Implementer,
                watch: true,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(refusal.kind, BoardFailureKind::ImplementerAlreadyExists);
    assert_eq!(refusal.next_action, BoardNextAction::ReplaceImplementer);
    fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root.clone(),
                actor: second.clone(),
                role: ParticipantRole::Implementer,
                watch: true,
                replace: Some(first),
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let shown = fixture
        .store
        .show_thread(ThreadShowRequest {
            root_message_id: root.clone(),
            reader: None,
        })
        .await
        .unwrap();
    assert_eq!(
        shown.thread.implementer.map(|holder| holder.identity),
        Some(second.clone())
    );
    let participants = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: root,
            page: page(20),
        })
        .await
        .unwrap();
    assert_eq!(
        participants.implementer.map(|holder| holder.identity),
        Some(second)
    );
    fixture.finish().await;
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
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: created.message.message_id.clone(),
                actor: creator.clone(),
                role: ParticipantRole::Reviewer,
                watch: true,
                replace: None,
                note: Some(ParticipantNote::try_from("kept note".to_owned()).unwrap()),
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert!(first_join.watch_status.watching);
    let second_join = fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: created.message.message_id.clone(),
                actor: creator,
                role: ParticipantRole::Participant,
                watch: false,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
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
        .leave_thread(
            ThreadLeaveRequest {
                root_message_id: root_message_id.clone(),
                actor: second_join.participant.identity.clone(),
                to: None,
                resolve: false,
            },
            chrono::Utc::now(),
        )
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
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id,
                actor: second_join.participant.identity,
                role: ParticipantRole::Advisor,
                watch: false,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
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
        .create_thread(
            ThreadCreateRequest {
                message_id: MessageId::generate(),
                topic_id: fixture.topic_id.clone(),
                actor: session("missing-role"),
                acting_for: None,
                text: MessageText::try_from("Root".to_owned()).unwrap(),
                references: MessageReferences::try_from(Vec::new()).unwrap(),
                role: None,
                watch: false,
            },
            chrono::Utc::now(),
        )
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
        .post_message(
            MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Topic {
                    topic_id: fixture.topic_id.clone(),
                },
                actor: session("unjoined"),
                acting_for: None,
                text: MessageText::try_from("must create".to_owned()).unwrap(),
                references: MessageReferences::try_from(Vec::new()).unwrap(),
            },
            chrono::Utc::now(),
        )
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
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root.clone(),
                actor: second.clone(),
                role: ParticipantRole::Reviewer,
                watch: true,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let self_replace = fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root.clone(),
                actor: first.clone(),
                role: ParticipantRole::Orchestrator,
                watch: true,
                replace: Some(first.clone()),
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(self_replace.kind, BoardFailureKind::SelfReplace);
    let stale_replace = fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root.clone(),
                actor: third.clone(),
                role: ParticipantRole::Orchestrator,
                watch: false,
                replace: Some(session("stale-holder")),
                note: None,
            },
            chrono::Utc::now(),
        )
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
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root.clone(),
                actor: third.clone(),
                role: ParticipantRole::Orchestrator,
                watch: false,
                replace: Some(first.clone()),
                note: None,
            },
            chrono::Utc::now(),
        )
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
        .leave_thread(
            ThreadLeaveRequest {
                root_message_id: root.clone(),
                actor: replaced.participant.identity,
                to: Some(second.clone()),
                resolve: false,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert_eq!(left.orchestrator.unwrap().identity, second);
    assert!(!left.watch_status.watching);
    let resolved = fixture
        .store
        .resolve_thread(
            ThreadResolveRequest {
                root_message_id: root.clone(),
                actor: human("owner"),
                acting_for: None,
            },
            chrono::Utc::now(),
        )
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

#[path = "thread_participants/corruption_tests.rs"]
mod corruption_tests;
#[path = "thread_participants/subscription_tests.rs"]
mod subscription_tests;
