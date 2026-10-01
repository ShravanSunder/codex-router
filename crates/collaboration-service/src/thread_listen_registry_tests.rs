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
        self.register_with_delivery(registry, mode, ThreadListenDelivery::Stdout)
            .await
    }

    async fn register_session_delivery(
        &self,
        registry: &ThreadListenRegistry,
        mode: ThreadListenMode,
    ) -> ThreadListenSnapshot {
        self.register_with_delivery(registry, mode, ThreadListenDelivery::Session)
            .await
    }

    async fn register_with_delivery(
        &self,
        registry: &ThreadListenRegistry,
        mode: ThreadListenMode,
        delivery: ThreadListenDelivery,
    ) -> ThreadListenSnapshot {
        let request = ThreadListenRequest {
            reader: self.reader.clone(),
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![self.root_message_id.clone()],
            },
            mode,
            from_activity_sequence: None,
            acknowledge: false,
            delivery,
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
        for _ in 0..1024 {
            if Arc::strong_count(&self.store) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
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
async fn catch_up_follows_the_first_batch_set_on_the_set_and_the_finalization() {
    // Arrange: activity lands before the listen is armed, so the stored
    // Delivered position precedes the armed sequence.
    let fixture = ListenFixture::create("catch-up").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    fixture.reply("Older than the listen").await;
    let listen = fixture
        .register(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    assert!(!listen.catch_up, "arming alone proves no catch-up");

    // Act.
    let waiting_registry = registry.clone();
    let waiting_store = Arc::clone(&fixture.store);
    let listen_id = listen.listen_id.clone();
    let wait = tokio::spawn(async move {
        waiting_registry
            .wait(&listen_id, &waiting_store, usize::MAX)
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(THREAD_LISTEN_DEBOUNCE).await;
    let result = wait.await.unwrap().unwrap();

    // Assert: the Batch set and the listen state that feeds every finalization agree.
    assert!(result.batch_set.as_ref().unwrap().catch_up);
    assert!(
        registry.show(&listen.listen_id).await.unwrap().catch_up,
        "the finalization must report the delivered set, not the --from proxy"
    );
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
        .post_message(
            MessagePostRequest {
                message_id: MessageId::generate(),
                placement,
                actor,
                acting_for: None,
                text: MessageText::try_from(body.to_owned()).unwrap(),
                references: MessageReferences::try_from(Vec::new()).unwrap(),
            },
            chrono::Utc::now(),
        )
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
    /// Rejects this many records before accepting anything, for reset proofs.
    rejections_remaining: Arc<std::sync::atomic::AtomicU8>,
    batch_recorded: Arc<tokio::sync::Notify>,
    batch_rejected: Arc<tokio::sync::Notify>,
    first_batch_rejection_release: Option<Arc<tokio::sync::Notify>>,
    heartbeat_recorded: Arc<tokio::sync::Notify>,
    heartbeat_rejected: Arc<tokio::sync::Notify>,
}

impl RecordingSink {
    fn accepting(records: &Arc<TokioMutex<Vec<ListenDeliveryRecord>>>) -> Arc<Self> {
        Arc::new(Self {
            records: Arc::clone(records),
            reject_batches: false,
            rejections_remaining: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            batch_recorded: Arc::new(tokio::sync::Notify::new()),
            batch_rejected: Arc::new(tokio::sync::Notify::new()),
            first_batch_rejection_release: None,
            heartbeat_recorded: Arc::new(tokio::sync::Notify::new()),
            heartbeat_rejected: Arc::new(tokio::sync::Notify::new()),
        })
    }
}

impl BatchSink for RecordingSink {
    fn deliver<'a>(
        &'a self,
        record: ListenDeliveryRecord,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BatchSinkFailure>> + Send + 'a>,
    > {
        Box::pin(async move {
            // A finalization is always recorded: refusing it would hide the very
            // outcome these proofs read.
            let remaining = self.rejections_remaining.load(Ordering::Relaxed);
            if !matches!(record, ListenDeliveryRecord::Finalization(_))
                && ((self.reject_batches && matches!(record, ListenDeliveryRecord::Batch(_)))
                    || remaining > 0)
            {
                self.rejections_remaining
                    .store(remaining.saturating_sub(1), Ordering::Relaxed);
                if matches!(record, ListenDeliveryRecord::Batch(_)) {
                    self.batch_rejected.notify_one();
                    if remaining == u8::MAX
                        && let Some(release) = &self.first_batch_rejection_release
                    {
                        release.notified().await;
                    }
                }
                if matches!(record, ListenDeliveryRecord::Heartbeat(_)) {
                    self.heartbeat_rejected.notify_one();
                }
                return Err(BatchSinkFailure::Rejected {
                    evidence: serde_json::json!({"kind":"nativeRejected","reason":"busy"}),
                });
            }
            let is_batch = matches!(record, ListenDeliveryRecord::Batch(_));
            let is_heartbeat = matches!(record, ListenDeliveryRecord::Heartbeat(_));
            self.records.lock().await.push(record);
            if is_batch {
                self.batch_recorded.notify_one();
            }
            if is_heartbeat {
                self.heartbeat_recorded.notify_one();
            }
            Ok(())
        })
    }
}

#[tokio::test(start_paused = true)]
async fn long_session_delivery_heartbeats_only_at_silent_marks_then_finalizes() {
    let fixture = ListenFixture::create("session-heartbeat").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register_session_delivery(
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
        RecordingSink::accepting(&records),
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

fn describe_record(record: &ListenDeliveryRecord) -> String {
    match record {
        ListenDeliveryRecord::Batch(value) => format!("batch(catch_up={})", value.catch_up),
        ListenDeliveryRecord::Heartbeat(value) => format!("heartbeat({})", value.mark),
        ListenDeliveryRecord::Finalization(value) => format!("final({:?})", value.reason),
    }
}

/// Drives a spawned session delivery under paused time until it has produced
/// the record a proof reads, or until the simulated minutes run out.
///
/// Two things paused time cannot do on its own. Auto-advance races task startup,
/// so a single long sleep can jump the clock past the delivery task's first poll
/// and leave its debounce window opening after the observation window closed.
/// Storage calls behind a Batch set complete on a real background thread.
/// Settle those calls and await accepted Batch progress before virtual time
/// advances; cancellation exposes a failed delivery instead of parking here.
async fn drive_session_delivery(
    store: &Arc<Mutex<BoardStore>>,
    records: &Arc<TokioMutex<Vec<ListenDeliveryRecord>>>,
    minutes: u64,
    batch_progress: Option<(&tokio::sync::Notify, &tokio_util::sync::CancellationToken)>,
    settled: impl Fn(&[ListenDeliveryRecord]) -> bool,
) {
    let mut observed_batch_progress = false;
    settle_storage_work(store).await;
    for _ in 0..minutes {
        let seen = records.lock().await;
        let settled_now = settled(&seen);
        let accepted_batch_seen = seen
            .iter()
            .any(|record| matches!(record, ListenDeliveryRecord::Batch(_)));
        drop(seen);
        if accepted_batch_seen && !observed_batch_progress {
            if let Some((progress, cancellation)) = batch_progress {
                tokio::select! {
                    biased;
                    () = progress.notified() => {}
                    () = cancellation.cancelled() => return,
                }
            }
            observed_batch_progress = true;
        }
        if settled_now {
            // Finalization is published before the detached delivery task drops
            // its last store reference. Let that task finish so fixture cleanup
            // observes ownership rather than racing publication.
            settle_storage_work(store).await;
            return;
        }
        tokio::time::advance(std::time::Duration::from_secs(60)).await;
        settle_storage_work(store).await;
    }
}

/// Waits in real time until no task is inside a storage call.
///
/// Every storage call holds the store lock until its SQLx worker replies, which
/// can take milliseconds when SQLite syncs to disk. A fixed number of yields is
/// not enough to cover that on every host. This task stays runnable while it
/// waits, so the paused clock cannot auto-advance, and a free lock after a round
/// of yields means the delivery tasks are parked on timers or channels rather
/// than on storage.
async fn settle_storage_work(store: &Arc<Mutex<BoardStore>>) {
    let real_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        if store.try_lock().is_ok() {
            return;
        }
        assert!(
            std::time::Instant::now() < real_deadline,
            "storage work did not settle within 10 real seconds"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn an_accepted_record_resets_the_rejection_counter_and_keeps_the_listen() {
    // Arrange: a sink that rejects twice, then accepts.
    let fixture = ListenFixture::create("rejection-reset").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register_session_delivery(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    fixture.reply("Rejected activity").await;
    let records = Arc::new(TokioMutex::new(Vec::new()));
    let state = registry.state(&listen.listen_id).await.unwrap();

    // Act.
    registry.spawn_session_delivery(
        listen.listen_id.clone(),
        Arc::clone(&fixture.store),
        Arc::new(RecordingSink {
            records: Arc::clone(&records),
            reject_batches: false,
            rejections_remaining: Arc::new(std::sync::atomic::AtomicU8::new(2)),
            batch_recorded: Arc::new(tokio::sync::Notify::new()),
            batch_rejected: Arc::new(tokio::sync::Notify::new()),
            first_batch_rejection_release: None,
            heartbeat_recorded: Arc::new(tokio::sync::Notify::new()),
            heartbeat_rejected: Arc::new(tokio::sync::Notify::new()),
        }),
    );
    drive_session_delivery(
        &fixture.store,
        &records,
        90,
        Some((&state.batch_progress_recorded, &state.cancellation)),
        |seen| matches!(seen.last(), Some(ListenDeliveryRecord::Finalization(_))),
    )
    .await;

    // Assert: the listen reached its lifetime instead of failing, and the
    // rejection it survived is still reported.
    let seen = records.lock().await;
    assert!(matches!(
        seen.last(),
        Some(ListenDeliveryRecord::Finalization(value))
            if value.reason == ThreadListenEndReason::Lifetime
                && value.last_rejection.is_some()
    ));
    drop(seen);
    fixture.finish().await;
}

struct RetryOnceSink {
    records: Arc<TokioMutex<Vec<ListenDeliveryRecord>>>,
    attempts: Arc<std::sync::atomic::AtomicU8>,
}

struct HeldAcceptedBatchSink {
    records: Arc<TokioMutex<Vec<ListenDeliveryRecord>>>,
    accepted: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl BatchSink for HeldAcceptedBatchSink {
    fn deliver<'a>(
        &'a self,
        record: ListenDeliveryRecord,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BatchSinkFailure>> + Send + 'a>,
    > {
        Box::pin(async move {
            let is_batch = matches!(record, ListenDeliveryRecord::Batch(_));
            self.records.lock().await.push(record);
            if is_batch {
                self.accepted.notify_one();
                self.release.notified().await;
            }
            Ok(())
        })
    }
}

#[tokio::test(start_paused = true)]
async fn delivery_driver_waits_for_durable_progress_after_sink_accepts_batch() {
    let fixture = ListenFixture::create("held-accepted-batch").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register_session_delivery(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    fixture.reply("Accepted before persistence").await;
    let state = registry.state(&listen.listen_id).await.unwrap();
    let records = Arc::new(TokioMutex::new(Vec::new()));
    let accepted = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let observed = Arc::new(tokio::sync::Notify::new());
    registry.spawn_session_delivery(
        listen.listen_id.clone(),
        Arc::clone(&fixture.store),
        Arc::new(HeldAcceptedBatchSink {
            records: Arc::clone(&records),
            accepted: Arc::clone(&accepted),
            release: Arc::clone(&release),
        }),
    );
    let driver_records = Arc::clone(&records);
    let driver_store = Arc::clone(&fixture.store);
    let driver_observed = Arc::clone(&observed);
    let driver_state = Arc::clone(&state);
    let mut driver = tokio::spawn(async move {
        drive_session_delivery(
            &driver_store,
            &driver_records,
            30,
            Some((
                &driver_state.batch_progress_recorded,
                &driver_state.cancellation,
            )),
            |seen| {
                let has_batch = seen
                    .iter()
                    .any(|record| matches!(record, ListenDeliveryRecord::Batch(_)));
                if has_batch {
                    driver_observed.notify_one();
                }
                has_batch
            },
        )
        .await;
    });
    accepted.notified().await;
    observed.notified().await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1), &mut driver)
            .await
            .is_err(),
        "a sink record must not settle delivery before its SQLite position write"
    );
    assert_eq!(
        registry
            .show(&listen.listen_id)
            .await
            .unwrap()
            .batches_delivered,
        0
    );
    release.notify_one();
    driver
        .await
        .expect("delivery driver should finish after persistence");
    assert_eq!(
        registry
            .show(&listen.listen_id)
            .await
            .unwrap()
            .batches_delivered,
        1
    );
    // The same wait must also end if delivery terminates without another
    // durable Batch, as it does after a position-persistence failure.
    let missing_progress = Arc::new(tokio::sync::Notify::new());
    let failure_observed = Arc::new(tokio::sync::Notify::new());
    let failure_records = Arc::clone(&records);
    let failure_store = Arc::clone(&fixture.store);
    let failure_state = Arc::clone(&state);
    let failure_progress = Arc::clone(&missing_progress);
    let failure_started = Arc::clone(&failure_observed);
    let failure_driver = tokio::spawn(async move {
        drive_session_delivery(
            &failure_store,
            &failure_records,
            5,
            Some((&failure_progress, &failure_state.cancellation)),
            |seen| {
                let has_batch = seen
                    .iter()
                    .any(|record| matches!(record, ListenDeliveryRecord::Batch(_)));
                if has_batch {
                    failure_started.notify_one();
                }
                has_batch
            },
        )
        .await;
    });
    failure_observed.notified().await;
    registry.cancel(&listen.listen_id).await.unwrap();
    failure_driver
        .await
        .expect("cancellation should release a missing-progress wait");
    drive_session_delivery(&fixture.store, &records, 5, None, |seen| {
        matches!(seen.last(), Some(ListenDeliveryRecord::Finalization(_)))
    })
    .await;
    fixture.finish().await;
}

impl BatchSink for RetryOnceSink {
    fn deliver<'a>(
        &'a self,
        record: ListenDeliveryRecord,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BatchSinkFailure>> + Send + 'a>,
    > {
        Box::pin(async move {
            if matches!(record, ListenDeliveryRecord::Batch(_))
                && self.attempts.fetch_add(1, Ordering::Relaxed) == 0
            {
                return Err(BatchSinkFailure::Unavailable);
            }
            self.records.lock().await.push(record);
            Ok(())
        })
    }
}

#[tokio::test(start_paused = true)]
async fn retryable_unavailable_keeps_the_session_batch_pending_until_delivery_succeeds() {
    let fixture = ListenFixture::create("retryable-unavailable").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register_session_delivery(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    assert_eq!(listen.delivery, ThreadListenDelivery::Session);
    fixture.reply("Retry this provider push").await;
    let records = Arc::new(TokioMutex::new(Vec::new()));
    let attempts = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let state = registry.state(&listen.listen_id).await.unwrap();

    registry.spawn_session_delivery(
        listen.listen_id.clone(),
        Arc::clone(&fixture.store),
        Arc::new(RetryOnceSink {
            records: Arc::clone(&records),
            attempts: Arc::clone(&attempts),
        }),
    );
    drive_session_delivery(
        &fixture.store,
        &records,
        30,
        Some((&state.batch_progress_recorded, &state.cancellation)),
        |seen| {
            seen.iter()
                .any(|record| matches!(record, ListenDeliveryRecord::Batch(_)))
        },
    )
    .await;

    let snapshot = registry
        .show(&listen.listen_id)
        .await
        .expect("retrying listen remains active");
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    assert_eq!(snapshot.batches_delivered, 1);
    assert!(records.lock().await.iter().any(|record| matches!(
        record,
        ListenDeliveryRecord::Batch(batch_set)
            if batch_set.batches[0].messages[0].text.as_str() == "Retry this provider push"
    )));
    registry
        .cancel(&listen.listen_id)
        .await
        .expect("cancel listen");
    drive_session_delivery(&fixture.store, &records, 5, None, |seen| {
        matches!(seen.last(), Some(ListenDeliveryRecord::Finalization(_)))
    })
    .await;
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn delivery_inside_a_mark_window_suppresses_that_marks_heartbeat() {
    // Arrange: a long session listen with activity waiting in its first mark window.
    let fixture = ListenFixture::create("mark-suppression").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register_session_delivery(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    fixture.reply("Inside the first mark").await;
    let records = Arc::new(TokioMutex::new(Vec::new()));

    let state = registry.state(&listen.listen_id).await.unwrap();
    let sink = RecordingSink::accepting(&records);
    let delivery_registry = registry.clone();
    let delivery_store = Arc::clone(&fixture.store);
    let delivery_listen_id = listen.listen_id.clone();
    let delivery_sink = sink.clone();
    tokio::time::resume();
    let delivery = tokio::spawn(async move {
        session_delivery::run(
            delivery_registry,
            delivery_listen_id,
            delivery_store,
            delivery_sink,
        )
        .await;
    });
    // Real SQLite work must finish before virtual time reaches the mark.
    state.debounce_armed.notified().await;
    tokio::time::pause();
    tokio::time::advance(THREAD_LISTEN_DEBOUNCE).await;
    tokio::time::resume();
    sink.batch_recorded.notified().await;
    tokio::time::pause();
    // Run past the second mark, which proves the first passed silently.
    tokio::time::advance(
        THREAD_LISTEN_MARK * 2 - THREAD_LISTEN_DEBOUNCE + std::time::Duration::from_secs(1),
    )
    .await;
    sink.heartbeat_recorded.notified().await;

    // Assert: the Batch set stood in for that mark's heartbeat.
    let seen = records.lock().await;
    let kinds: Vec<String> = seen.iter().map(describe_record).collect();
    assert!(
        matches!(seen.first(), Some(ListenDeliveryRecord::Batch(_))),
        "records: {kinds:?}"
    );
    assert!(
        !seen.iter().any(|record| {
            matches!(record, ListenDeliveryRecord::Heartbeat(value) if value.mark == 1)
        }),
        "a mark with a delivery must not also emit a heartbeat"
    );
    drop(seen);

    registry.cancel(&listen.listen_id).await.unwrap();
    tokio::time::resume();
    delivery.await.unwrap();
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn a_third_consecutive_rejection_ends_the_listen_with_its_evidence() {
    // Arrange: a sink that refuses every record it is offered.
    let fixture = ListenFixture::create("rejection-terminal").await;
    let registry = ThreadListenRegistry::new_without_lifecycle_cleanup();
    let listen = fixture
        .register_session_delivery(
            &registry,
            ThreadListenMode::Repeating {
                lifetime_seconds: ThreadListenLifetime::Long.seconds(),
            },
        )
        .await;
    fixture.reply("Refused activity").await;
    let records = Arc::new(TokioMutex::new(Vec::new()));

    // Act: a refused Batch set, then two refused heartbeats.
    let first_batch_rejection_release = Arc::new(tokio::sync::Notify::new());
    let sink = Arc::new(RecordingSink {
        records: Arc::clone(&records),
        reject_batches: false,
        rejections_remaining: Arc::new(std::sync::atomic::AtomicU8::new(u8::MAX)),
        batch_recorded: Arc::new(tokio::sync::Notify::new()),
        batch_rejected: Arc::new(tokio::sync::Notify::new()),
        first_batch_rejection_release: Some(Arc::clone(&first_batch_rejection_release)),
        heartbeat_recorded: Arc::new(tokio::sync::Notify::new()),
        heartbeat_rejected: Arc::new(tokio::sync::Notify::new()),
    });
    let state = registry.state(&listen.listen_id).await.unwrap();
    let delivery_registry = registry.clone();
    let delivery_store = Arc::clone(&fixture.store);
    let delivery_listen_id = listen.listen_id.clone();
    let delivery_sink = sink.clone();
    tokio::time::resume();
    let delivery = tokio::spawn(async move {
        session_delivery::run(
            delivery_registry,
            delivery_listen_id,
            delivery_store,
            delivery_sink,
        )
        .await;
    });
    state.debounce_armed.notified().await;
    tokio::time::pause();
    tokio::time::advance(THREAD_LISTEN_DEBOUNCE).await;
    tokio::time::resume();
    sink.batch_rejected.notified().await;
    let store_guard = fixture.store.lock().await;
    tokio::time::pause();
    first_batch_rejection_release.notify_one();
    tokio::time::advance(THREAD_LISTEN_MARK - THREAD_LISTEN_DEBOUNCE).await;
    sink.heartbeat_rejected.notified().await;
    tokio::time::advance(THREAD_LISTEN_MARK).await;
    sink.heartbeat_rejected.notified().await;
    delivery.await.unwrap();
    assert_eq!(
        sink.rejections_remaining.load(Ordering::Relaxed),
        u8::MAX - 3,
        "one batch and two heartbeat refusals should end this listen"
    );
    assert_eq!(Arc::strong_count(&fixture.store), 1);
    drop(store_guard);

    // Assert: the listen ended on the third refusal, carrying the last evidence.
    let seen = records.lock().await;
    assert!(
        matches!(
            seen.last(),
            Some(ListenDeliveryRecord::Finalization(value))
                if value.reason == ThreadListenEndReason::Error
                    && value.last_rejection.is_some()
        ),
        "three refusals in a row must end the listen with its reason, saw {:?}",
        seen.iter().map(describe_record).collect::<Vec<_>>()
    );
    drop(seen);
    fixture.finish().await;
}
