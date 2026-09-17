#![allow(clippy::unwrap_used)]
use message_board::*;
use message_board_storage::BoardStore;
use sqlx::{Connection, SqliteConnection};

struct ThreadListenFixture {
    path: std::path::PathBuf,
    store: BoardStore,
    project_id: ProjectId,
    board_id: BoardId,
    topic_id: TopicId,
    root_message_id: MessageId,
    reader: Identity,
}

impl ThreadListenFixture {
    async fn create(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "thread-listen-{label}-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let mut store = BoardStore::open(&path).await.unwrap();
        let project_id = ProjectId::generate();
        let board_id = BoardId::generate();
        let topic_id = TopicId::generate();
        let owner = actor("owner");
        store
            .create_project(ProjectCreateRequest {
                project_id: project_id.clone(),
                name: name("Listen project"),
                description: description("Storage proof"),
                actor: owner.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_board(BoardCreateRequest {
                board_id: board_id.clone(),
                project_id: project_id.clone(),
                name: name("Listen board"),
                description: description("Storage proof"),
                actor: owner.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_topic(TopicCreateRequest {
                topic_id: topic_id.clone(),
                board_id: board_id.clone(),
                name: name("Listen topic"),
                description: description("Storage proof"),
                actor: owner.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        let root = post(
            &mut store,
            Placement::Topic {
                topic_id: topic_id.clone(),
            },
            owner,
            "Root",
        )
        .await;
        let reader = actor("reader");
        store
            .watch_thread(ThreadWatchRequest {
                root_message_id: root.message.message_id.clone(),
                actor: reader.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        Self {
            path,
            store,
            project_id,
            board_id,
            topic_id,
            root_message_id: root.message.message_id,
            reader,
        }
    }

    fn request(&self, from_activity_sequence: Option<ActivitySequence>) -> ThreadListenRequest {
        ThreadListenRequest {
            reader: self.reader.clone(),
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![self.root_message_id.clone()],
            },
            mode: ThreadListenMode::Once {
                max_wait_seconds: 540,
            },
            from_activity_sequence,
            acknowledge: false,
            delivery: ThreadListenDelivery::Stdout,
        }
    }

    async fn reply(&mut self, author: &str, body: &str) -> Message {
        post(
            &mut self.store,
            Placement::Thread {
                root_message_id: self.root_message_id.clone(),
            },
            actor(author),
            body,
        )
        .await
        .message
    }

    async fn finish(self) {
        self.store.close().await.unwrap();
        std::fs::remove_file(self.path).unwrap();
    }
}

#[tokio::test]
async fn watched_selection_groups_each_thread_and_named_selection_creates_a_watch() {
    let mut fixture = ThreadListenFixture::create("selections").await;
    let second_root = post(
        &mut fixture.store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("second-owner"),
        "Second root",
    )
    .await
    .message;
    fixture
        .store
        .watch_thread(ThreadWatchRequest {
            root_message_id: second_root.message_id.clone(),
            actor: fixture.reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    let first_activity = fixture.reply("writer", "First Thread Activity").await;
    let second_activity = post(
        &mut fixture.store,
        Placement::Thread {
            root_message_id: second_root.message_id.clone(),
        },
        actor("writer"),
        "Second Thread Activity",
    )
    .await
    .message;
    let watched = ThreadListenRequest {
        reader: fixture.reader.clone(),
        selection: ThreadListenSelection::Watched,
        mode: ThreadListenMode::Once {
            max_wait_seconds: 540,
        },
        from_activity_sequence: None,
        acknowledge: false,
        delivery: ThreadListenDelivery::Stdout,
    };
    let context = fixture.store.prepare_thread_listen(&watched).await.unwrap();
    let batch_set = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
        .await
        .unwrap();
    assert_eq!(batch_set.batches.len(), 2);
    assert!(batch_set.batches.iter().any(|batch| {
        batch.root_message_id == fixture.root_message_id
            && batch.messages[0].activity_sequence == first_activity.activity_sequence
    }));
    assert!(batch_set.batches.iter().any(|batch| {
        batch.root_message_id == second_root.message_id
            && batch.messages[0].activity_sequence == second_activity.activity_sequence
    }));

    let new_reader = actor("new-reader");
    let named = ThreadListenRequest {
        reader: new_reader.clone(),
        selection: ThreadListenSelection::Roots {
            root_message_ids: vec![fixture.root_message_id.clone()],
        },
        mode: ThreadListenMode::Once {
            max_wait_seconds: 540,
        },
        from_activity_sequence: None,
        acknowledge: false,
        delivery: ThreadListenDelivery::Stdout,
    };
    fixture.store.prepare_thread_listen(&named).await.unwrap();
    let watched = fixture
        .store
        .show_thread(ThreadShowRequest {
            root_message_id: fixture.root_message_id.clone(),
            reader: Some(new_reader),
        })
        .await
        .unwrap();
    assert!(watched.watch_status.unwrap().watching);
    fixture.finish().await;
}

#[tokio::test]
async fn topic_watched_roots_skip_the_participant_gate_for_a_session_reader() {
    // Arrange: a session Reader that has joined nothing arms a Topic listen.
    let mut fixture = ThreadListenFixture::create("topic-watched-session").await;
    let reader = session_reader("listening-session");
    let topic_request = ThreadListenRequest {
        reader: reader.clone(),
        selection: ThreadListenSelection::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        mode: ThreadListenMode::Once {
            max_wait_seconds: ThreadListenLifetime::Short.seconds(),
        },
        from_activity_sequence: None,
        acknowledge: false,
        delivery: ThreadListenDelivery::Stdout,
    };
    fixture
        .store
        .prepare_thread_listen(&topic_request)
        .await
        .unwrap();
    let posted = post(
        &mut fixture.store,
        Placement::Thread {
            root_message_id: fixture.root_message_id.clone(),
        },
        actor("other"),
        "Topic activity",
    )
    .await
    .message;

    // Act: the same session arms --watched, reaching the Thread only through its Topic Watch.
    let watched = fixture
        .store
        .prepare_thread_listen(&ThreadListenRequest {
            reader: reader.clone(),
            selection: ThreadListenSelection::Watched,
            mode: ThreadListenMode::Once {
                max_wait_seconds: ThreadListenLifetime::Short.seconds(),
            },
            from_activity_sequence: None,
            acknowledge: false,
            delivery: ThreadListenDelivery::Stdout,
        })
        .await
        .unwrap();
    let batch = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &watched, usize::MAX)
        .await
        .unwrap();

    // Assert: batches, not a participants_required refusal.
    assert!(batch.batches.iter().any(|record| {
        record
            .messages
            .iter()
            .any(|message| message.message_id == posted.message_id)
    }));

    // Act: a Thread Watch on a Thread outside every watched Topic keeps its gate.
    let other_topic = TopicId::generate();
    fixture
        .store
        .create_topic(TopicCreateRequest {
            topic_id: other_topic.clone(),
            board_id: fixture.board_id.clone(),
            name: name("Unwatched topic"),
            description: description("Storage proof"),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    let unjoined = post(
        &mut fixture.store,
        Placement::Topic {
            topic_id: other_topic,
        },
        actor("other"),
        "Another root",
    )
    .await
    .message;
    fixture
        .store
        .watch_thread(ThreadWatchRequest {
            root_message_id: unjoined.message_id.clone(),
            actor: reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    let refused = fixture
        .store
        .prepare_thread_listen(&ThreadListenRequest {
            reader,
            selection: ThreadListenSelection::Watched,
            mode: ThreadListenMode::Once {
                max_wait_seconds: ThreadListenLifetime::Short.seconds(),
            },
            from_activity_sequence: None,
            acknowledge: false,
            delivery: ThreadListenDelivery::Stdout,
        })
        .await;

    // Assert.
    assert!(
        refused.is_err(),
        "a directly watched unjoined Thread still refuses"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn topic_selection_delivers_roots_created_after_arming_and_topic_watch_feeds_watched() {
    let mut fixture = ThreadListenFixture::create("topic-selection").await;
    let request = ThreadListenRequest {
        reader: fixture.reader.clone(),
        selection: ThreadListenSelection::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        mode: ThreadListenMode::Once {
            max_wait_seconds: ThreadListenLifetime::Short.seconds(),
        },
        from_activity_sequence: None,
        acknowledge: false,
        delivery: ThreadListenDelivery::Stdout,
    };
    let context = fixture.store.prepare_thread_listen(&request).await.unwrap();
    let new_root = post(
        &mut fixture.store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("other"),
        "New root",
    )
    .await
    .message;
    assert!(
        fixture
            .store
            .thread_listen_has_activity(&context)
            .await
            .unwrap()
    );
    let batch = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
        .await
        .unwrap();
    assert!(!batch.catch_up);
    assert!(batch.batches.iter().any(|record| {
        record.root_message_id == new_root.message_id
            && record
                .messages
                .iter()
                .any(|message| message.message_id == new_root.message_id)
    }));

    let watched = fixture
        .store
        .prepare_thread_listen(&ThreadListenRequest {
            reader: fixture.reader.clone(),
            selection: ThreadListenSelection::Watched,
            mode: ThreadListenMode::Once {
                max_wait_seconds: ThreadListenLifetime::Short.seconds(),
            },
            from_activity_sequence: None,
            acknowledge: false,
            delivery: ThreadListenDelivery::Stdout,
        })
        .await
        .unwrap();
    assert!(watched.topic_ids.contains(&fixture.topic_id));
    fixture.finish().await;
}

#[tokio::test]
async fn batches_are_exactly_once_gap_free_ordered_and_exclude_reader_activity() {
    let mut fixture = ThreadListenFixture::create("exactly-once").await;
    let own = fixture.reply("reader", "Reader activity").await;
    let first = fixture.reply("writer", "First eligible Activity").await;
    let second = fixture.reply("writer", "Second eligible Activity").await;
    let context = fixture
        .store
        .prepare_thread_listen(&fixture.request(None))
        .await
        .unwrap();

    assert!(
        fixture
            .store
            .thread_listen_has_activity(&context)
            .await
            .unwrap()
    );
    let batch_set = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
        .await
        .unwrap();
    assert_eq!(batch_set.batches.len(), 1);
    let batch = &batch_set.batches[0];
    assert_eq!(batch.root_message_id, fixture.root_message_id);
    assert_eq!(batch.delivered_through, second.activity_sequence);
    assert_eq!(
        batch
            .messages
            .iter()
            .map(|message| message.activity_sequence)
            .collect::<Vec<_>>(),
        vec![first.activity_sequence, second.activity_sequence]
    );
    assert!(
        batch
            .messages
            .iter()
            .all(|message| message.activity_sequence != own.activity_sequence)
    );
    assert!(
        !fixture
            .store
            .thread_listen_has_activity(&context)
            .await
            .unwrap()
    );
    assert!(
        fixture
            .store
            .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
            .await
            .unwrap()
            .batches
            .is_empty()
    );
    fixture.finish().await;
}

#[tokio::test]
async fn committed_delivery_survives_reopen_without_acknowledging_the_inbox() {
    let mut fixture = ThreadListenFixture::create("crash-window").await;
    let delivered = fixture.reply("writer", "Committed before emit").await;
    let context = fixture
        .store
        .prepare_thread_listen(&fixture.request(None))
        .await
        .unwrap();
    let first = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        first.batches[0].delivered_through,
        delivered.activity_sequence
    );

    fixture.store.close().await.unwrap();
    fixture.store = BoardStore::open(&fixture.path).await.unwrap();
    assert!(
        fixture
            .store
            .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
            .await
            .unwrap()
            .batches
            .is_empty()
    );
    let unread = fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: fixture.project_id.clone(),
            },
            reader: fixture.reader.clone(),
            read_mode: InboxReadMode::Unread,
            page: PageRequest::default(),
        })
        .await
        .unwrap();
    assert!(unread.page.records.iter().any(|activity| {
        matches!(activity, InboxActivity::MessageCreated { activity_sequence, .. } if *activity_sequence == delivered.activity_sequence)
    }));
    let later = fixture.reply("writer", "After the crash window").await;
    let next = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
        .await
        .unwrap();
    assert_eq!(next.batches[0].messages.len(), 1);
    assert_eq!(
        next.batches[0].messages[0].activity_sequence,
        later.activity_sequence
    );
    fixture.finish().await;
}

#[tokio::test]
async fn from_initializes_only_a_first_delivered_position_inside_the_watch_boundary() {
    let mut fixture = ThreadListenFixture::create("from").await;
    let skipped = fixture.reply("writer", "Skip through this Activity").await;
    let delivered = fixture.reply("writer", "Replay from sequence").await;
    let request = fixture.request(Some(skipped.activity_sequence));
    let context = fixture.store.prepare_thread_listen(&request).await.unwrap();
    let batch_set = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
        .await
        .unwrap();
    assert_eq!(batch_set.batches[0].messages.len(), 1);
    assert_eq!(
        batch_set.batches[0].messages[0].activity_sequence,
        delivered.activity_sequence
    );
    assert!(
        fixture
            .store
            .prepare_thread_listen(&request)
            .await
            .unwrap_err()
            .message
            .contains("only before")
    );
    fixture.finish().await;
}

#[tokio::test]
async fn invalid_from_boundaries_return_errors_without_advancing_delivery() {
    let mut fixture = ThreadListenFixture::create("invalid-from").await;
    let watch_start = fixture
        .store
        .show_thread(ThreadShowRequest {
            root_message_id: fixture.root_message_id.clone(),
            reader: Some(fixture.reader.clone()),
        })
        .await
        .unwrap()
        .watch_status
        .unwrap()
        .starts_after_activity_sequence
        .unwrap();
    assert!(watch_start.get() > 0);
    let activity = fixture.reply("writer", "Still pending after errors").await;
    let below_watch = ActivitySequence::try_from(watch_start.get() - 1).unwrap();
    assert!(
        fixture
            .store
            .prepare_thread_listen(&fixture.request(Some(below_watch)))
            .await
            .is_err()
    );
    let above_latest = ActivitySequence::try_from(activity.activity_sequence.get() + 1).unwrap();
    assert!(
        fixture
            .store
            .prepare_thread_listen(&fixture.request(Some(above_latest)))
            .await
            .is_err()
    );
    let context = fixture
        .store
        .prepare_thread_listen(&fixture.request(None))
        .await
        .unwrap();
    let batch = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
        .await
        .unwrap();
    assert_eq!(batch.batches[0].messages.len(), 1);
    assert_eq!(
        batch.batches[0].messages[0].activity_sequence,
        activity.activity_sequence
    );
    fixture.finish().await;
}

#[tokio::test]
async fn acknowledged_position_advances_only_after_an_emitted_batch() {
    let mut fixture = ThreadListenFixture::create("acknowledge").await;
    let delivered = fixture.reply("writer", "Handle me").await;
    let context = fixture
        .store
        .prepare_thread_listen(&fixture.request(None))
        .await
        .unwrap();
    assert!(
        fixture
            .store
            .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
            .await
            .unwrap()
            .batches
            .len()
            == 1
    );
    let before = fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: fixture.project_id.clone(),
            },
            reader: fixture.reader.clone(),
            read_mode: InboxReadMode::Unread,
            page: PageRequest::default(),
        })
        .await
        .unwrap();
    assert!(!before.page.records.is_empty());
    fixture
        .store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: fixture.reader.clone(),
            acting_for: None,
            scope: ReadScope::Thread {
                root_message_id: fixture.root_message_id.clone(),
            },
            through_activity_sequence: delivered.activity_sequence,
        })
        .await
        .unwrap();
    let after = fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: fixture.project_id.clone(),
            },
            reader: fixture.reader.clone(),
            read_mode: InboxReadMode::Unread,
            page: PageRequest::default(),
        })
        .await
        .unwrap();
    assert!(after.page.records.is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn reactivated_watch_reinitializes_delivered_position_at_its_new_start() {
    let mut fixture = ThreadListenFixture::create("reactivate").await;
    fixture.reply("writer", "Before unwatch").await;
    let first_context = fixture
        .store
        .prepare_thread_listen(&fixture.request(None))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .select_thread_listen_batch_set(ListenId::generate(), &first_context, usize::MAX)
            .await
            .unwrap()
            .batches
            .len(),
        1
    );
    fixture
        .store
        .unwatch_thread(ThreadUnwatchRequest {
            root_message_id: fixture.root_message_id.clone(),
            actor: fixture.reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    fixture.reply("writer", "While inactive").await;
    fixture
        .store
        .watch_thread(ThreadWatchRequest {
            root_message_id: fixture.root_message_id.clone(),
            actor: fixture.reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    assert!(
        !fixture
            .store
            .thread_listen_has_activity(&first_context)
            .await
            .unwrap()
    );
    let after_reactivation = fixture.reply("writer", "After reactivation").await;
    let batch_set = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &first_context, usize::MAX)
        .await
        .unwrap();
    assert_eq!(batch_set.batches[0].messages.len(), 1);
    assert_eq!(
        batch_set.batches[0].messages[0].activity_sequence,
        after_reactivation.activity_sequence
    );
    fixture.finish().await;
}

#[tokio::test]
async fn delivered_position_from_another_thread_is_rejected_at_every_listen_boundary() {
    let mut fixture = ThreadListenFixture::create("wrong-thread-position").await;
    fixture.reply("writer", "First Thread Activity").await;
    let context = fixture
        .store
        .prepare_thread_listen(&fixture.request(None))
        .await
        .unwrap();
    fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
        .await
        .unwrap();

    let other_root = post(
        &mut fixture.store,
        Placement::Topic {
            topic_id: fixture.topic_id.clone(),
        },
        actor("other-owner"),
        "Other root",
    )
    .await
    .message;
    let other_activity = post(
        &mut fixture.store,
        Placement::Thread {
            root_message_id: other_root.message_id,
        },
        actor("other-writer"),
        "Other Thread Activity",
    )
    .await
    .message;
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", fixture.path.display()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE thread_delivery_positions SET delivered_through=? WHERE reader_key=? AND root_id=?",
    )
    .bind(i64::try_from(other_activity.activity_sequence.get()).unwrap())
    .bind("human:reader")
    .bind(fixture.root_message_id.as_str())
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    let preparation_error = fixture
        .store
        .prepare_thread_listen(&fixture.request(None))
        .await
        .unwrap_err();
    assert_eq!(preparation_error.kind, BoardFailureKind::InvalidRecord);
    let observation_error = fixture
        .store
        .thread_listen_latest_activity(&context)
        .await
        .unwrap_err();
    assert_eq!(observation_error.kind, BoardFailureKind::InvalidRecord);
    let selection_error = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, usize::MAX)
        .await
        .unwrap_err();
    assert_eq!(selection_error.kind, BoardFailureKind::InvalidRecord);
    fixture.finish().await;
}

#[tokio::test]
async fn batch_set_byte_budget_retains_overflow_for_the_next_listen() {
    let mut fixture = ThreadListenFixture::create("batch-byte-budget").await;
    let maximum_message = "x".repeat(MAX_MESSAGE_TEXT_BYTES);
    let mut expected_sequences = Vec::new();
    for _ in 0..18 {
        expected_sequences.push(
            fixture
                .reply("writer", &maximum_message)
                .await
                .activity_sequence,
        );
    }
    let context = fixture
        .store
        .prepare_thread_listen(&fixture.request(None))
        .await
        .unwrap();
    let maximum_bytes = 1024 * 1024 - 4096;
    let first_batch_set = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, maximum_bytes)
        .await
        .unwrap();
    assert!(serde_json::to_vec(&first_batch_set).unwrap().len() <= maximum_bytes);
    assert!(first_batch_set.batches[0].messages.len() < expected_sequences.len());
    let mut delivered_sequences = first_batch_set.batches[0]
        .messages
        .iter()
        .map(|message| message.activity_sequence)
        .collect::<Vec<_>>();
    let second_batch_set = fixture
        .store
        .select_thread_listen_batch_set(ListenId::generate(), &context, maximum_bytes)
        .await
        .unwrap();
    assert!(serde_json::to_vec(&second_batch_set).unwrap().len() <= maximum_bytes);
    delivered_sequences.extend(
        second_batch_set.batches[0]
            .messages
            .iter()
            .map(|message| message.activity_sequence),
    );
    assert_eq!(delivered_sequences, expected_sequences);
    fixture.finish().await;
}

async fn post(
    store: &mut BoardStore,
    placement: Placement,
    author: Identity,
    body: &str,
) -> MessagePostResult {
    store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement,
            actor: author,
            acting_for: None,
            text: MessageText::try_from(body.to_owned()).unwrap(),
            references: MessageReferences::try_from(Vec::new()).unwrap(),
        })
        .await
        .unwrap()
}

fn session_reader(value: &str) -> Identity {
    Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from("550e8400-e29b-41d4-a716-446655440000".to_owned())
                    .unwrap(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned()).unwrap(),
            },
            session_id: SessionId::try_from(value.to_owned()).unwrap(),
        },
    }
}

fn actor(value: &str) -> Identity {
    Identity::Human {
        human_id: HumanId::try_from(value.to_owned()).unwrap(),
    }
}

fn name(value: &str) -> ResourceName {
    ResourceName::try_from(value.to_owned()).unwrap()
}

fn description(value: &str) -> Description {
    Description::try_from(value.to_owned()).unwrap()
}
