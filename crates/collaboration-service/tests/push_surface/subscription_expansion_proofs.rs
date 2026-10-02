//! Control router/show expands a stored subscription range without acknowledging it.
use super::*;
use chrono::Utc;
use message_board::{
    BoardCreateRequest, BoardId, Description, Identity, InboxActivity, InboxFetchRequest,
    InboxReadMode, InboxScope, Message, MessageId, MessagePostRequest, MessageReferences,
    MessageText, ParticipantRole, Placement, ProjectCreateRequest, ProjectId, ResourceName,
    ServiceId, SessionEndpointRef, SessionId as BoardSessionId, SessionRef as BoardSessionRef,
    SubscriptionMode, ThreadCreateRequest, ThreadJoinRequest, TopicCreateRequest, TopicId,
    WhenIdle,
};
use message_board_storage::BoardStore;

struct SubscriptionExpansionFixture {
    _directory: tempfile::TempDir,
    board: Arc<TokioMutex<BoardStore>>,
    pushes: Arc<TokioMutex<AutomationStore>>,
    control: ControlHarness,
    project_id: ProjectId,
    root_message_id: MessageId,
    target: SessionRef,
    board_reader: Identity,
}

impl SubscriptionExpansionFixture {
    async fn start() -> Self {
        let directory = tempfile::tempdir().expect("isolated push and board databases");
        let board_path = directory.path().join("board.sqlite");
        let board = Arc::new(TokioMutex::new(
            BoardStore::open(&board_path)
                .await
                .expect("real SQLite board store"),
        ));
        let pushes = Arc::new(TokioMutex::new(
            AutomationStore::open(&directory.path().join("automation.sqlite"))
                .await
                .expect("real SQLite automation store"),
        ));
        let target = session("claude-local", "subscription-range-reader");
        let board_reader = board_session_identity(&target);
        let writer = Identity::Human {
            human_id: "subscription-range-writer"
                .to_owned()
                .try_into()
                .expect("writer identity"),
        };
        let project_id = ProjectId::generate();
        let board_id = BoardId::generate();
        let topic_id = TopicId::generate();
        let root_message_id = MessageId::generate();
        let now = Utc::now();
        {
            let mut board_store = board.lock().await;
            board_store
                .create_project(ProjectCreateRequest {
                    project_id: project_id.clone(),
                    name: ResourceName::try_from("Push expansion project".to_owned())
                        .expect("project name"),
                    description: Description::try_from(String::new()).expect("project description"),
                    actor: writer.clone(),
                    acting_for: None,
                })
                .await
                .expect("create Board project");
            board_store
                .create_board(BoardCreateRequest {
                    board_id: board_id.clone(),
                    project_id: project_id.clone(),
                    name: ResourceName::try_from("Push expansion board".to_owned())
                        .expect("board name"),
                    description: Description::try_from(String::new()).expect("board description"),
                    actor: writer.clone(),
                    acting_for: None,
                })
                .await
                .expect("create Board");
            board_store
                .create_topic(TopicCreateRequest {
                    topic_id: topic_id.clone(),
                    board_id,
                    name: ResourceName::try_from("Subscription activity".to_owned())
                        .expect("topic name"),
                    description: Description::try_from(String::new()).expect("topic description"),
                    actor: writer.clone(),
                    acting_for: None,
                })
                .await
                .expect("create Board topic");
            board_store
                .create_thread(
                    ThreadCreateRequest {
                        message_id: root_message_id.clone(),
                        topic_id,
                        actor: writer,
                        acting_for: None,
                        text: MessageText::try_from("Root title outside activity range".to_owned())
                            .expect("root text"),
                        references: MessageReferences::try_from(Vec::new())
                            .expect("root references"),
                        role: None,
                        watch: false,
                    },
                    now,
                )
                .await
                .expect("create Board thread");
            board_store
                .join_thread(
                    ThreadJoinRequest {
                        root_message_id: root_message_id.clone(),
                        actor: board_reader.clone(),
                        role: ParticipantRole::Participant,
                        watch: true,
                        mode: Some(SubscriptionMode::Deliver),
                        when_idle: Some(WhenIdle::Hold),
                        replace: None,
                        note: None,
                    },
                    now,
                )
                .await
                .expect("watch thread as the target session");
        }

        let identity = ServiceIdentity::new(
            SERVICE_ID,
            SERVICE_ID,
            &format!("sha256:{}", "a".repeat(64)),
        )
        .expect("service identity")
        .with_board_store(Arc::clone(&board))
        .with_automation_store(Arc::clone(&pushes));
        let control = ControlHarness::start(identity).await;

        Self {
            _directory: directory,
            board,
            pushes,
            control,
            project_id,
            root_message_id,
            target,
            board_reader,
        }
    }

    async fn close(self) {
        let Self {
            _directory,
            board,
            pushes,
            control,
            ..
        } = self;
        control.close().await;
        Arc::try_unwrap(board)
            .ok()
            .expect("Control releases Board storage")
            .into_inner()
            .close()
            .await
            .expect("close Board database");
        Arc::try_unwrap(pushes)
            .ok()
            .expect("Control releases automation storage")
            .into_inner()
            .close()
            .await
            .expect("close automation database");
        drop(_directory);
    }

    async fn post_thread_message(&self, body: &str) -> Message {
        self.board
            .lock()
            .await
            .post_message(
                MessagePostRequest {
                    message_id: MessageId::generate(),
                    placement: Placement::Thread {
                        root_message_id: self.root_message_id.clone(),
                    },
                    actor: Identity::Human {
                        human_id: "subscription-range-writer"
                            .to_owned()
                            .try_into()
                            .expect("writer identity"),
                    },
                    acting_for: None,
                    text: MessageText::try_from(body.to_owned()).expect("posted body"),
                    references: MessageReferences::try_from(Vec::new()).expect("posted references"),
                },
                Utc::now(),
            )
            .await
            .expect("post real Board message")
            .message
    }

    async fn unread_project_activity(&self) -> Vec<InboxActivity> {
        self.board
            .lock()
            .await
            .fetch_inbox(InboxFetchRequest {
                scope: InboxScope::Project {
                    project_id: self.project_id.clone(),
                },
                reader: self.board_reader.clone(),
                read_mode: InboxReadMode::Unread,
                page: message_board::PageRequest::default(),
            })
            .await
            .expect("read target's real Board inbox")
            .page
            .records
    }

    async fn insert_subscription_push(
        &self,
        push_id: PushId,
        from_activity_sequence: message_board::ActivitySequence,
        through_activity_sequence: message_board::ActivitySequence,
        message_count: u64,
    ) {
        let origin = RouterOriginRef::SubscriptionActivity {
            target: self.target.clone(),
            batch_id: message_board::BatchId::generate(),
        };
        self.pushes
            .lock()
            .await
            .insert_push_record(PushRecordDraft {
                push_id,
                kind: PushKind::SubscriptionActivity,
                origin: PushOrigin::Router(PushKind::SubscriptionActivity),
                origin_router_ref: Some(
                    origin
                        .canonical_string()
                        .expect("canonical subscription batch origin"),
                ),
                target: self.target.clone(),
                reply_to_push_id: None,
                header_facts: PushHeaderFacts::SubscriptionActivity {
                    root_count: 1,
                    message_count,
                    held_since: None,
                    thread_resolved: false,
                },
                body: None,
                activity: Some(PushActivitySnapshot {
                    ranges: vec![PushActivityRange {
                        root_message_id: self.root_message_id.clone(),
                        from_activity_sequence,
                        through_activity_sequence,
                    }],
                    held: false,
                    draining: false,
                }),
                mode: None,
                guard: None,
                created_at: Utc::now(),
            })
            .await
            .expect("persist subscription push in real SQLite");
    }
}

fn board_session_identity(target: &SessionRef) -> Identity {
    Identity::Session {
        session: BoardSessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from(String::from(target.endpoint.service_id.clone()))
                    .expect("Board service id"),
                endpoint_id: message_board::EndpointId::try_from(String::from(
                    target.endpoint.endpoint_id.clone(),
                ))
                .expect("Board endpoint id"),
            },
            session_id: BoardSessionId::try_from(String::from(target.session_id.clone()))
                .expect("Board session id"),
        },
    }
}

#[tokio::test]
async fn router_show_expands_only_stored_subscription_range_without_read_acknowledgement() {
    let mut fixture = SubscriptionExpansionFixture::start().await;
    let initial_unread = fixture.unread_project_activity().await;
    assert!(
        initial_unread.is_empty(),
        "establish the inbox baseline first"
    );

    let before_range = fixture
        .post_thread_message("outside-before: do not return")
        .await;
    let first_in_range = fixture
        .post_thread_message("inside-first: return this body")
        .await;
    let second_in_range = fixture
        .post_thread_message("inside-second: return this body")
        .await;
    let after_range = fixture
        .post_thread_message("outside-after: do not return")
        .await;

    let push_id = PushId::try_from(uuid::Uuid::now_v7().to_string()).expect("UUIDv7 push id");
    fixture
        .insert_subscription_push(
            push_id.clone(),
            first_in_range.activity_sequence,
            second_in_range.activity_sequence,
            2,
        )
        .await;
    let link = collaboration_protocol::RouterLink::new(
        collaboration_protocol::MachineId::from(
            UuidIdentity::try_from(SERVICE_ID.to_owned()).expect("machine UUID"),
        ),
        push_id.clone(),
    )
    .to_string();

    let shown = fixture
        .control
        .call(
            "router/show",
            json!({"caller":fixture.target.clone(),"reference":link.clone()}),
        )
        .await;
    assert!(shown.get("result").is_some(), "real Control show: {shown}");
    assert_eq!(shown["result"]["link"], link);
    assert_eq!(shown["result"]["record"]["pushId"], push_id.as_str());
    assert_eq!(shown["result"]["record"]["kind"], "subscription-activity");
    assert_eq!(
        shown["result"]["activityRanges"].as_array().map(Vec::len),
        Some(1)
    );

    let range_read = &shown["result"]["activityRanges"][0];
    assert_eq!(
        range_read["range"],
        json!({
            "rootMessageId":fixture.root_message_id.clone(),
            "fromActivitySequence":first_in_range.activity_sequence,
            "throughActivitySequence":second_in_range.activity_sequence
        })
    );
    let expanded_messages = range_read["messages"]
        .as_array()
        .expect("Control returned expanded Board messages");
    assert_eq!(expanded_messages.len(), 2);
    assert_eq!(
        expanded_messages
            .iter()
            .map(|message| message["messageId"].as_str().expect("message id"))
            .collect::<Vec<_>>(),
        vec![
            first_in_range.message_id.as_str(),
            second_in_range.message_id.as_str()
        ]
    );
    assert_eq!(
        expanded_messages[0]["text"],
        "inside-first: return this body"
    );
    assert_eq!(
        expanded_messages[1]["text"],
        "inside-second: return this body"
    );
    assert!(
        expanded_messages.iter().all(|message| message["messageId"]
            != before_range.message_id.as_str()
            && message["messageId"] != after_range.message_id.as_str()),
        "the stored activity range excludes surrounding Board messages"
    );

    assert!(
        shown["result"]["record"]["readAt"].is_null(),
        "showing a subscription push does not acknowledge it"
    );
    let stored_after_show = fixture
        .pushes
        .lock()
        .await
        .get_push_record(&push_id)
        .await
        .expect("read stored subscription push")
        .expect("subscription push remains stored");
    assert!(stored_after_show.read_at.is_none());
    assert_eq!(stored_after_show.delivery_state, PushDeliveryState::Pending);

    let unread_after_show = fixture.unread_project_activity().await;
    assert_eq!(unread_after_show.len(), 4);
    let unread_message_ids = unread_after_show
        .iter()
        .filter_map(|activity| match activity {
            InboxActivity::MessageCreated { message, .. } => {
                Some(message.message_id.as_str().to_owned())
            }
            InboxActivity::ThreadStateChanged { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        unread_message_ids,
        vec![
            before_range.message_id.as_str().to_owned(),
            first_in_range.message_id.as_str().to_owned(),
            second_in_range.message_id.as_str().to_owned(),
            after_range.message_id.as_str().to_owned()
        ],
        "range expansion reads the Board but leaves the target inbox unacknowledged"
    );

    fixture.close().await;
}
