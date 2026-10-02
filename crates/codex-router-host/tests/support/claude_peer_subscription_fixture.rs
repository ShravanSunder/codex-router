//! Real Board/automation storage and authenticated Claude peer composition for resume proof.
#[path = "claude_peer_subscription_clock.rs"]
mod observed_clock;
use automation_storage::AutomationStore;
use chrono::{DateTime, Utc};
use claude_code_peer_messaging::{
    ClaudeCodePeerSocket, ClaudeCodeSessionRegistry, PeerSessionLookup, PeerSessionRecord,
};
use codex_router_host::ClaudeCodePeerDeliveryRoute;
use collaboration_protocol::{
    EndpointId as PeerEndpointId, EndpointRef, PushId, PushRecord, RouterOriginRef, SessionRef,
    UuidIdentity,
};
use collaboration_service::{
    BoardAvailability, MachineIdentity, SessionDeliveryRouter, SessionMessageDelivery,
    SubscriptionClock, SubscriptionDeliveryService, SubscriptionDeliveryServiceProps,
    TargetPresence, TargetPresenceProbe,
};
use message_board::{
    BoardCreateRequest, BoardId, Description, Identity, InboxFetchRequest, InboxPage,
    InboxReadMode, InboxScope, Message, MessageId, MessagePostRequest, MessageReferences,
    MessageText, ParticipantRole, Placement, ProjectCreateRequest, ProjectId, ResourceName,
    ServiceId, SessionEndpointRef, SessionId as BoardSessionId, SessionRef as BoardSessionRef,
    SubscriptionMode, SubscriptionPolicyPatch, SubscriptionScope, SubscriptionTimingPatch,
    ThreadCreateRequest, ThreadJoinRequest, ThreadSubscriptionRecord, TopicCreateRequest, TopicId,
    WhenIdle,
};
use message_board_storage::BoardStore;
use observed_clock::ManualSubscriptionClock;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    error::Error,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    net::UnixListener,
    sync::mpsc::UnboundedReceiver,
    task::JoinHandle,
    time::Instant,
};

pub type ProofResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

pub const SESSION_ID: &str = "claude-subscription-reader";
pub const ROOT_THREAD_TEXT: &str = "Claude peer resume proof";
pub const POSTED_MESSAGE_TEXT: &str = "A reply arrives while Claude is closed";
pub const PEER_TOKEN: &str = "0123456789abcdef0123456789abcdef";
pub const PRESENCE_RECHECK: Duration = Duration::from_secs(30);
pub const OWNER_EVENT_TIMEOUT: Duration = Duration::from_secs(5);

const SERVICE_ID: &str = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
const ENDPOINT_ID: &str = "claude-local";

pub struct PeerMessageFrames {
    pub authentication: Value,
    pub user: Value,
    pub extra_frame: Option<String>,
}

pub struct ClaudePeerSubscriptionFixture {
    _directory: tempfile::TempDir,
    registry_directory: PathBuf,
    socket_path: PathBuf,
    target: SessionRef,
    reader: Identity,
    writer: Identity,
    root_message_id: MessageId,
    project_id: ProjectId,
    clock: Arc<ManualSubscriptionClock>,
    owner_sleep_deadlines: UnboundedReceiver<Instant>,
    peer_registry: Arc<ClaudeCodeSessionRegistry>,
    peer_route: Arc<ClaudeCodePeerDeliveryRoute>,
    router: Arc<SessionDeliveryRouter>,
    service: Option<SubscriptionDeliveryService>,
    board_store: Arc<tokio::sync::Mutex<BoardStore>>,
    automation_store: Arc<tokio::sync::Mutex<AutomationStore>>,
}

impl ClaudePeerSubscriptionFixture {
    pub async fn start() -> ProofResult<Self> {
        let directory = tempfile::tempdir()?;
        let board_path = directory.path().join("board.sqlite");
        let automation_path = directory.path().join("automation.sqlite");
        let registry_directory = directory.path().join("claude-sessions");
        std::fs::create_dir(&registry_directory)?;

        let (clock, owner_sleep_deadlines) = ManualSubscriptionClock::new(Utc::now());
        let clock = Arc::new(clock);
        let board_store = Arc::new(tokio::sync::Mutex::new(
            BoardStore::open(&board_path).await?,
        ));
        let automation_store = Arc::new(tokio::sync::Mutex::new(
            AutomationStore::open(&automation_path).await?,
        ));
        let target = peer_target()?;
        let reader = board_reader_identity()?;
        let writer = human_identity("peer-resume-test-writer")?;
        let root_message_id = MessageId::generate();
        let project_id = create_board_thread_and_subscription(
            &board_store,
            &reader,
            &writer,
            &root_message_id,
            clock.now(),
        )
        .await?;

        let peer_registry = Arc::new(ClaudeCodeSessionRegistry::new(registry_directory.clone()));
        let peer_route = Arc::new(ClaudeCodePeerDeliveryRoute::new(
            target.endpoint.clone(),
            Arc::clone(&peer_registry),
            Arc::new(ClaudeCodePeerSocket::new(registry_directory.clone())),
        ));
        let router = Arc::new(SessionDeliveryRouter::new(vec![peer_route.clone()]));
        let delivery: Arc<dyn SessionMessageDelivery> = router.clone();
        let presence: Arc<dyn TargetPresenceProbe> = router.clone();
        let service = SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
            board_availability: BoardAvailability::Available(Arc::clone(&board_store)),
            push_store: Arc::clone(&automation_store),
            delivery,
            presence,
            machine_identity: MachineIdentity::new(
                target.endpoint.service_id.clone(),
                Some("claude-peer-resume-fixture"),
            )?,
            clock: clock.clone(),
        });
        service.start().await?;

        Ok(Self {
            socket_path: registry_directory.join("peer.sock"),
            _directory: directory,
            registry_directory,
            target,
            reader,
            writer,
            root_message_id,
            project_id,
            clock,
            owner_sleep_deadlines,
            peer_registry,
            peer_route,
            router,
            service: Some(service),
            board_store,
            automation_store,
        })
    }

    pub fn target(&self) -> &SessionRef {
        &self.target
    }

    pub fn root_message_id(&self) -> &MessageId {
        &self.root_message_id
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn current_monotonic(&self) -> ProofResult<Instant> {
        self.clock.current_monotonic()
    }

    pub fn advance(&self, amount: Duration) -> ProofResult {
        self.clock.advance(amount)
    }

    pub async fn next_owner_sleep(
        &mut self,
        stage: &'static str,
        exact_remaining: Option<Duration>,
    ) -> ProofResult<Instant> {
        ManualSubscriptionClock::next_sleep_deadline_matching(
            &self.clock,
            &mut self.owner_sleep_deadlines,
            stage,
            exact_remaining,
        )
        .await
    }

    pub async fn presence(&self) -> ProofResult<TargetPresence> {
        Ok(self.router.presence(&self.target).await?)
    }

    pub fn lookup_peer(&self) -> ProofResult<PeerSessionLookup> {
        Ok(self.peer_registry.lookup(&self.target.session_id)?)
    }

    pub fn publish_writable_peer(&self) -> ProofResult<(Arc<UnixListener>, PeerSessionRecord)> {
        let listener = Arc::new(UnixListener::bind(&self.socket_path)?);
        let process_id = publish_peer_registry_record(&self.registry_directory, &self.socket_path)?;
        match self.peer_registry.lookup(&self.target.session_id)? {
            PeerSessionLookup::Writable(peer)
                if u32::from(peer.process_id) == process_id
                    && peer.session_id == self.target.session_id
                    && peer.socket_path == self.socket_path =>
            {
                Ok((listener, peer))
            }
            _ => Err(std::io::Error::other(
                "published Claude peer did not become writable in the real registry",
            )
            .into()),
        }
    }

    pub fn receive_one_peer_batch(
        listener: Arc<UnixListener>,
    ) -> JoinHandle<ProofResult<PeerMessageFrames>> {
        tokio::spawn(async move {
            let (stream, _) = tokio::time::timeout(OWNER_EVENT_TIMEOUT, listener.accept())
                .await
                .map_err(|_| std::io::Error::other("Claude peer did not accept the notice"))??;
            let mut lines = BufReader::new(stream).lines();
            let authentication: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await?
                    .ok_or_else(|| std::io::Error::other("Claude peer auth frame is missing"))?,
            )?;
            let user: Value =
                serde_json::from_str(&lines.next_line().await?.ok_or_else(|| {
                    std::io::Error::other("Claude peer user Batch frame is missing")
                })?)?;
            let extra_frame = lines.next_line().await?;
            Ok(PeerMessageFrames {
                authentication,
                user,
                extra_frame,
            })
        })
    }

    pub async fn initialize_unread_inbox(&self) -> ProofResult<InboxPage> {
        self.fetch_project_inbox().await
    }

    pub async fn unread_project_inbox(&self) -> ProofResult<InboxPage> {
        self.fetch_project_inbox().await
    }

    async fn fetch_project_inbox(&self) -> ProofResult<InboxPage> {
        Ok(self
            .board_store
            .lock()
            .await
            .fetch_inbox(InboxFetchRequest {
                scope: InboxScope::Project {
                    project_id: self.project_id.clone(),
                },
                reader: self.reader.clone(),
                read_mode: InboxReadMode::Unread,
                page: message_board::PageRequest::default(),
            })
            .await?
            .page)
    }

    pub async fn post_activity(&self) -> ProofResult<Message> {
        Ok(self
            .board_store
            .lock()
            .await
            .post_message(
                MessagePostRequest {
                    message_id: MessageId::generate(),
                    placement: Placement::Thread {
                        root_message_id: self.root_message_id.clone(),
                    },
                    actor: self.writer.clone(),
                    acting_for: None,
                    text: MessageText::try_from(POSTED_MESSAGE_TEXT.to_owned())?,
                    references: MessageReferences::try_from(Vec::new())?,
                },
                self.clock.now(),
            )
            .await?
            .message)
    }

    pub async fn subscription(&self) -> ProofResult<ThreadSubscriptionRecord> {
        self.board_store
            .lock()
            .await
            .get_thread_subscription_record(
                &self.reader,
                &SubscriptionScope::thread(self.root_message_id.clone()),
            )
            .await?
            .ok_or_else(|| std::io::Error::other("thread subscription was not persisted").into())
    }

    pub async fn pending_push_records(
        &self,
    ) -> ProofResult<Vec<collaboration_protocol::PushRecord>> {
        Ok(self
            .automation_store
            .lock()
            .await
            .list_pending_push_records(&self.target)
            .await?)
    }

    pub async fn push_record(&self, push_id: &PushId) -> ProofResult<Option<PushRecord>> {
        Ok(self
            .automation_store
            .lock()
            .await
            .get_push_record(push_id)
            .await?)
    }

    pub async fn push_record_by_origin_reference(
        &self,
        origin: &RouterOriginRef,
    ) -> ProofResult<Option<PushRecord>> {
        Ok(self
            .automation_store
            .lock()
            .await
            .get_push_record_by_origin_reference(origin)
            .await?)
    }

    pub async fn due_subscription_roots(&self) -> ProofResult<Vec<MessageId>> {
        Ok(self
            .board_store
            .lock()
            .await
            .due_subscription_roots(&self.reader, self.clock.now())
            .await?)
    }

    pub async fn reconcile_reader(&self) -> ProofResult {
        self.service
            .as_ref()
            .ok_or_else(|| std::io::Error::other("delivery service is already shut down"))?
            .reconcile_reader(self.reader.clone())
            .await?;
        Ok(())
    }

    pub async fn shutdown(mut self) -> ProofResult {
        if let Some(service) = self.service.take() {
            service.shutdown().await;
            drop(service);
        }
        drop(self.router);
        drop(self.peer_route);
        let automation_store = Arc::try_unwrap(self.automation_store)
            .map_err(|_| std::io::Error::other("delivery owner retained AutomationStore"))?
            .into_inner();
        automation_store.close().await?;
        let board_store = Arc::try_unwrap(self.board_store)
            .map_err(|_| std::io::Error::other("delivery owner retained BoardStore"))?
            .into_inner();
        board_store.close().await?;
        Ok(())
    }
}

fn peer_target() -> ProofResult<SessionRef> {
    Ok(SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from(SERVICE_ID.to_owned())?,
            endpoint_id: PeerEndpointId::try_from(ENDPOINT_ID.to_owned())?,
        },
        session_id: collaboration_protocol::SessionId::try_from(SESSION_ID.to_owned())?,
    })
}

fn board_reader_identity() -> ProofResult<Identity> {
    Ok(Identity::Session {
        session: BoardSessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from(SERVICE_ID.to_owned())?,
                endpoint_id: message_board::EndpointId::try_from(ENDPOINT_ID.to_owned())?,
            },
            session_id: BoardSessionId::try_from(SESSION_ID.to_owned())?,
        },
    })
}

fn human_identity(name: &str) -> ProofResult<Identity> {
    Ok(Identity::Human {
        human_id: name.to_owned().try_into()?,
    })
}

fn publish_peer_registry_record(registry_directory: &Path, socket_path: &Path) -> ProofResult<u32> {
    let process_id = std::process::id();
    std::fs::write(
        registry_directory.join(format!("{process_id}.json")),
        json!({
            "pid": process_id,
            "sessionId": SESSION_ID,
            "status": "idle",
            "peerProtocol": 1,
            "messagingSocketPath": socket_path,
        })
        .to_string(),
    )?;
    let socket_text = socket_path
        .to_str()
        .ok_or_else(|| std::io::Error::other("peer socket path is not UTF-8"))?;
    let digest = Sha256::digest(socket_text.as_bytes());
    let key_path = registry_directory.join(format!("{process_id}.{digest:x}.key"));
    std::fs::write(&key_path, json!({"peerToken": PEER_TOKEN}).to_string())?;
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    Ok(process_id)
}

async fn create_board_thread_and_subscription(
    store: &Arc<tokio::sync::Mutex<BoardStore>>,
    reader: &Identity,
    writer: &Identity,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> ProofResult<ProjectId> {
    let project_id = ProjectId::generate();
    let board_id = BoardId::generate();
    let topic_id = TopicId::generate();
    {
        let mut board = store.lock().await;
        board
            .create_project(ProjectCreateRequest {
                project_id: project_id.clone(),
                name: ResourceName::try_from("Peer resume proof project".to_owned())?,
                description: Description::try_from(String::new())?,
                actor: writer.clone(),
                acting_for: None,
            })
            .await?;
        board
            .create_board(BoardCreateRequest {
                board_id: board_id.clone(),
                project_id: project_id.clone(),
                name: ResourceName::try_from("Peer resume proof board".to_owned())?,
                description: Description::try_from(String::new())?,
                actor: writer.clone(),
                acting_for: None,
            })
            .await?;
        board
            .create_topic(TopicCreateRequest {
                topic_id: topic_id.clone(),
                board_id,
                name: ResourceName::try_from("Resume proof topic".to_owned())?,
                description: Description::try_from(String::new())?,
                actor: writer.clone(),
                acting_for: None,
            })
            .await?;
        board
            .create_thread(
                ThreadCreateRequest {
                    message_id: root_message_id.clone(),
                    topic_id,
                    actor: writer.clone(),
                    acting_for: None,
                    text: MessageText::try_from(ROOT_THREAD_TEXT.to_owned())?,
                    references: MessageReferences::try_from(Vec::new())?,
                    role: None,
                    watch: false,
                },
                now,
            )
            .await?;
        board
            .join_thread(
                ThreadJoinRequest {
                    root_message_id: root_message_id.clone(),
                    actor: reader.clone(),
                    role: ParticipantRole::Participant,
                    watch: true,
                    mode: Some(SubscriptionMode::Deliver),
                    when_idle: Some(WhenIdle::Hold),
                    replace: None,
                    note: None,
                },
                now,
            )
            .await?;
        board
            .subscribe_thread_subscription(
                message_board::ThreadSubscriptionSubscribeRequest {
                    reader: reader.clone(),
                    scope: SubscriptionScope::thread(root_message_id.clone()),
                    policy: SubscriptionPolicyPatch {
                        mode: Some(SubscriptionMode::Deliver),
                        when_idle: Some(WhenIdle::Hold),
                        timing: SubscriptionTimingPatch {
                            quiet_seconds: Some(0),
                            cap_seconds: Some(0),
                        },
                        ..SubscriptionPolicyPatch::default()
                    },
                },
                now,
            )
            .await?;
    }
    Ok(project_id)
}
