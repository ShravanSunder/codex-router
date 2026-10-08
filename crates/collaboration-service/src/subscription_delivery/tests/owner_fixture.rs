#[path = "scripted_delivery.rs"]
mod scripted_delivery;
use super::super::*;
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
    SessionMessageDelivery, TargetPresence, TargetPresenceProbe,
};
use chrono::{DateTime, Utc};
use collaboration_protocol::{DeliveryOutcome, DeliveryReceipt, SessionRef};
use message_board::*;
use message_board_storage::BoardStore;
use scripted_delivery::{PreparedRecordingRoute, ScriptedDelivery};
use std::sync::{Arc, Mutex as StdMutex};
use tempfile::TempDir;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::Instant;

pub struct OwnerFixture {
    pub directory: TempDir,
    pub store: Arc<Mutex<BoardStore>>,
    pub push_store: Arc<Mutex<automation_storage::AutomationStore>>,
    pub reader: Identity,
    pub topic: TopicId,
    pub root: MessageId,
    pub clock: Arc<TestClock>,
    pub presence: Arc<ScriptedPresence>,
}

impl OwnerFixture {
    pub async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut store = BoardStore::open(&directory.path().join("board.sqlite"))
            .await
            .unwrap();
        let push_store =
            automation_storage::AutomationStore::open(&directory.path().join("automation.sqlite"))
                .await
                .unwrap();
        let clock = Arc::new(TestClock::new(
            DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).unwrap(),
        ));
        let owner = human("board-owner");
        let project_id = ProjectId::generate();
        let board_id = BoardId::generate();
        let topic = TopicId::generate();
        store
            .create_project(ProjectCreateRequest {
                project_id: project_id.clone(),
                name: name("Owner project"),
                description: description(),
                actor: owner.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_board(BoardCreateRequest {
                board_id: board_id.clone(),
                project_id,
                name: name("Owner board"),
                description: description(),
                actor: owner.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_topic(TopicCreateRequest {
                topic_id: topic.clone(),
                board_id,
                name: name("Owner topic"),
                description: description(),
                actor: owner.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        let root = post_message(
            &mut store,
            Placement::Topic {
                topic_id: topic.clone(),
            },
            owner,
            "Root",
            clock.now(),
        )
        .await
        .message
        .message_id;
        let reader = reader_identity();
        store
            .join_thread(
                ThreadJoinRequest {
                    mode: None,
                    when_idle: None,
                    root_message_id: root.clone(),
                    actor: reader.clone(),
                    role: ParticipantRole::Participant,
                    watch: true,
                    replace: None,
                    note: None,
                },
                clock.now(),
            )
            .await
            .unwrap();
        let fixture = Self {
            directory,
            store: Arc::new(Mutex::new(store)),
            push_store: Arc::new(Mutex::new(push_store)),
            reader,
            topic,
            root,
            clock,
            presence: Arc::new(ScriptedPresence(StdMutex::new(TargetPresence::Running))),
        };
        fixture
            .policy(
                &fixture.root,
                SubscriptionMode::Deliver,
                WhenIdle::Hold,
                0,
                0,
            )
            .await;
        fixture
    }

    pub async fn policy(
        &self,
        root: &MessageId,
        mode: SubscriptionMode,
        idle: WhenIdle,
        quiet: u64,
        cap: u64,
    ) {
        self.store
            .lock()
            .await
            .subscribe_thread_subscription(
                ThreadSubscriptionSubscribeRequest {
                    reader: self.reader.clone(),
                    scope: SubscriptionScope::thread(root.clone()),
                    policy: SubscriptionPolicyPatch {
                        mode: Some(mode),
                        when_idle: Some(idle),
                        timing: SubscriptionTimingPatch {
                            quiet_seconds: Some(quiet),
                            cap_seconds: Some(cap),
                        },
                        lifetime_seconds: None,
                    },
                },
                self.clock.now(),
            )
            .await
            .unwrap();
    }

    pub async fn post(&self, root: &MessageId, body: &str) -> MessagePostResult {
        self.post_at(root, body, self.clock.now()).await
    }
    pub async fn post_at(
        &self,
        root: &MessageId,
        body: &str,
        now: DateTime<Utc>,
    ) -> MessagePostResult {
        post_message(
            &mut *self.store.lock().await,
            Placement::Thread {
                root_message_id: root.clone(),
            },
            human("writer"),
            body,
            now,
        )
        .await
    }

    pub async fn add_root(&self) -> MessageId {
        let mut store = self.store.lock().await;
        let root = post_message(
            &mut store,
            Placement::Topic {
                topic_id: self.topic.clone(),
            },
            human(&format!("root-author-{}", MessageId::generate().as_str())),
            "Other root",
            self.clock.now(),
        )
        .await
        .message
        .message_id;
        store
            .join_thread(
                ThreadJoinRequest {
                    mode: None,
                    when_idle: None,
                    root_message_id: root.clone(),
                    actor: self.reader.clone(),
                    role: ParticipantRole::Participant,
                    watch: true,
                    replace: None,
                    note: None,
                },
                self.clock.now(),
            )
            .await
            .unwrap();
        drop(store);
        self.policy(&root, SubscriptionMode::Deliver, WhenIdle::Hold, 0, 0)
            .await;
        root
    }

    pub async fn runtime(&self) -> OwnerRuntime {
        self.runtime_with_clock(self.clock.clone()).await
    }
    pub async fn runtime_with_clock(&self, clock: Arc<dyn SubscriptionClock>) -> OwnerRuntime {
        self.runtime_with_route(
            clock,
            collaboration_protocol::SessionReachability::CodexAppServer,
        )
        .await
    }
    pub async fn queued_runtime(&self) -> OwnerRuntime {
        self.runtime_with_route(
            self.clock.clone(),
            collaboration_protocol::SessionReachability::ProviderAcp,
        )
        .await
    }
    async fn runtime_with_route(
        &self,
        clock: Arc<dyn SubscriptionClock>,
        reachability: collaboration_protocol::SessionReachability,
    ) -> OwnerRuntime {
        self.runtime_with_availability(
            clock,
            reachability,
            BoardAvailability::Available(Arc::clone(&self.store)),
        )
        .await
    }
    pub async fn runtime_without_board(&self) -> OwnerRuntime {
        self.runtime_with_availability(
            self.clock.clone(),
            collaboration_protocol::SessionReachability::CodexAppServer,
            BoardAvailability::Unavailable,
        )
        .await
    }
    async fn runtime_with_availability(
        &self,
        clock: Arc<dyn SubscriptionClock>,
        reachability: collaboration_protocol::SessionReachability,
        board_availability: BoardAvailability,
    ) -> OwnerRuntime {
        let (requests, receiver) = mpsc::unbounded_channel();
        let (completions, completion_receiver) = mpsc::unbounded_channel();
        let (reconciliations, reconciliation_receiver) = mpsc::unbounded_channel();
        let delivery = Arc::new(ScriptedDelivery {
            requests,
            completions: Mutex::new(completion_receiver),
            reconciliations,
        });
        let route: Arc<dyn crate::SessionDeliveryRoute> = Arc::new(PreparedRecordingRoute {
            delivery,
            reachability,
        });
        let router = Arc::new(crate::SessionDeliveryRouter::new(vec![route]));
        let service = SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
            board_availability,
            push_store: Arc::clone(&self.push_store),
            delivery: router,
            presence: self.presence.clone(),
            clock,
            machine_identity: crate::MachineIdentity::new(
                collaboration_protocol::UuidIdentity::try_from(SERVICE_ID.to_owned()).unwrap(),
                Some("Owner-test-machine"),
            )
            .unwrap(),
        });
        let observations = service.observe_owners();
        service.start().await.unwrap();
        OwnerRuntime {
            service,
            requests: Mutex::new(receiver),
            completions,
            observations: Mutex::new(observations),
            reconciliations: Mutex::new(reconciliation_receiver),
        }
    }

    pub async fn unread_count(&self) -> usize {
        self.store
            .lock()
            .await
            .fetch_inbox(InboxFetchRequest {
                scope: InboxScope::Topic {
                    topic_id: self.topic.clone(),
                },
                reader: self.reader.clone(),
                read_mode: InboxReadMode::Unread,
                page: PageRequest::default(),
            })
            .await
            .unwrap()
            .page
            .records
            .len()
    }
}

pub struct OwnerRuntime {
    pub service: SubscriptionDeliveryService,
    pub requests: Mutex<mpsc::UnboundedReceiver<crate::layer_zero::DeliveryRequest>>,
    pub completions: mpsc::UnboundedSender<DeliveryOutcome>,
    pub observations: Mutex<
        tokio::sync::broadcast::Receiver<super::super::subscription_service::OwnerObservation>,
    >,
    pub reconciliations: Mutex<
        mpsc::UnboundedReceiver<(
            AttemptReconciliationContext,
            oneshot::Sender<AttemptReconciliation>,
        )>,
    >,
}
impl OwnerRuntime {
    pub async fn await_settled(&self) {
        self.completions.send(DeliveryOutcome::Started).unwrap();
        self.observe(|event| {
            matches!(
                event,
                super::super::subscription_service::OwnerObservation::Settled
            )
        })
        .await;
    }
    pub async fn observe(
        &self,
        matches: impl Fn(&super::super::subscription_service::OwnerObservation) -> bool,
    ) {
        let mut receiver = self.observations.lock().await;
        loop {
            let event = receiver.recv().await.unwrap();
            if matches(&event) {
                return;
            }
        }
    }
    pub async fn synchronize(&self, reader: &Identity) {
        self.service.synchronize_reader(reader.clone()).await;
    }
    pub async fn hold_reader_at_storage_boundary(
        &self,
        reader: &Identity,
    ) -> super::super::subscription_service::ReaderStorageHold {
        self.service
            .hold_reader_at_storage_boundary(reader.clone())
            .await
    }

    pub async fn reconcile(&self, reader: &Identity) {
        self.service.request_reconcile(reader.clone()).await;
    }
    pub async fn close(self) {
        self.service.shutdown().await;
    }
}

pub struct ElapsedClock {
    pub wall_start: DateTime<Utc>,
    pub monotonic_start: Instant,
}
impl SubscriptionClock for ElapsedClock {
    fn now(&self) -> DateTime<Utc> {
        self.wall_start
            + chrono::Duration::from_std(Instant::now().duration_since(self.monotonic_start))
                .unwrap()
    }
    fn monotonic_now(&self) -> Instant {
        Instant::now()
    }
}

pub struct ScriptedPresence(pub StdMutex<TargetPresence>);
impl TargetPresenceProbe for ScriptedPresence {
    fn presence(&self, _: &SessionRef) -> DeliveryFuture<'_, TargetPresence> {
        let presence = self.0.lock().unwrap().clone();
        Box::pin(async move { Ok(presence) })
    }
}

pub struct TestClock {
    wall: StdMutex<DateTime<Utc>>,
    monotonic: StdMutex<Instant>,
    advanced: tokio::sync::Notify,
}
impl TestClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            wall: StdMutex::new(now),
            monotonic: StdMutex::new(Instant::now()),
            advanced: tokio::sync::Notify::new(),
        }
    }
    fn advance_wall_and_monotonic(&self, seconds: u64) {
        *self.wall.lock().unwrap() += chrono::Duration::seconds(i64::try_from(seconds).unwrap());
        *self.monotonic.lock().unwrap() += std::time::Duration::from_secs(seconds);
    }

    pub async fn advance(&self, seconds: u64) {
        self.advance_wall_and_monotonic(seconds);
        tokio::time::advance(std::time::Duration::from_secs(seconds)).await;
        self.advanced.notify_waiters();
    }

    pub fn advance_without_tokio_time(&self, seconds: u64) {
        self.advance_wall_and_monotonic(seconds);
        self.advanced.notify_waiters();
    }
}
impl SubscriptionClock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.wall.lock().unwrap()
    }
    fn monotonic_now(&self) -> Instant {
        *self.monotonic.lock().unwrap()
    }
    fn sleep_until(
        &self,
        deadline: Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            loop {
                let advanced = self.advanced.notified();
                tokio::pin!(advanced);
                advanced.as_mut().enable();
                if self.monotonic_now() >= deadline {
                    return;
                }
                advanced.await;
            }
        })
    }
}

pub const SERVICE_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
pub fn reader_identity() -> Identity {
    Identity::Session {
        session: message_board::SessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from(SERVICE_ID.to_owned()).unwrap(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned()).unwrap(),
            },
            session_id: SessionId::try_from("subscription-reader".to_owned()).unwrap(),
        },
    }
}
pub fn human(value: &str) -> Identity {
    Identity::Human {
        human_id: HumanId::try_from(value.to_owned()).unwrap(),
    }
}
fn name(value: &str) -> ResourceName {
    ResourceName::try_from(value.to_owned()).unwrap()
}
fn description() -> Description {
    Description::try_from("Owner integration proof".to_owned()).unwrap()
}
async fn post_message(
    store: &mut BoardStore,
    placement: Placement,
    actor: Identity,
    body: &str,
    now: DateTime<Utc>,
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
            now,
        )
        .await
        .unwrap()
}
