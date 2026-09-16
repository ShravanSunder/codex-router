#![allow(clippy::unwrap_used)]
use super::*;
use std::path::PathBuf;
use tokio::sync::Mutex as TokioMutex;

struct ListenFixture {
    path: PathBuf,
    store: Arc<Mutex<BoardStore>>,
    reader: Identity,
    root_message_id: MessageId,
}

impl ListenFixture {
    async fn create(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "thread-listen-service-{label}-{}.sqlite",
            MessageId::generate().as_str()
        ));
        let mut store = BoardStore::open(&path).await.unwrap();
        let owner = actor("owner");
        let project_id = ProjectId::generate();
        let board_id = BoardId::generate();
        let topic_id = TopicId::generate();
        store
            .create_project(ProjectCreateRequest {
                project_id: project_id.clone(),
                name: name("Listen project"),
                description: description("Service proof"),
                actor: owner.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_board(BoardCreateRequest {
                board_id: board_id.clone(),
                project_id,
                name: name("Listen board"),
                description: description("Service proof"),
                actor: owner.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_topic(TopicCreateRequest {
                topic_id: topic_id.clone(),
                board_id,
                name: name("Listen topic"),
                description: description("Service proof"),
                actor: owner.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        let root = post(&mut store, Placement::Topic { topic_id }, owner, "Root").await;
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
            store: Arc::new(Mutex::new(store)),
            reader,
            root_message_id: root.message.message_id,
        }
    }

    async fn register(
        &self,
        registry: &ThreadListenRegistry,
        mode: ThreadListenMode,
    ) -> ThreadListenSnapshot {
        let request = ThreadListenRequest {
            reader: self.reader.clone(),
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![self.root_message_id.clone()],
            },
            mode,
            from_activity_sequence: None,
            acknowledge: false,
            delivery: ThreadListenDelivery::Stdout,
        };
        let context = self
            .store
            .lock()
            .await
            .prepare_thread_listen(&request)
            .await
            .unwrap();
        registry.register(&request, context).await.unwrap()
    }

    async fn reply(&self, body: &str) {
        let mut store = self.store.lock().await;
        post(
            &mut store,
            Placement::Thread {
                root_message_id: self.root_message_id.clone(),
            },
            actor("writer"),
            body,
        )
        .await;
    }

    async fn finish(self) {
        Arc::try_unwrap(self.store)
            .ok()
            .unwrap()
            .into_inner()
            .close()
            .await
            .unwrap();
        std::fs::remove_file(self.path).unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn debounce_coalesces_a_burst_into_one_batch_set() {
    let fixture = ListenFixture::create("debounce").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    fixture.reply("First").await;
    let waiting_registry = registry.clone();
    let waiting_store = Arc::clone(&fixture.store);
    let listen_id = listen.listen_id.clone();
    let wait = tokio::spawn(async move {
        waiting_registry
            .wait(&listen_id, &waiting_store, usize::MAX)
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(20)).await;
    fixture.reply("Second").await;
    tokio::task::yield_now().await;
    tokio::time::advance(THREAD_LISTEN_DEBOUNCE - std::time::Duration::from_secs(1)).await;
    assert!(!wait.is_finished());
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let result = wait.await.unwrap().unwrap();
    assert_eq!(result.batch_set.unwrap().batches[0].messages.len(), 2);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn debounce_cap_emits_during_continuous_activity() {
    let fixture = ListenFixture::create("cap").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    fixture.reply("Initial").await;
    let waiting_registry = registry.clone();
    let waiting_store = Arc::clone(&fixture.store);
    let listen_id = listen.listen_id.clone();
    let wait = tokio::spawn(async move {
        waiting_registry
            .wait(&listen_id, &waiting_store, usize::MAX)
            .await
    });
    tokio::task::yield_now().await;
    for index in 1..=4 {
        tokio::time::advance(std::time::Duration::from_secs(4 * 60)).await;
        fixture.reply(&format!("Activity {index}")).await;
        tokio::task::yield_now().await;
    }
    tokio::time::advance(std::time::Duration::from_secs(4 * 60)).await;
    let result = wait.await.unwrap().unwrap();
    assert_eq!(result.batch_set.unwrap().batches[0].messages.len(), 5);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn timeout_advances_no_delivered_position() {
    let fixture = ListenFixture::create("timeout").await;
    let registry = ThreadListenRegistry::new();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Once {
                max_wait_seconds: 10,
            },
        )
        .await;
    let waiting_registry = registry.clone();
    let waiting_store = Arc::clone(&fixture.store);
    let listen_id = listen.listen_id.clone();
    let wait = tokio::spawn(async move {
        waiting_registry
            .wait(&listen_id, &waiting_store, usize::MAX)
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    let result = wait.await.unwrap().unwrap();
    assert_eq!(result.end.unwrap().reason, ThreadListenEndReason::Timeout);

    fixture.reply("After timeout").await;
    let next = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    let next_state = registry.state(&next.listen_id).await.unwrap();
    let batch = fixture
        .store
        .lock()
        .await
        .select_thread_listen_batch_set(next.listen_id, &next_state.context, usize::MAX)
        .await
        .unwrap();
    assert_eq!(batch.batches[0].messages.len(), 1);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn cancel_ends_a_repeating_listen() {
    let fixture = ListenFixture::create("cancel").await;
    let registry = ThreadListenRegistry::new();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    let waiting_registry = registry.clone();
    let waiting_store = Arc::clone(&fixture.store);
    let listen_id = listen.listen_id.clone();
    let wait = tokio::spawn(async move {
        waiting_registry
            .wait(&listen_id, &waiting_store, usize::MAX)
            .await
    });
    tokio::task::yield_now().await;
    assert!(registry.show(&listen.listen_id).await.unwrap().active);
    let cancelled = registry.cancel(&listen.listen_id).await.unwrap();
    assert!(!cancelled.active);
    let result = wait.await.unwrap().unwrap();
    assert_eq!(result.end.unwrap().reason, ThreadListenEndReason::Cancelled);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn cancel_without_a_wait_releases_the_listen_registration() {
    let fixture = ListenFixture::create("cancel-idle").await;
    let registry = ThreadListenRegistry::new();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    registry.cancel(&listen.listen_id).await.unwrap();
    tokio::task::yield_now().await;
    assert!(registry.state(&listen.listen_id).await.is_err());
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn dropped_wait_releases_the_listen_registration() {
    let fixture = ListenFixture::create("drop-wait").await;
    let registry = ThreadListenRegistry::new();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    let waiting_registry = registry.clone();
    let waiting_store = Arc::clone(&fixture.store);
    let listen_id = listen.listen_id.clone();
    let wait = tokio::spawn(async move {
        waiting_registry
            .wait(&listen_id, &waiting_store, usize::MAX)
            .await
    });
    tokio::task::yield_now().await;
    wait.abort();
    assert!(wait.await.unwrap_err().is_cancelled());
    tokio::task::yield_now().await;
    assert!(registry.state(&listen.listen_id).await.is_err());
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn repeating_lifetime_returns_lifetime_without_a_batch() {
    let fixture = ListenFixture::create("lifetime").await;
    let registry = ThreadListenRegistry::new();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: 10,
            },
        )
        .await;
    let waiting_registry = registry.clone();
    let waiting_store = Arc::clone(&fixture.store);
    let listen_id = listen.listen_id;
    let wait = tokio::spawn(async move {
        waiting_registry
            .wait(&listen_id, &waiting_store, usize::MAX)
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    let result = wait.await.unwrap().unwrap();
    assert!(result.batch_set.is_none());
    assert_eq!(result.end.unwrap().reason, ThreadListenEndReason::Lifetime);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn repeating_lifetime_is_preserved_between_waits_after_a_batch() {
    let fixture = ListenFixture::create("lifetime-between-waits").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    let listen_state = registry.state(&listen.listen_id).await.unwrap();
    fixture.reply("Batch").await;
    let waiting_registry = registry.clone();
    let waiting_store = Arc::clone(&fixture.store);
    let waiting_listen_id = listen.listen_id.clone();
    let first_wait = tokio::spawn(async move {
        waiting_registry
            .wait(&waiting_listen_id, &waiting_store, usize::MAX)
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(THREAD_LISTEN_DEBOUNCE).await;
    let first_result = first_wait.await.unwrap().unwrap();
    assert!(first_result.batch_set.is_some(), "{first_result:?}");

    registry
        .preserve_terminal(&listen.listen_id, &listen_state)
        .await;
    let terminal = registry
        .wait(&listen.listen_id, &fixture.store, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        terminal.end.unwrap().reason,
        ThreadListenEndReason::Lifetime
    );
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn repeating_cancel_is_preserved_between_waits_after_a_batch() {
    let fixture = ListenFixture::create("cancel-between-waits").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    let listen_state = registry.state(&listen.listen_id).await.unwrap();
    fixture.reply("Batch").await;
    let waiting_registry = registry.clone();
    let waiting_store = Arc::clone(&fixture.store);
    let waiting_listen_id = listen.listen_id.clone();
    let first_wait = tokio::spawn(async move {
        waiting_registry
            .wait(&waiting_listen_id, &waiting_store, usize::MAX)
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(THREAD_LISTEN_DEBOUNCE).await;
    let first_result = first_wait.await.unwrap().unwrap();
    assert!(first_result.batch_set.is_some(), "{first_result:?}");

    registry.cancel(&listen.listen_id).await.unwrap();
    registry
        .preserve_terminal(&listen.listen_id, &listen_state)
        .await;
    let terminal = registry
        .wait(&listen.listen_id, &fixture.store, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        terminal.end.unwrap().reason,
        ThreadListenEndReason::Cancelled
    );
    fixture.finish().await;
}

#[tokio::test]
async fn listen_admission_is_bounded_and_cancel_releases_capacity() {
    let fixture = ListenFixture::create("capacity").await;
    let registry = ThreadListenRegistry::new();
    let mode = ThreadListenMode::Repeating {
        lifetime_seconds: 300,
    };
    let mut listens = Vec::new();
    for _ in 0..MAX_ACTIVE_THREAD_LISTENS {
        listens.push(fixture.register(&registry, mode.clone()).await);
    }
    let request = ThreadListenRequest {
        reader: fixture.reader.clone(),
        selection: ThreadListenSelection::Roots {
            root_message_ids: vec![fixture.root_message_id.clone()],
        },
        mode: mode.clone(),
        from_activity_sequence: None,
        acknowledge: false,
        delivery: ThreadListenDelivery::Stdout,
    };
    let context = fixture
        .store
        .lock()
        .await
        .prepare_thread_listen(&request)
        .await
        .unwrap();
    assert!(registry.register(&request, context).await.is_err());
    registry.cancel(&listens[0].listen_id).await.unwrap();
    tokio::task::yield_now().await;
    fixture.register(&registry, mode).await;
    for listen in listens.into_iter().skip(1) {
        registry.cancel(&listen.listen_id).await.unwrap();
    }
    fixture.finish().await;
}

#[tokio::test]
async fn terminal_listen_outcomes_remain_bounded() {
    let fixture = ListenFixture::create("terminal-capacity").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let mut first_listen_id = None;
    for _ in 0..=MAX_TERMINAL_THREAD_LISTENS {
        let listen = fixture
            .register(
                &registry,
                ThreadListenMode::Repeating {
                    lifetime_seconds: 300,
                },
            )
            .await;
        first_listen_id.get_or_insert_with(|| listen.listen_id.clone());
        let state = registry.state(&listen.listen_id).await.unwrap();
        registry.preserve_terminal(&listen.listen_id, &state).await;
    }
    let entries = registry.entries.lock().await;
    assert_eq!(entries.active.len(), 0);
    assert_eq!(entries.terminal.len(), MAX_TERMINAL_THREAD_LISTENS);
    assert!(
        !entries
            .terminal
            .contains_key(first_listen_id.as_ref().unwrap())
    );
    drop(entries);
    fixture.finish().await;
}

async fn post(
    store: &mut BoardStore,
    placement: Placement,
    actor: Identity,
    body: &str,
) -> MessagePostResult {
    store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement,
            actor,
            acting_for: None,
            text: MessageText::try_from(body.to_owned()).unwrap(),
            references: MessageReferences::try_from(Vec::new()).unwrap(),
        })
        .await
        .unwrap()
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

#[derive(Clone)]
struct RecordingSink {
    records: Arc<TokioMutex<Vec<ListenDeliveryRecord>>>,
    reject_batches: bool,
}

impl BatchSink for RecordingSink {
    fn deliver<'a>(
        &'a self,
        record: ListenDeliveryRecord,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BatchSinkFailure>> + Send + 'a>,
    > {
        Box::pin(async move {
            if self.reject_batches && matches!(record, ListenDeliveryRecord::Batch(_)) {
                return Err(BatchSinkFailure::Rejected {
                    evidence: serde_json::json!({"kind":"nativeRejected","reason":"busy"}),
                });
            }
            self.records.lock().await.push(record);
            Ok(())
        })
    }
}

#[tokio::test(start_paused = true)]
async fn long_session_delivery_heartbeats_only_at_silent_marks_then_finalizes() {
    let fixture = ListenFixture::create("session-heartbeat").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    let records = Arc::new(TokioMutex::new(Vec::new()));
    registry.spawn_session_delivery(
        listen.listen_id,
        Arc::clone(&fixture.store),
        Arc::new(RecordingSink {
            records: Arc::clone(&records),
            reject_batches: false,
        }),
    );
    tokio::task::yield_now().await;
    for _ in 0..3 {
        tokio::time::advance(THREAD_LISTEN_MARK).await;
        tokio::task::yield_now().await;
    }
    let records = records.lock().await;
    assert_eq!(records.len(), 3);
    assert!(matches!(&records[0], ListenDeliveryRecord::Heartbeat(value) if value.mark == 1));
    assert!(matches!(&records[1], ListenDeliveryRecord::Heartbeat(value) if value.mark == 2));
    assert!(
        matches!(&records[2], ListenDeliveryRecord::Finalization(value) if value.reason == ThreadListenEndReason::Lifetime)
    );
    drop(records);
    fixture.finish().await;
}

#[test]
fn three_consecutive_session_rejections_end_with_error_evidence() {
    let mut count = 0_u8;
    let mut evidence = serde_json::Value::Null;
    assert!(!record_rejection(
        &mut count,
        &mut evidence,
        serde_json::json!({"reason":"busy"})
    ));
    assert!(!record_rejection(
        &mut count,
        &mut evidence,
        serde_json::json!({"reason":"busy"})
    ));
    assert!(record_rejection(
        &mut count,
        &mut evidence,
        serde_json::json!({"reason":"childThread"})
    ));
    assert_eq!(count, 3);
    assert_eq!(evidence["reason"], "childThread");
}
