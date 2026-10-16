//! Real local Runtime/client/store fixture; the existing PeerFixture owns its own lifetime.
use crate::{
    owned_resources::OwnedMatrixResources, peer_fixture::PeerFixture, proof_context::ProofResult,
};
use automation_storage::AutomationStore;
use codex_router_host::{CollaborationRuntime, CollaborationRuntimeInputs};
use collaboration_client::{
    CollaborationClient,
    board::{
        BoardCreateRequest, BoardId, Description, HumanId, Identity, MessageId, MessageReferences,
        MessageText, ParticipantRole, ProjectCreateRequest, ProjectId, ResourceName,
        SubscriptionScope, ThreadCreateRequest, ThreadJoinRequest, TopicCreateRequest, TopicId,
    },
    protocol::{
        AutomationConfigureRequest, DestinationPreparation, EndpointId, EndpointRef,
        InstructionCreateParams, InstructionId, OperationId, ScheduleCreateRequest,
        ScheduleEnableRequest, ScheduleId, SchedulePrepareRequest, SessionRef, WakeSendRequest,
        WakeState, WakeupId,
    },
};
use message_board_storage::BoardStore;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::{
    io,
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimePrefix {
    FirstReader,
    WakeReceipt,
    BeforePrepare,
    EnabledSchedule,
    UnavailableApi,
}

pub(super) struct OwnedClientResources {
    pub(super) client: CollaborationClient,
    pub(super) resources: OwnedMatrixResources,
}
#[derive(Clone)]
pub(super) struct ActorRootExpectation {
    pub(super) actor: Identity,
    pub(super) root_message_id: MessageId,
}
#[derive(Clone, Default)]
pub(super) struct ExpectedResources {
    pub(super) pairs: Vec<ActorRootExpectation>,
    pub(super) wake: Option<(OperationId, WakeupId)>,
    pub(super) instruction: Option<InstructionId>,
    pub(super) schedule: Option<(OperationId, ScheduleId)>,
}

pub(super) struct RuntimeCleanupFixture {
    pub(super) directory: PathBuf,
    pub(super) owned: Option<OwnedClientResources>,
    pub(super) board: Option<BoardStore>,
    pub(super) automation: Option<AutomationStore>,
    pub(super) expected: ExpectedResources,
    runtime: Option<CollaborationRuntime>,
    peer: Option<PeerFixture>,
    registry: PathBuf,
}

impl RuntimeCleanupFixture {
    /// The caller retains the private TempDir until all teardown and reopen assertions finish.
    pub(super) fn allocate(directory: &Path) -> ProofResult<Self> {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        let registry = directory.join("peer-registry");
        std::fs::DirBuilder::new().mode(0o700).create(&registry)?;
        std::fs::write(
            registry.join("unrelated-sentinel"),
            b"preserve this fixture sentinel",
        )?;
        Ok(Self {
            directory: directory.to_owned(),
            registry,
            owned: None,
            runtime: None,
            peer: None,
            board: None,
            automation: None,
            expected: ExpectedResources::default(),
        })
    }

    pub(super) async fn start(&mut self) -> ProofResult<()> {
        self.runtime = Some(
            CollaborationRuntime::start(CollaborationRuntimeInputs {
                directory: self.directory.clone(),
                codex_home: self.directory.clone(),
                backend_socket: self.directory.join("absent-native.sock"),
                mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                native_schema: None,
                peer_registry_directory: Some(self.registry.clone()),
                remote_control_server_name: None,
                owner_human_id: None,
            })
            .await?,
        );
        let service_id = self
            .runtime
            .as_ref()
            .ok_or("Runtime was not retained")?
            .service_id()
            .clone();
        let client =
            CollaborationClient::connect(&self.directory, "runtime-cleanup-test", "1").await?;
        self.owned = Some(OwnedClientResources {
            client,
            resources: OwnedMatrixResources::default(),
        });
        let endpoint = EndpointRef {
            service_id,
            endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
        };
        // Coupling: the frozen L2 draft's pub(super) constructor exposure must be integrated first.
        self.peer = Some(PeerFixture::start_in_registry(&self.registry, &endpoint).await?);
        self.board = Some(BoardStore::open(&self.directory.join("project-board.sqlite")).await?);
        self.automation =
            Some(AutomationStore::open(&self.directory.join("automation.sqlite")).await?);
        Ok(())
    }

    pub(super) async fn build_prefix(&mut self, prefix: RuntimePrefix) -> ProofResult<()> {
        self.create_actor_root_pairs(if prefix == RuntimePrefix::FirstReader {
            1
        } else {
            2
        })
        .await?;
        if prefix == RuntimePrefix::FirstReader {
            return Ok(());
        }
        self.create_wake().await?;
        if prefix == RuntimePrefix::WakeReceipt {
            return Ok(());
        }
        self.create_schedule().await?;
        if prefix == RuntimePrefix::EnabledSchedule {
            self.prepare_and_enable().await?;
        }
        Ok(())
    }

    async fn create_actor_root_pairs(&mut self, root_count: usize) -> ProofResult<()> {
        let reader: Identity = serde_json::from_value(json!({"kind":"session",
            "session":self.peer.as_ref().ok_or("peer was not retained")?.target}))?;
        let sender = Identity::Human {
            human_id: HumanId::try_from("runtime-cleanup-sender".to_owned())?,
        };
        let owned = self
            .owned
            .as_mut()
            .ok_or("actual cleanup client was not retained")?;
        let project_id = ProjectId::generate();
        owned
            .client
            .board_project_create(ProjectCreateRequest {
                project_id: project_id.clone(),
                name: ResourceName::try_from("Runtime cleanup".to_owned())?,
                description: Description::try_from(String::new())?,
                actor: sender.clone(),
                acting_for: None,
            })
            .await?;
        for root_index in 0..root_count {
            // Actor+board cooldown remains real; cleanup-only roots use separate private boards.
            let board_id = BoardId::generate();
            owned
                .client
                .board_create(BoardCreateRequest {
                    board_id: board_id.clone(),
                    project_id: project_id.clone(),
                    name: ResourceName::try_from(format!("Cleanup board {root_index}"))?,
                    description: Description::try_from(String::new())?,
                    actor: sender.clone(),
                    acting_for: None,
                })
                .await?;
            let topic_id = TopicId::generate();
            owned
                .client
                .board_topic_create(TopicCreateRequest {
                    topic_id: topic_id.clone(),
                    board_id,
                    name: ResourceName::try_from("Cleanup topic".to_owned())?,
                    description: Description::try_from(String::new())?,
                    actor: sender.clone(),
                    acting_for: None,
                })
                .await?;
            let root_message_id = MessageId::generate();
            owned
                .resources
                .register_actor_root(sender.clone(), root_message_id.clone())?;
            self.expected.pairs.push(ActorRootExpectation {
                actor: sender.clone(),
                root_message_id: root_message_id.clone(),
            });
            let created = owned
                .client
                .board_thread_create(ThreadCreateRequest {
                    message_id: root_message_id.clone(),
                    topic_id,
                    actor: sender.clone(),
                    acting_for: None,
                    text: MessageText::try_from("Synthetic cleanup root".to_owned())?,
                    references: MessageReferences::try_from(Vec::new())?,
                    role: Some(ParticipantRole::Orchestrator),
                    watch: true,
                })
                .await?;
            ensure(
                created.message.message_id == root_message_id,
                "thread create changed actual root identity",
            )?;
            owned
                .resources
                .register_actor_root(reader.clone(), root_message_id.clone())?;
            self.expected.pairs.push(ActorRootExpectation {
                actor: reader.clone(),
                root_message_id: root_message_id.clone(),
            });
            owned
                .client
                .board_thread_join(ThreadJoinRequest {
                    mode: None,
                    when_idle: None,
                    root_message_id,
                    actor: reader.clone(),
                    role: ParticipantRole::Participant,
                    watch: true,
                    replace: None,
                    note: None,
                })
                .await?;
        }
        Ok(())
    }

    async fn create_wake(&mut self) -> ProofResult<()> {
        let target = self
            .peer
            .as_ref()
            .ok_or("peer was not retained")?
            .target
            .clone();
        let owned = self
            .owned
            .as_mut()
            .ok_or("actual cleanup client was not retained")?;
        let operation_id = OperationId::generate();
        owned
            .resources
            .record_wake_submission(operation_id.clone())?;
        let request: WakeSendRequest = serde_json::from_value(json!({
            "operationId":operation_id,"message":{"target":target,
                "content":{"kind":"humanUser","text":"Synthetic Runtime cleanup wake"},
                "delivery":"auto","generationGuard":null},
            "timing":{"kind":"after","seconds":3600},"expiry":{"kind":"none"}
        }))?;
        let created = owned.client.send_wakeup(request).await?;
        owned
            .resources
            .record_wake_receipt(&operation_id, created.definition.wakeup_id.clone())?;
        self.expected.wake = Some((operation_id, created.definition.wakeup_id));
        ensure(
            created.state == WakeState::Active
                && created.next_due_at.is_some()
                && created.first_fire.is_none(),
            "real wake was not active and unfired before cleanup",
        )
    }

    async fn create_schedule(&mut self) -> ProofResult<()> {
        let owned = self
            .owned
            .as_mut()
            .ok_or("actual cleanup client was not retained")?;
        let instruction_operation = OperationId::generate();
        owned
            .resources
            .record_instruction_submission(instruction_operation.clone())?;
        let instruction = owned
            .client
            .create_instruction(InstructionCreateParams {
                operation_id: instruction_operation.clone(),
                text: "Synthetic Runtime schedule".to_owned().try_into()?,
            })
            .await?;
        owned.resources.record_instruction_receipt(
            &instruction_operation,
            instruction.instruction_id.clone(),
        )?;
        self.expected.instruction = Some(instruction.instruction_id.clone());
        let operation_id = OperationId::generate();
        owned
            .resources
            .record_schedule_submission(operation_id.clone())?;
        let request: ScheduleCreateRequest = serde_json::from_value(json!({
            "operationId":operation_id,"definition":{"instructionId":instruction.instruction_id,
                "timing":{"kind":"interval","seconds":3600},"enabled":false,
                "destination":{"kind":"unprepared"},"executionTimeoutSeconds":120,
                "model":"fixture-model","effort":"medium"}
        }))?;
        let created = owned.client.create_schedule(request).await?;
        owned
            .resources
            .record_schedule_receipt(&operation_id, created.schedule_id.clone())?;
        self.expected.schedule = Some((operation_id, created.schedule_id));
        ensure(
            !created.definition.enabled && created.next_due_at.is_none(),
            "real schedule was not disabled before preparation",
        )
    }

    async fn prepare_and_enable(&self) -> ProofResult<()> {
        let owned = self
            .owned
            .as_ref()
            .ok_or("actual cleanup client was not retained")?;
        let (_, schedule_id) = self
            .expected
            .schedule
            .as_ref()
            .ok_or("actual schedule receipt missing")?;
        let target = self
            .peer
            .as_ref()
            .ok_or("peer was not retained")?
            .target
            .clone();
        owned
            .client
            .configure_automation(AutomationConfigureRequest {
                operation_id: OperationId::generate(),
                execution_timeout_seconds: 120.try_into()?,
                summary_timeout_seconds: 30.try_into()?,
            })
            .await?;
        owned
            .client
            .prepare_schedule(SchedulePrepareRequest {
                operation_id: OperationId::generate(),
                schedule_id: schedule_id.clone(),
                destination: DestinationPreparation::Existing {
                    target,
                    cwd: self.directory.display().to_string(),
                },
            })
            .await?;
        let enabled = owned
            .client
            .enable_schedule(ScheduleEnableRequest {
                operation_id: OperationId::generate(),
                schedule_id: schedule_id.clone(),
            })
            .await?;
        ensure(
            enabled.schedule_id == *schedule_id
                && enabled.definition.enabled
                && enabled.next_due_at.is_some(),
            "real peer schedule did not become enabled before cleanup",
        )
    }

    pub(super) fn peer_target(&self) -> ProofResult<SessionRef> {
        Ok(self
            .peer
            .as_ref()
            .ok_or("peer was not retained")?
            .target
            .clone())
    }

    /// Observe actual installed rows only; producer-retained ownership remains authoritative.
    pub(super) async fn observe_actual_helper_pairs(
        &mut self,
        actors: &[Identity; 2],
    ) -> ProofResult<()> {
        ensure(
            self.expected.pairs.is_empty(),
            "actual helper row expectations already observed",
        )?;
        let board = self
            .board
            .as_mut()
            .ok_or("fixture-owned BoardStore missing")?;
        for actor in actors {
            let records = board
                .list_reader_subscriptions(actor, chrono::Utc::now())
                .await?;
            for record in records {
                let SubscriptionScope::Thread { root_message_id } = record.scope() else {
                    return Err("actual helper unexpectedly installed non-thread scope".into());
                };
                if !self
                    .expected
                    .pairs
                    .iter()
                    .any(|pair| pair.actor == *actor && pair.root_message_id == *root_message_id)
                {
                    self.expected.pairs.push(ActorRootExpectation {
                        actor: actor.clone(),
                        root_message_id: root_message_id.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) async fn stop_runtime(&mut self) -> ProofResult<()> {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await?;
        }
        Ok(())
    }

    pub(super) async fn finish(mut self) -> Vec<FixtureCleanupIssue> {
        let mut issues = Vec::new();
        if let Err(error) = self.stop_runtime().await {
            issues.push(FixtureCleanupIssue {
                stage: CleanupStage::Runtime,
                error,
            });
        }
        if let Some(peer) = self.peer.take()
            && let Err(error) = peer.shutdown().await
        {
            issues.push(FixtureCleanupIssue {
                stage: CleanupStage::Peer,
                error,
            });
        }
        let socket = self.registry.join("peer.sock");
        let process_id = std::process::id();
        let digest = Sha256::digest(socket.to_string_lossy().as_bytes());
        for path in [
            socket,
            self.registry.join(format!("{process_id}.json")),
            self.registry.join(format!("{process_id}.{digest:x}.key")),
        ] {
            let absent = match std::fs::symlink_metadata(path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
                Ok(_) => Err("peer artifact survived awaited shutdown".into()),
            };
            if let Err(error) = absent {
                issues.push(FixtureCleanupIssue {
                    stage: CleanupStage::Artifacts,
                    error,
                });
            }
        }
        let sentinel = std::fs::read(self.registry.join("unrelated-sentinel"));
        if !matches!(sentinel.as_deref(), Ok(b"preserve this fixture sentinel")) {
            issues.push(FixtureCleanupIssue {
                stage: CleanupStage::Artifacts,
                error: "unrelated sentinel changed".into(),
            });
        }
        if let Some(board) = self.board.take()
            && let Err(error) = board.close().await
        {
            issues.push(FixtureCleanupIssue {
                stage: CleanupStage::BoardStore,
                error: error.into(),
            });
        }
        if let Some(automation) = self.automation.take()
            && let Err(error) = automation.close().await
        {
            issues.push(FixtureCleanupIssue {
                stage: CleanupStage::AutomationStore,
                error: error.into(),
            });
        }
        issues
    }
}

#[derive(Debug)]
pub(super) enum CleanupStage {
    Runtime,
    Peer,
    Artifacts,
    BoardStore,
    AutomationStore,
}
#[derive(Debug)]
pub(super) struct FixtureCleanupIssue {
    pub(super) stage: CleanupStage,
    pub(super) error: Box<dyn std::error::Error + Send + Sync>,
}
pub(super) fn ensure(condition: bool, message: &'static str) -> ProofResult<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}
