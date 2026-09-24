//! Host-owned composition of public listeners and lifecycle publication.
use crate::BackendPublication;
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, EndpointAvailability, EndpointDescription, EndpointId,
    EndpointRef, GenerationNumber, NonEmptyText, ObservationTimestamp, ProviderBindingId,
    ProviderBindingIdentity, ProviderCapabilities, ProviderCapability, ProviderCapabilityEvidence,
    ProviderCapabilityName, ProviderCapabilityStatus, ProviderKind, ProviderRuntimeIdentity,
    ProviderTransport, SchemaDigest, UuidIdentity,
};
use collaboration_service::{
    LocalControlService, NativeRelayListener, ServiceIdentity, load_service_identity,
    new_service_uuid,
};
use std::{io, path::PathBuf};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub struct CollaborationRuntimeInputs {
    pub directory: PathBuf,
    pub codex_home: PathBuf,
    pub backend_socket: PathBuf,
    pub mcp_bind: std::net::SocketAddr,
    /// An actual executable-bound export, or metadata-only schema admission.
    pub native_schema: Option<std::sync::Arc<codex_native_integration::NativeSchemaExport>>,
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
    board_store: Option<std::sync::Arc<tokio::sync::Mutex<message_board_storage::BoardStore>>>,
    provider_store:
        Option<std::sync::Arc<tokio::sync::Mutex<collaboration_service::ProviderOperationStore>>>,
    external_provider_supervisor: Option<std::sync::Arc<crate::ExternalProviderSupervisor>>,
    provider_retention: Option<tokio::task::JoinHandle<()>>,
    directory: PathBuf,
    control_schema: collaboration_protocol::ControlSchema,
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
    mcp: Option<collaboration_mcp::CollaborationMcpListener>,
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
        provider_launches: Vec<ExternalProviderLaunchBinding>,
    ) -> io::Result<Self> {
        let service_id = load_service_identity(&inputs.directory)?;
        let service_epoch = new_service_uuid()?;
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
        let control_schema = collaboration_protocol::ControlSchema::generate(native_digest)
            .map_err(io::Error::other)?;
        collaboration_service::publish_control_schema(&inputs.directory, &control_schema)?;
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
            &String::from(control_schema.digest().clone()),
        )
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
        let mut settings_backend = None;
        match automation_storage::AutomationStore::open(&inputs.directory.join("automation.sqlite"))
            .await
        {
            Ok(store) => {
                let store = std::sync::Arc::new(tokio::sync::Mutex::new(store));
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
        let mcp_bind = collaboration_mcp::LoopbackBindAddress::new(inputs.mcp_bind)
            .map_err(io::Error::other)?;
        let mcp = collaboration_mcp::CollaborationMcpListener::start(
            collaboration_mcp::CollaborationMcpListenerConfig {
                bind_address: mcp_bind,
                service_directory: inputs.directory.clone(),
                allowed_origins: Vec::new(),
            },
        )
        .await?;
        let mcp_url = mcp.local_url();
        let mut external_provider_supervisor = None;
        let mut provider_retirements = Vec::new();
        if !provider_launches.is_empty() {
            let store = provider_store.clone().ok_or_else(|| {
                io::Error::other("external provider operation storage is unavailable")
            })?;
            let mut bindings = Vec::with_capacity(provider_launches.len());
            let mut endpoint_descriptions = Vec::with_capacity(provider_launches.len());
            for (index, configured) in provider_launches.into_iter().enumerate() {
                let runtime = crate::ExternalProviderRuntime::initialize_with_mcp_http(
                    configured.launch,
                    "router-collaboration",
                    mcp_url.clone(),
                )
                .await
                .map_err(io::Error::other)?;
                let endpoint = EndpointRef {
                    service_id: service_id.clone(),
                    endpoint_id: configured.endpoint_id,
                };
                let generation_number = GenerationNumber::try_from(
                    u64::try_from(index)
                        .map_err(io::Error::other)?
                        .saturating_add(1),
                )
                .map_err(io::Error::other)?;
                let runtime_name = runtime.admission().runtime_name.clone().unwrap_or_else(|| {
                    match configured.provider {
                        ProviderKind::ClaudeCode => "claude-code",
                        ProviderKind::Cursor => "cursor",
                    }
                    .to_owned()
                });
                let runtime_identity = ProviderRuntimeIdentity {
                    provider: configured.provider,
                    runtime_name: NonEmptyText::try_from(runtime_name).map_err(io::Error::other)?,
                    runtime_version: runtime
                        .admission()
                        .runtime_version
                        .clone()
                        .map(NonEmptyText::try_from)
                        .transpose()
                        .map_err(io::Error::other)?,
                };
                let capabilities = provider_capabilities(
                    runtime.admission().supports_load,
                    runtime.admission().supports_mcp_http,
                )?;
                let binding_id = ProviderBindingId::try_from(String::from(new_service_uuid()?))
                    .map_err(io::Error::other)?;
                let generation = CodexGeneration {
                    service_epoch: service_epoch.clone(),
                    generation: generation_number,
                };
                let binding = ProviderBindingIdentity {
                    endpoint: endpoint.clone(),
                    binding_id: binding_id.clone(),
                    runtime: runtime_identity.clone(),
                    transport: ProviderTransport::StdioAcp,
                    generation: generation.clone(),
                    capabilities: capabilities.clone(),
                };
                let endpoint_description = EndpointDescription {
                    endpoint,
                    label: configured.label,
                    availability: EndpointAvailability::Available {
                        observed_at: current_observation_timestamp()?,
                    },
                    channels: vec![ChannelDescription::ExternalProvider {
                        transport: ProviderTransport::StdioAcp,
                        binding_id,
                        binding_generation: generation_number,
                        runtime: runtime_identity,
                        capabilities,
                    }],
                };
                provider_retirements.push((runtime.retirement(), endpoint_description.clone()));
                endpoint_descriptions.push(endpoint_description);
                bindings.push(crate::ExternalProviderBinding {
                    identity: binding,
                    runtime,
                });
            }
            let supervisor = std::sync::Arc::new(
                crate::ExternalProviderSupervisor::new(bindings, store)
                    .map_err(io::Error::other)?,
            );
            let provider_backend: std::sync::Arc<
                dyn collaboration_service::ProviderConversationBackend,
            > = supervisor.clone();
            identity = identity
                .with_endpoints(endpoint_descriptions)
                .map_err(io::Error::other)?
                .with_provider_conversation_backend(provider_backend);
            external_provider_supervisor = Some(supervisor);
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
        let approval_broker = collaboration_service::ServiceApprovalBroker::load(
            service_id.clone(),
            native_backend.clone(),
            inputs.directory.join("approval-routes.json"),
        )
        .await
        .map_err(io::Error::other)?;
        let codex_route: std::sync::Arc<dyn collaboration_service::SessionDeliveryRoute> =
            std::sync::Arc::new(collaboration_service::CodexAppServerDeliveryRoute::new(
                service_id.clone(),
                identity.endpoint_directory(),
                native_backend.clone(),
            ));
        let session_router =
            std::sync::Arc::new(collaboration_service::SessionDeliveryRouter::new(vec![
                codex_route,
            ]));
        let session_delivery: std::sync::Arc<dyn collaboration_service::SessionMessageDelivery> =
            session_router.clone();
        let scheduled_run_execution: std::sync::Arc<
            dyn collaboration_service::ScheduledRunExecution,
        > = session_router;
        approval_broker
            .install_session_delivery(std::sync::Arc::clone(&session_delivery))
            .map_err(io::Error::other)?;
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
        let identity = identity.with_approval_broker(std::sync::Arc::clone(&approval_broker));
        let endpoint_directory = identity.endpoint_directory();
        let wake_worker = identity.wake_timing_worker();
        let schedule_worker = identity.schedule_timing_worker();
        let retention_worker = identity.automation_retention_worker();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
        let control = LocalControlService::bind(&inputs.directory.join("control.sock"), identity)?
            .with_connection_budget(std::sync::Arc::clone(&permits));
        let native = NativeRelayListener::bind(
            &inputs.directory.join("codex-native.sock"),
            publication.admission_gate(),
        )?
        .with_connection_budget(std::sync::Arc::clone(&permits));
        let stored = std::sync::Arc::new(collaboration_service::NativeStoredSessions::new(
            inputs.codex_home,
            String::from(service_id.clone()),
        ));
        let acp = collaboration_service::AcpChannelListener::bind(
            &inputs.directory.join("codex-acp.sock"),
            publication.admission_gate(),
            stored,
            approval_broker,
        )?
        .with_connection_budget(permits);
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
                version: 2,
                service_id: service_id.clone(),
                service_epoch: service_epoch.clone(),
                control: collaboration_protocol::ControlSelector {
                    transport: collaboration_protocol::ControlTransport::UnixJsonLines,
                    path: collaboration_protocol::ControlSocketPath::ControlSocket,
                },
                control_schema_digest: control_schema.digest().clone(),
                mcp: collaboration_protocol::McpSelector {
                    transport: collaboration_protocol::McpTransport::StreamableHttp,
                    url: mcp_url,
                },
            },
        )?;
        let shutdown = CancellationToken::new();
        // Binding above establishes this runtime owns the listeners before recovery can mutate state.
        let automation_task = wake_worker.map(|worker| tokio::spawn(worker.run(shutdown.clone())));
        let schedule_task =
            schedule_worker.map(|worker| tokio::spawn(worker.run(shutdown.clone())));
        let automation_maintenance =
            retention_worker.map(|worker| tokio::spawn(worker.run(shutdown.clone())));
        let mut tasks = JoinSet::new();
        let mut provider_retirement_tasks = JoinSet::new();
        tasks.spawn(control.run(shutdown.clone()));
        tasks.spawn(native.run(shutdown.clone()));
        tasks.spawn(acp.run(shutdown.clone()));
        for (retirement, mut description) in provider_retirements {
            let directory = endpoint_directory.clone();
            provider_retirement_tasks.spawn(async move {
                retirement.cancelled().await;
                description.availability = EndpointAvailability::Unavailable {
                    observed_at: current_observation_timestamp()?,
                    reason: NonEmptyText::try_from("provider process retired".to_owned())
                        .map_err(io::Error::other)?,
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
            board_store,
            provider_store,
            external_provider_supervisor,
            provider_retention: None,
            directory: inputs.directory,
            control_schema,
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
            mcp: Some(mcp),
        })
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
            if self.control_schema.native_digest() == schema.as_ref() {
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
    /// The lifecycle event loop must monitor this future; listener death is not healthy readiness.
    pub async fn listener_failure(&mut self) -> io::Error {
        loop {
            let failure = tokio::select! {
                task = self.tasks.join_next() => match task {
                    Some(Ok(Err(error))) => error,
                    Some(Err(_)) => io::Error::other("collaboration listener task failed"),
                    _ => io::Error::other("collaboration listener stopped unexpectedly"),
                },
                task = self.provider_retirement_tasks.join_next(), if !self.provider_retirement_tasks.is_empty() => match task {
                    Some(Ok(Ok(()))) => continue,
                    Some(Ok(Err(error))) => error,
                    Some(Err(_)) => io::Error::other("provider retirement publication task failed"),
                    None => continue,
                },
                failure = async {
                    match &mut self.mcp {
                        Some(mcp) => mcp.listener_failure().await,
                        None => std::future::pending().await,
                    }
                } => failure,
            };
            self.manifest.take();
            self.shutdown.cancel();
            let _retire = self.publication.admission_gate().retire();
            return failure;
        }
    }
    pub async fn shutdown(mut self) -> io::Result<()> {
        self.manifest.take();
        self.shutdown.cancel();
        let mut failure = None;
        if let Some(mcp) = self.mcp.take()
            && let Err(error) = mcp.shutdown().await
        {
            failure.get_or_insert(error);
        }
        if let Err(error) = self.publication.admission_gate().retire() {
            failure.get_or_insert(error);
        }
        if let Some(supervisor) = self.external_provider_supervisor.take()
            && let Err(message) = supervisor.shutdown().await
        {
            failure.get_or_insert(io::Error::other(message));
        }
        while let Some(result) = self.provider_retirement_tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert(io::Error::other(
                        "provider retirement publication task failed",
                    ));
                }
            }
        }
        self.drain_native_observer().await;
        while let Some(result) = self.tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert(io::Error::other("collaboration task shutdown failed"));
                }
            }
        }
        if let Some(maintenance) = self.maintenance.take() {
            match maintenance.await {
                Ok(Ok(())) => {}
                _ => {
                    failure.get_or_insert(io::Error::other("lifecycle maintenance failed"));
                }
            }
        }
        if let Some(task) = self.automation_task.take()
            && task.await.is_err()
        {
            failure.get_or_insert(io::Error::other("automation worker shutdown failed"));
        }
        if let Some(task) = self.schedule_task.take()
            && task.await.is_err()
        {
            failure.get_or_insert(io::Error::other("schedule worker shutdown failed"));
        }
        if let Some(task) = self.automation_maintenance.take()
            && task.await.is_err()
        {
            failure.get_or_insert(io::Error::other("automation maintenance shutdown failed"));
        }
        if let Some(task) = self.provider_retention.take()
            && task.await.is_err()
        {
            failure.get_or_insert(io::Error::other("provider retention maintenance failed"));
        }
        if let Some(store) = self.provider_store.take() {
            match std::sync::Arc::try_unwrap(store) {
                Ok(store) => {
                    if store.into_inner().close().await.is_err() {
                        failure.get_or_insert(io::Error::other(
                            "provider operation storage close failed",
                        ));
                    }
                }
                Err(_) => {
                    failure.get_or_insert(io::Error::other(
                        "provider operation storage still owned after listener shutdown",
                    ));
                }
            }
        }
        if let Some(store) = self.board_store.take() {
            match std::sync::Arc::try_unwrap(store) {
                Ok(store) => {
                    if store.into_inner().close().await.is_err() {
                        failure.get_or_insert(io::Error::other("board storage close failed"));
                    }
                }
                Err(_) => {
                    failure.get_or_insert(io::Error::other(
                        "board storage still owned after listener shutdown",
                    ));
                }
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
impl Drop for CollaborationRuntime {
    fn drop(&mut self) {
        self.manifest.take();
        self.shutdown.cancel();
        let _retire = self.publication.admission_gate().retire();
    }
}

fn unix_seconds() -> io::Result<i64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?;
    i64::try_from(elapsed.as_secs()).map_err(io::Error::other)
}

fn current_observation_timestamp() -> io::Result<ObservationTimestamp> {
    ObservationTimestamp::try_from(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )
    .map_err(io::Error::other)
}

fn provider_capabilities(
    supports_load: bool,
    supports_mcp_http: bool,
) -> io::Result<ProviderCapabilities> {
    ProviderCapabilities::try_from(vec![
        ProviderCapability {
            name: ProviderCapabilityName::Create,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        },
        ProviderCapability {
            name: ProviderCapabilityName::Prompt,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        },
        ProviderCapability {
            name: ProviderCapabilityName::Load,
            status: if supports_load {
                ProviderCapabilityStatus::Supported
            } else {
                ProviderCapabilityStatus::Unsupported
            },
            evidence: if supports_load {
                ProviderCapabilityEvidence::Advertised
            } else {
                ProviderCapabilityEvidence::NotAvailable
            },
        },
        ProviderCapability {
            name: ProviderCapabilityName::Cancel,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        },
        ProviderCapability {
            name: ProviderCapabilityName::Permissions,
            status: ProviderCapabilityStatus::Unverified,
            evidence: ProviderCapabilityEvidence::NotAvailable,
        },
        ProviderCapability {
            name: ProviderCapabilityName::CollaborationMcp,
            status: if supports_mcp_http {
                ProviderCapabilityStatus::Supported
            } else {
                ProviderCapabilityStatus::Unsupported
            },
            evidence: if supports_mcp_http {
                ProviderCapabilityEvidence::Advertised
            } else {
                ProviderCapabilityEvidence::NotAvailable
            },
        },
        ProviderCapability {
            name: ProviderCapabilityName::CallerDetach,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::RouterQualified,
        },
    ])
    .map_err(io::Error::other)
}
