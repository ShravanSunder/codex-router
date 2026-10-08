//! Host-owned composition of public listeners and lifecycle publication.
use crate::BackendPublication;
use crate::RemoteControlServerName;
use collaboration_protocol::{
    CodexGeneration, EndpointAvailability, EndpointId, EndpointRef, NonEmptyText,
    ObservationTimestamp, ProviderKind, RouterExecutableRelation, SchemaDigest, UuidIdentity,
};
use collaboration_service::{
    NativeRelayListener, ServiceIdentity, load_service_identity, new_service_uuid,
};
use std::{io, net::SocketAddr, path::PathBuf};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub struct CollaborationRuntimeInputs {
    pub owner_human_id: Option<message_board::HumanId>,
    pub directory: PathBuf,
    pub codex_home: PathBuf,
    pub backend_socket: PathBuf,
    pub mcp_bind: std::net::SocketAddr,
    /// An actual executable-bound export, or metadata-only schema admission.
    pub native_schema: Option<std::sync::Arc<codex_native_integration::NativeSchemaExport>>,
    /// Fixture override; normal Host starts read the owner's ~/.claude/sessions.
    pub peer_registry_directory: Option<PathBuf>,
    /// Remote Control machine name observed when this Host started.
    pub remote_control_server_name: Option<RemoteControlServerName>,
}

/// Host-owned startup facts that are not part of the public collaboration runtime inputs.
pub(crate) struct HostCollaborationInputs {
    pub collaboration_runtime: CollaborationRuntimeInputs,
    pub router_proxy_endpoint: SocketAddr,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProviderLaunchBinding {
    pub endpoint_id: EndpointId,
    pub label: NonEmptyText,
    pub provider: ProviderKind,
    pub launch: crate::ExternalProviderLaunch,
}
impl ExternalProviderLaunchBinding {
    pub fn claude(executable: PathBuf, arguments: Vec<String>) -> Result<Self, &'static str> {
        Self::fixed(
            "claude-local",
            "Claude Code",
            ProviderKind::ClaudeCode,
            executable,
            arguments,
        )
    }

    pub fn cursor(executable: PathBuf, arguments: Vec<String>) -> Result<Self, &'static str> {
        Self::fixed(
            "cursor-local",
            "Cursor",
            ProviderKind::Cursor,
            executable,
            arguments,
        )
    }

    fn fixed(
        endpoint_id: &str,
        label: &str,
        provider: ProviderKind,
        executable: PathBuf,
        arguments: Vec<String>,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            endpoint_id: EndpointId::try_from(endpoint_id.to_owned())
                .map_err(|_| "invalid external provider endpoint ID")?,
            label: NonEmptyText::try_from(label.to_owned())?,
            provider,
            launch: crate::ExternalProviderLaunch {
                persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
                executable,
                arguments,
                environment: Vec::new(),
            },
        })
    }

    #[must_use]
    pub const fn provider(&self) -> ProviderKind {
        self.provider
    }

    #[must_use]
    pub const fn command_flags(&self) -> (&'static str, &'static str) {
        match self.provider {
            ProviderKind::ClaudeCode => ("--claude-acp-executable", "--claude-acp-arguments"),
            ProviderKind::Cursor => ("--cursor-acp-executable", "--cursor-acp-arguments"),
        }
    }
}
pub struct BackendSchemaEvidence<'a> {
    pub running_executable: &'a codex_native_integration::ExecutableIdentity,
    pub export: &'a codex_native_integration::NativeSchemaExport,
}
pub struct CollaborationRuntime {
    owner_human_id: Option<message_board::HumanId>,
    board_store: Option<std::sync::Arc<tokio::sync::Mutex<message_board_storage::BoardStore>>>,
    subscription_delivery: Option<collaboration_service::SubscriptionDeliveryService>,
    provider_store:
        Option<std::sync::Arc<tokio::sync::Mutex<collaboration_service::ProviderOperationStore>>>,
    external_provider_supervisor: Option<std::sync::Arc<crate::ExternalProviderSupervisor>>,
    provider_delivery_route: Option<std::sync::Arc<crate::ProviderAcpDeliveryRoute>>,
    provider_retention: Option<tokio::task::JoinHandle<()>>,
    directory: PathBuf,
    /// The native schema bundle the Host started with, published as the manifest's
    /// `nativeSchemaDigest`.
    native_digest: Option<SchemaDigest>,
    payload_cache: crate::native_schema_cache::NativeSchemaCache,
    service_id: UuidIdentity,
    service_epoch: UuidIdentity,
    publication: BackendPublication,
    shutdown: CancellationToken,
    tasks: JoinSet<io::Result<()>>,
    provider_retirement_tasks: JoinSet<io::Result<()>>,
    journal: Option<std::sync::Arc<lifecycle_observation::LifecycleStore>>,
    maintenance: Option<tokio::task::JoinHandle<Result<(), lifecycle_observation::JournalError>>>,
    automation_task: Option<tokio::task::JoinHandle<()>>,
    schedule_task: Option<tokio::task::JoinHandle<()>>,
    automation_maintenance: Option<tokio::task::JoinHandle<()>>,
    current_generation: Option<CodexGeneration>,
    observer_task: Option<tokio::task::JoinHandle<()>>,
    manifest: Option<collaboration_service::ManifestPublication>,
    collaboration_api: ServedCollaborationApi,
}
impl CollaborationRuntime {
    pub async fn configure_provider_operation_retention(
        &mut self,
        retention_days: std::num::NonZeroU32,
    ) -> io::Result<u64> {
        let cutoff_ms = unix_seconds()?
            .saturating_mul(1_000)
            .saturating_sub(i64::from(retention_days.get()).saturating_mul(24 * 60 * 60 * 1_000));
        let protected = self
            .external_provider_supervisor
            .as_ref()
            .map(|supervisor| supervisor.live_operation_ids())
            .unwrap_or_default();
        let Some(store) = &self.provider_store else {
            return Ok(0);
        };
        let pruned = store
            .lock()
            .await
            .prune_terminal_before(cutoff_ms, &protected)
            .await
            .map_err(io::Error::other)?;
        let store = std::sync::Arc::clone(store);
        let supervisor = self.external_provider_supervisor.clone();
        let shutdown = self.shutdown.clone();
        self.provider_retention = Some(tokio::spawn(async move {
            let period = std::time::Duration::from_secs(24 * 60 * 60);
            let mut interval =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    _ = interval.tick() => {
                        let cutoff_ms = chrono::Utc::now().timestamp_millis().saturating_sub(
                            i64::from(retention_days.get()).saturating_mul(24 * 60 * 60 * 1_000),
                        );
                        let protected = supervisor.as_ref().map(|value| value.live_operation_ids()).unwrap_or_default();
                        let _ = store.lock().await.prune_terminal_before(cutoff_ms, &protected).await;
                    }
                }
            }
        }));
        Ok(pruned)
    }
    /// Binds both private listeners before spawning tasks. No native process is started.
    pub async fn start(inputs: CollaborationRuntimeInputs) -> io::Result<Self> {
        Self::start_with_external_providers(inputs, Vec::new()).await
    }

    pub async fn start_with_external_providers(
        inputs: CollaborationRuntimeInputs,
        provider_launches: Vec<crate::ExternalProviderStartup>,
    ) -> io::Result<Self> {
        let (_relation_sender, relation_receiver) =
            tokio::sync::watch::channel(RouterExecutableRelation::Match);
        Self::start_with_external_providers_and_router_relation(
            inputs,
            provider_launches,
            relation_receiver,
        )
        .await
    }

    pub async fn start_with_external_providers_and_router_relation(
        inputs: CollaborationRuntimeInputs,
        provider_launches: Vec<crate::ExternalProviderStartup>,
        relation_receiver: tokio::sync::watch::Receiver<RouterExecutableRelation>,
    ) -> io::Result<Self> {
        Self::start_with_optional_router_proxy_endpoint(
            inputs,
            provider_launches,
            relation_receiver,
            None,
        )
        .await
    }

    pub(crate) async fn start_for_host_with_router_proxy_endpoint(
        host_inputs: HostCollaborationInputs,
        provider_launches: Vec<crate::ExternalProviderStartup>,
        relation_receiver: tokio::sync::watch::Receiver<RouterExecutableRelation>,
    ) -> io::Result<Self> {
        Self::start_with_optional_router_proxy_endpoint(
            host_inputs.collaboration_runtime,
            provider_launches,
            relation_receiver,
            Some(host_inputs.router_proxy_endpoint),
        )
        .await
    }

    async fn start_with_optional_router_proxy_endpoint(
        inputs: CollaborationRuntimeInputs,
        provider_launches: Vec<crate::ExternalProviderStartup>,
        relation_receiver: tokio::sync::watch::Receiver<RouterExecutableRelation>,
        router_proxy_endpoint: Option<SocketAddr>,
    ) -> io::Result<Self> {
        let mut owner_human_id = inputs.owner_human_id.clone();
        let service_id = load_service_identity(&inputs.directory)?;
        let service_epoch = new_service_uuid()?;
        let machine_identity = collaboration_service::MachineIdentity::new(
            service_id.clone(),
            inputs
                .remote_control_server_name
                .as_ref()
                .map(RemoteControlServerName::as_str),
        )
        .map_err(io::Error::other)?;
        let native_digest = if let Some(export) = &inputs.native_schema {
            export
                .bundle()
                .publish(&inputs.directory)
                .map_err(io::Error::other)?;
            let encoded = format!(
                "sha256:{}",
                export
                    .bundle()
                    .digest()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            );
            Some(SchemaDigest::try_from(encoded).map_err(io::Error::other)?)
        } else {
            None
        };
        let native_definitions = inputs.native_schema.as_ref().and_then(|export| {
            collaboration_mcp::NativeSchemaDefinitions::from_bundle(export.bundle())
        });
        let journal = async {
            let database = lifecycle_observation::ObservationJournal::open(
                &inputs.directory.join("session-registry.sqlite"),
                new_service_uuid()?,
            )
            .await
            .map_err(io::Error::other)?;
            let journal = std::sync::Arc::new(lifecycle_observation::LifecycleStore::new(database));
            journal
                .prepare(unix_seconds()?)
                .await
                .map_err(io::Error::other)?;
            Ok::<_, io::Error>(journal)
        }
        .await;
        let journal = match journal {
            Ok(journal) => Some(journal),
            Err(_) => {
                tracing::warn!("lifecycle storage unavailable; collaboration remains independent");
                None
            }
        };
        let mut identity = ServiceIdentity::new(
            &String::from(service_id.clone()),
            &String::from(service_epoch.clone()),
        )
        .map_err(io::Error::other)?
        .with_machine_identity(machine_identity.clone())
        .map_err(io::Error::other)?;
        if let Some(journal) = &journal {
            identity = identity.with_journal(std::sync::Arc::clone(journal));
        }
        let mut board_store = None;
        match message_board_storage::BoardStore::open(
            &inputs.directory.join("project-board.sqlite"),
        )
        .await
        {
            Ok(store) => {
                let store = std::sync::Arc::new(tokio::sync::Mutex::new(store));
                identity = identity.with_board_store(store.clone());
                board_store = Some(store);
            }
            Err(_) => {
                tracing::warn!(
                    "board storage unavailable; other collaboration remains independent"
                );
            }
        }
        let mut automation_store = None;
        let mut settings_backend = None;
        match automation_storage::AutomationStore::open(&inputs.directory.join("automation.sqlite"))
            .await
        {
            Ok(store) => {
                let store = std::sync::Arc::new(tokio::sync::Mutex::new(store));
                automation_store = Some(std::sync::Arc::clone(&store));
                let handle = collaboration_service::AutomationConfigurationHandle::default();
                handle.suspend().await;
                let backend = std::sync::Arc::new(crate::AutomationSettingsFile::new(
                    &inputs.directory,
                    std::sync::Arc::clone(&store),
                    handle.clone(),
                ));
                identity = identity
                    .with_automation_store(store)
                    .with_automation_configuration(handle, backend.clone());
                settings_backend = Some(backend);
            }
            Err(_) => {
                tracing::warn!("automation storage unavailable; collaboration remains independent")
            }
        }
        let provider_store = match collaboration_service::ProviderOperationStore::open(
            &inputs.directory.join("provider-operations.sqlite"),
        )
        .await
        {
            Ok(store) => {
                let store = std::sync::Arc::new(tokio::sync::Mutex::new(store));
                identity = identity.with_provider_operation_store(std::sync::Arc::clone(&store));
                Some(store)
            }
            Err(_) => {
                tracing::warn!(
                    "provider operation storage unavailable; external providers remain disabled"
                );
                None
            }
        };
        let collaboration_api = BoundCollaborationApi::bind(inputs.mcp_bind).await?;
        let mcp_url = collaboration_api.url();
        let startup = crate::provider_startup_composition::compose_provider_startup(
            provider_launches,
            provider_store.clone(),
            &service_id,
            &service_epoch,
            &mcp_url,
        )
        .await?;
        let provider_face_endpoints = startup.endpoints.clone();
        let provider_retirements = startup.retirements;
        let external_provider_supervisor = startup.supervisor;
        let provider_session_hub = startup.hub;
        let provider_endpoints = startup
            .endpoints
            .iter()
            .map(|description| description.endpoint.clone())
            .collect();
        if !startup.endpoints.is_empty() {
            identity = identity
                .with_endpoints(startup.endpoints)
                .map_err(io::Error::other)?;
        }
        if let Some(supervisor) = &external_provider_supervisor {
            let provider_backend: std::sync::Arc<
                dyn collaboration_service::ProviderConversationBackend,
            > = supervisor.clone();
            identity = identity.with_provider_conversation_backend(provider_backend);
        }
        if let Some(hub) = provider_session_hub.clone() {
            identity = identity.with_provider_session_hub(hub);
        }
        let endpoint = EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: EndpointId::try_from("codex-local".to_owned())
                .map_err(io::Error::other)?,
        };
        let publication = BackendPublication::new(
            identity.endpoint_directory(),
            endpoint.clone(),
            service_epoch.clone(),
            inputs.backend_socket,
        )?;
        let native_backend = collaboration_service::NativeControlBackend {
            codex_home: inputs.codex_home.clone(),
            endpoint,
            gate: publication.admission_gate(),
        };
        let approval_broker = collaboration_service::ServiceInteractionBroker::load(
            service_id.clone(),
            native_backend.clone(),
            inputs.directory.join("approval-routes.json"),
        )
        .await
        .map_err(io::Error::other)?;
        let unmaterialized_threads =
            std::sync::Arc::new(collaboration_service::UnmaterializedThreadHolder::new());
        let peer_registry_directory = inputs.peer_registry_directory.clone().map_or_else(
            crate::session_message_route_composition::default_peer_registry_directory,
            Ok,
        )?;
        let identity = identity.with_claude_code_sessions(std::sync::Arc::new(
            claude_code_peer_messaging::ClaudeCodeSessionRegistry::new(
                peer_registry_directory.clone(),
            ),
        ));
        let message_routes =
            crate::session_message_route_composition::compose_session_message_routes(
                crate::session_message_route_composition::SessionMessageRouteInputs {
                    provider_endpoints,
                    directory: identity.endpoint_directory(),
                    native_backend: native_backend.clone(),
                    unmaterialized_threads: std::sync::Arc::clone(&unmaterialized_threads),
                    provider_supervisor: external_provider_supervisor.clone(),
                    provider_store: provider_store.clone(),
                    peer_registry_directory,
                },
            )?;
        let session_delivery: std::sync::Arc<dyn collaboration_service::SessionMessageDelivery> =
            message_routes.router.clone();
        let subscription_presence: std::sync::Arc<dyn collaboration_service::TargetPresenceProbe> =
            message_routes.router.clone();
        let scheduled_run_execution: std::sync::Arc<
            dyn collaboration_service::ScheduledRunExecution,
        > = message_routes.router.clone();
        let provider_delivery_route = message_routes.provider_route;
        let subscription_clock: std::sync::Arc<dyn collaboration_service::SubscriptionClock> =
            std::sync::Arc::new(collaboration_service::SystemSubscriptionClock);
        let subscription_delivery = automation_store.as_ref().map(|push_store| {
            let board_availability = match board_store.as_ref() {
                Some(store) => collaboration_service::BoardAvailability::Available(
                    std::sync::Arc::clone(store),
                ),
                None => collaboration_service::BoardAvailability::Unavailable,
            };
            collaboration_service::SubscriptionDeliveryService::new(
                collaboration_service::SubscriptionDeliveryServiceProps {
                    board_availability,
                    push_store: std::sync::Arc::clone(push_store),
                    delivery: std::sync::Arc::clone(&session_delivery),
                    presence: std::sync::Arc::clone(&subscription_presence),
                    machine_identity: machine_identity.clone(),
                    clock: std::sync::Arc::clone(&subscription_clock),
                },
            )
        });
        if let Some(supervisor) = &external_provider_supervisor {
            supervisor
                .install_approval_broker(std::sync::Arc::clone(&approval_broker))
                .await;
        }
        let identity = identity
            .with_session_delivery(session_delivery)
            .with_scheduled_run_execution(scheduled_run_execution)
            .with_native_backend(native_backend)
            .map_err(io::Error::other)?;
        let identity = if let Some(service) = &subscription_delivery {
            identity.with_subscription_delivery_service(
                service.clone(),
                std::sync::Arc::clone(&subscription_presence),
            )
        } else {
            identity
        };
        let delivery_for_approvals = identity
            .session_message_delivery()
            .ok_or_else(|| io::Error::other("session delivery unavailable"))?;
        approval_broker
            .install_session_delivery(delivery_for_approvals)
            .map_err(io::Error::other)?;
        let identity = identity.with_approval_broker(std::sync::Arc::clone(&approval_broker));
        let endpoint_directory = identity.endpoint_directory();
        let wake_worker = identity.wake_timing_worker();
        let schedule_worker = identity.schedule_timing_worker();
        let retention_worker = identity.automation_retention_worker();
        let codex_recorder = crate::codex_conversation_recording_composition::recording_for_store(
            provider_store.as_ref(),
            &inputs.directory,
            &service_id,
        )?;
        let identity = if let Some(recorder) = &codex_recorder {
            identity.with_codex_conversation_recorder(std::sync::Arc::clone(recorder))
        } else {
            identity
        };
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
        let application = collaboration_service::CollaborationApplication::new(identity);
        let collaboration_api =
            collaboration_api.with_service_socket(&inputs.directory.join("control.sock"))?;
        let native = NativeRelayListener::bind(
            &inputs.directory.join("codex-native.sock"),
            publication.admission_gate(),
        )?
        .with_connection_budget(std::sync::Arc::clone(&permits));
        let stored = std::sync::Arc::new(collaboration_service::NativeStoredSessions::new(
            inputs.codex_home.clone(),
            String::from(service_id.clone()),
        ));
        let mut acp = collaboration_service::AcpChannelListener::bind(
            &inputs.directory.join("codex-acp.sock"),
            publication.admission_gate(),
            stored,
            approval_broker.clone(),
            unmaterialized_threads,
            crate::codex_conversation_recording_composition::adapter_recorder(codex_recorder),
        )?
        .with_connection_budget(permits);
        let mut provider_app_servers = Vec::new();
        if !provider_face_endpoints.is_empty() && external_provider_supervisor.is_some() {
            owner_human_id =
                crate::owner_identity_resolution::resolve_face_owner_human_id(owner_human_id).await;
        }
        if external_provider_supervisor.is_some()
            && provider_delivery_route.is_some()
            && provider_session_hub.is_some()
            && provider_store.is_some()
        {
            acp = acp.with_interaction_broker(std::sync::Arc::clone(&approval_broker));
        }
        if let (Some(supervisor), Some(delivery), Some(hub), Some(store)) = (
            &external_provider_supervisor,
            &provider_delivery_route,
            &provider_session_hub,
            &provider_store,
        ) {
            let commands: std::sync::Arc<dyn collaboration_service::SessionCommandPort> =
                std::sync::Arc::new(crate::HostSessionCommandPort::new(
                    std::sync::Arc::clone(supervisor),
                    std::sync::Arc::clone(delivery),
                    std::sync::Arc::clone(hub),
                    std::sync::Arc::clone(store),
                ));
            let events: std::sync::Arc<dyn collaboration_service::SessionEventHub> =
                std::sync::Arc::clone(hub) as _;
            let socket_directory = inputs.directory.join("router-sessions");
            if owner_human_id.is_some() {
                create_private_socket_directory(&socket_directory)?;
            }
            for description in provider_face_endpoints {
                let Some(catalog) = supervisor.provider_model_catalog(&description.endpoint) else {
                    continue;
                };
                let endpoint = message_board::SessionEndpointRef {
                    service_id: message_board::ServiceId::try_from(String::from(
                        description.endpoint.service_id.clone(),
                    ))
                    .map_err(io::Error::other)?,
                    endpoint_id: message_board::EndpointId::try_from(String::from(
                        description.endpoint.endpoint_id.clone(),
                    ))
                    .map_err(io::Error::other)?,
                };
                acp = acp.with_provider_session_backend(
                    endpoint.clone(),
                    std::sync::Arc::clone(&commands),
                    std::sync::Arc::clone(&events),
                );
                let Some(owner_human_id) = owner_human_id.as_ref() else {
                    continue;
                };
                let context = collaboration_service::RouterSessionAppServerContext::new(
                    endpoint,
                    message_board::Identity::Human {
                        human_id: owner_human_id.clone(),
                    },
                    std::sync::Arc::clone(&commands),
                    std::sync::Arc::clone(&events),
                    catalog,
                )
                .with_interaction_broker(std::sync::Arc::clone(&approval_broker))
                .with_project_trust(std::sync::Arc::new(
                    codex_native_integration::CodexHomeProjectTrust::new(inputs.codex_home.clone()),
                ));
                let socket_path = socket_directory.join(format!(
                    "{}.sock",
                    String::from(description.endpoint.endpoint_id.clone())
                ));
                provider_app_servers.push(
                    collaboration_service::RouterSessionAppServerListener::bind(
                        &socket_path,
                        std::sync::Arc::new(context),
                    )?,
                );
            }
        }
        let publication = publication.with_acp_listener()?;
        if let Some(settings) = settings_backend
            && settings.recover().await.is_err()
        {
            tracing::warn!(
                "automation configuration recovery unavailable; new automation admission is paused"
            );
        }
        let manifest = collaboration_service::ManifestPublication::publish(
            &inputs.directory,
            &collaboration_protocol::ServiceManifest {
                version: collaboration_protocol::SERVICE_MANIFEST_VERSION,
                service_id: machine_identity.service_id().clone(),
                service_epoch: service_epoch.clone(),
                machine_label: machine_identity.machine_label().clone(),
                service_version: NonEmptyText::try_from(env!("CARGO_PKG_VERSION").to_owned())
                    .map_err(io::Error::other)?,
                api: collaboration_protocol::ApiSelector {
                    transport: collaboration_protocol::ApiTransport::StreamableHttpUnix,
                    path: collaboration_protocol::ApiSocketPath::ServiceSocket,
                },
                mcp: collaboration_protocol::McpSelector {
                    transport: collaboration_protocol::McpTransport::StreamableHttp,
                    url: mcp_url,
                },
                native_schema_digest: native_digest.clone(),
                router_proxy_endpoint,
            },
        )?;
        let shutdown = CancellationToken::new();
        if let Some(service) = &subscription_delivery
            && let Err(error) = service.start().await
        {
            service.shutdown().await;
            drop(manifest);
            return Err(io::Error::other(error));
        }
        // Binding above establishes this runtime owns the listeners before recovery can mutate state.
        let automation_task = wake_worker.map(|worker| tokio::spawn(worker.run(shutdown.clone())));
        let schedule_task =
            schedule_worker.map(|worker| tokio::spawn(worker.run(shutdown.clone())));
        let automation_maintenance =
            retention_worker.map(|worker| tokio::spawn(worker.run(shutdown.clone())));
        let collaboration_api =
            collaboration_api.serve(&collaboration_mcp::CollaborationApiConfig {
                application,
                service_directory: inputs.directory.clone(),
                native_definitions,
                router_executable_relation: relation_receiver,
                concurrent_requests: collaboration_mcp::DEFAULT_CONCURRENT_REQUESTS,
                shutdown: shutdown.clone(),
            });
        let mut tasks = JoinSet::new();
        let mut provider_retirement_tasks = JoinSet::new();
        tasks.spawn(native.run(shutdown.clone()));
        tasks.spawn(acp.run(shutdown.clone()));
        for app_server in provider_app_servers {
            tasks.spawn(app_server.run(shutdown.clone()));
        }
        for (retirement, mut description) in provider_retirements {
            let directory = endpoint_directory.clone();
            provider_retirement_tasks.spawn(async move {
                retirement.cancelled().await;
                description.availability = EndpointAvailability::Unavailable {
                    observed_at: current_observation_timestamp()?,
                    reason: NonEmptyText::try_from("provider process retired".to_owned())
                        .map_err(io::Error::other)?,
                    fix: Some(
                        NonEmptyText::try_from(
                            "restart the Host to relaunch the provider".to_owned(),
                        )
                        .map_err(io::Error::other)?,
                    ),
                };
                directory.publish(description)
            });
        }
        let maintenance = journal.as_ref().map(|journal| {
            let maintenance_store = std::sync::Arc::clone(journal);
            let maintenance_stop = shutdown.clone();
            tokio::spawn(async move { maintenance_store.run_maintenance(maintenance_stop).await })
        });
        Ok(Self {
            owner_human_id,
            board_store,
            subscription_delivery,
            provider_store,
            external_provider_supervisor,
            provider_delivery_route,
            provider_retention: None,
            directory: inputs.directory,
            native_digest,
            payload_cache: crate::native_schema_cache::NativeSchemaCache::default(),
            service_id,
            service_epoch,
            publication,
            shutdown,
            tasks,
            provider_retirement_tasks,
            manifest: Some(manifest),
            journal,
            maintenance,
            automation_task,
            schedule_task,
            automation_maintenance,
            current_generation: None,
            observer_task: None,
            collaboration_api,
        })
    }

    #[must_use]
    pub fn owner_human_id(&self) -> Option<&message_board::HumanId> {
        self.owner_human_id.as_ref()
    }

    #[must_use]
    pub fn service_id(&self) -> &UuidIdentity {
        &self.service_id
    }
    #[must_use]
    pub fn service_epoch(&self) -> &UuidIdentity {
        &self.service_epoch
    }
    pub async fn backend_ready(
        &mut self,
        at: ObservationTimestamp,
        schema: Option<BackendSchemaEvidence<'_>>,
    ) -> io::Result<CodexGeneration> {
        let timing = std::time::Instant::now();
        crate::debug_readiness_timing::record("publicReadinessBegin", timing);
        // Revalidate executable identity/publication on every generation; only compilation is cached.
        let (schema, payload_schemas) = match schema {
            Some(evidence) => {
                let digest = self
                    .publish_native_schema(BackendSchemaEvidence {
                        running_executable: evidence.running_executable,
                        export: evidence.export,
                    })
                    .await?;
                crate::debug_readiness_timing::record("publicSchemaVerified", timing);
                let compiled = self.payload_cache.resolve(evidence.export.bundle());
                crate::debug_readiness_timing::record("publicValidatorsResolved", timing);
                (Some(digest), compiled)
            }
            None => (None, None),
        };
        let generation = self.publication.ready(
            at.clone(),
            schema.clone(),
            if self.native_digest.as_ref() == schema.as_ref() {
                payload_schemas.clone()
            } else {
                None
            },
        )?;
        crate::debug_readiness_timing::record("publicGenerationPublished", timing);
        self.current_generation = Some(generation.clone());
        if self
            .record_backend(at, collaboration_protocol::BackendStatus::Ready)
            .await
            .is_err()
        {
            tracing::warn!("backend readiness could not be recorded in lifecycle storage");
        }
        if let Some(schemas) = payload_schemas
            && self.start_native_observer(schemas).is_err()
        {
            tracing::warn!("native observer could not start");
        }
        Ok(generation)
    }
    fn start_native_observer(
        &mut self,
        schemas: std::sync::Arc<codex_native_integration::NativePayloadSchemas>,
    ) -> io::Result<()> {
        let Some(journal) = &self.journal else {
            return Ok(());
        };
        if self.observer_task.is_some() {
            return Err(io::Error::other("previous native observer has not retired"));
        }
        let admission = self.publication.admission_gate().acquire()?;
        let store = std::sync::Arc::clone(journal);
        let scope = collaboration_protocol::ObservationScope {
            endpoint: EndpointRef {
                service_id: self.service_id.clone(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned())
                    .map_err(io::Error::other)?,
            },
            generation: Some(admission.generation().clone()),
            observer_id: new_service_uuid()?,
        };
        self.observer_task = Some(tokio::spawn(async move {
            let retired = admission.retirement();
            let connection = tokio::select! {
                _ = retired.cancelled() => return,
                connection = codex_native_integration::NativeProtocolConnection::connect(admission.backend_path()) => connection,
            };
            match connection {
                Ok(connection) => {
                    match lifecycle_observation::NativeObservationStream::new(
                        lifecycle_observation::NativeObservationInputs {
                            connection,
                            store,
                            scope,
                            schemas,
                        },
                    ) {
                        Ok(observer) => {
                            if observer.run(retired).await.is_err() {
                                tracing::warn!("native lifecycle observation stopped");
                            }
                        }
                        Err(_) => tracing::warn!("native lifecycle observer scope rejected"),
                    }
                }
                Err(_) => {
                    let observed_at = chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                        .try_into();
                    if let Ok(observed_at) = observed_at {
                        let observation = collaboration_protocol::LifecycleObservation {
                            observed_at,
                            source: collaboration_protocol::ObservationSource::ObserverLifecycle,
                            scope,
                            subject: collaboration_protocol::LifecycleSubject::Backend,
                            change: collaboration_protocol::LifecycleChange::CoverageLost,
                        };
                        let _recorded = store
                            .append(&observation, chrono::Utc::now().timestamp())
                            .await;
                    }
                    tracing::warn!("native lifecycle observation connection unavailable");
                }
            }
        }));
        Ok(())
    }
    async fn publish_native_schema(
        &self,
        evidence: BackendSchemaEvidence<'_>,
    ) -> io::Result<SchemaDigest> {
        if evidence.export.executable() != evidence.running_executable {
            return Err(io::Error::other(
                "schema executable differs from running backend",
            ));
        }
        let observed = codex_native_integration::executable_identity(
            evidence.running_executable.canonical_path(),
        )
        .await
        .map_err(io::Error::other)?;
        if &observed != evidence.running_executable {
            return Err(io::Error::other(
                "schema executable changed before publication",
            ));
        }
        // Publication precedes the directory announcement; readers never receive a missing bundle.
        evidence
            .export
            .bundle()
            .publish(&self.directory)
            .map_err(io::Error::other)?;
        let mut encoded = String::from("sha256:");
        for byte in evidence.export.bundle().digest() {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").map_err(io::Error::other)?;
        }
        SchemaDigest::try_from(encoded).map_err(io::Error::other)
    }
    pub async fn backend_unavailable(
        &mut self,
        at: ObservationTimestamp,
        reason: NonEmptyText,
    ) -> io::Result<()> {
        self.publication.unavailable(at.clone(), reason)?;
        self.drain_native_observer().await;
        if self
            .record_backend(at, collaboration_protocol::BackendStatus::Unavailable)
            .await
            .is_err()
        {
            tracing::warn!("backend loss could not be recorded in lifecycle storage");
        }
        Ok(())
    }
    async fn drain_native_observer(&mut self) {
        if let Some(observer) = self.observer_task.take() {
            let _drained = observer.await;
        }
    }
    async fn record_backend(
        &self,
        at: ObservationTimestamp,
        status: collaboration_protocol::BackendStatus,
    ) -> io::Result<()> {
        let Some(journal) = &self.journal else {
            return Ok(());
        };
        let observation = collaboration_protocol::LifecycleObservation {
            observed_at: at,
            source: collaboration_protocol::ObservationSource::HostLifecycle,
            scope: collaboration_protocol::ObservationScope {
                endpoint: EndpointRef {
                    service_id: self.service_id.clone(),
                    endpoint_id: EndpointId::try_from("codex-local".to_owned())
                        .map_err(io::Error::other)?,
                },
                generation: self.current_generation.clone(),
                observer_id: self.service_epoch.clone(),
            },
            subject: collaboration_protocol::LifecycleSubject::Backend,
            change: collaboration_protocol::LifecycleChange::BackendStatus { status },
        };
        journal
            .append(&observation, unix_seconds()?)
            .await
            .map_err(io::Error::other)?;
        Ok(())
    }
}

#[path = "collaboration_runtime/collaboration_api_serving.rs"]
mod collaboration_api_serving;
use collaboration_api_serving::{BoundCollaborationApi, ServedCollaborationApi};
#[path = "collaboration_runtime/lifecycle.rs"]
mod lifecycle;

impl Drop for CollaborationRuntime {
    fn drop(&mut self) {
        self.manifest.take();
        self.shutdown.cancel();
        if let Some(service) = &self.subscription_delivery {
            service.cancel();
        }
        let _retire = self.publication.admission_gate().retire();
    }
}

fn unix_seconds() -> io::Result<i64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?;
    i64::try_from(elapsed.as_secs()).map_err(io::Error::other)
}

fn create_private_socket_directory(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path)?;
            if metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0 {
                Ok(())
            } else {
                Err(io::Error::other(
                    "provider socket directory must be private",
                ))
            }
        }
        Err(error) => Err(error),
    }
}

fn current_observation_timestamp() -> io::Result<ObservationTimestamp> {
    ObservationTimestamp::try_from(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )
    .map_err(io::Error::other)
}
