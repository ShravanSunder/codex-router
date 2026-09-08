//! Host-owned composition of public listeners and lifecycle publication.
use crate::BackendPublication;
use communication_protocol::{
    CodexGeneration, EndpointId, EndpointRef, NonEmptyText, ObservationTimestamp, SchemaDigest,
    UuidIdentity,
};
use communication_service::{
    LocalControlService, NativeRelayListener, ServiceIdentity, load_service_identity,
    new_service_uuid,
};
use std::{io, path::PathBuf};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub struct CommunicationRuntimeInputs {
    pub directory: PathBuf,
    pub codex_home: PathBuf,
    pub backend_socket: PathBuf,
    /// An actual executable-bound export, or metadata-only schema admission.
    pub native_schema: Option<std::sync::Arc<codex_native_integration::NativeSchemaExport>>,
}
pub struct BackendSchemaEvidence<'a> {
    pub running_executable: &'a codex_native_integration::ExecutableIdentity,
    pub export: &'a codex_native_integration::NativeSchemaExport,
}
pub struct CommunicationRuntime {
    directory: PathBuf,
    control_schema: communication_protocol::ControlSchema,
    payload_cache: crate::native_schema_cache::NativeSchemaCache,
    service_id: UuidIdentity,
    service_epoch: UuidIdentity,
    publication: BackendPublication,
    shutdown: CancellationToken,
    tasks: JoinSet<io::Result<()>>,
    journal: Option<std::sync::Arc<lifecycle_observation::LifecycleStore>>,
    maintenance: Option<tokio::task::JoinHandle<Result<(), lifecycle_observation::JournalError>>>,
    automation_task: Option<tokio::task::JoinHandle<()>>,
    current_generation: Option<CodexGeneration>,
    observer_task: Option<tokio::task::JoinHandle<()>>,
    manifest: Option<communication_service::ManifestPublication>,
}
impl CommunicationRuntime {
    /// Binds both private listeners before spawning tasks. No native process is started.
    pub async fn start(inputs: CommunicationRuntimeInputs) -> io::Result<Self> {
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
        let control_schema = communication_protocol::ControlSchema::generate(native_digest)
            .map_err(io::Error::other)?;
        communication_service::publish_control_schema(&inputs.directory, &control_schema)?;
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
                tracing::warn!("lifecycle storage unavailable; communication remains independent");
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
        match automation_storage::AutomationStore::open(&inputs.directory.join("automation.sqlite"))
            .await
        {
            Ok(store) => {
                identity = identity
                    .with_automation_store(std::sync::Arc::new(tokio::sync::Mutex::new(store)));
            }
            Err(_) => {
                tracing::warn!("automation storage unavailable; communication remains independent")
            }
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
        let identity = identity
            .with_native_backend(communication_service::NativeControlBackend {
                codex_home: inputs.codex_home.clone(),
                endpoint,
                gate: publication.admission_gate(),
            })
            .map_err(io::Error::other)?;
        let wake_worker = identity.wake_timing_worker();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
        let control = LocalControlService::bind(&inputs.directory.join("control.sock"), identity)?
            .with_connection_budget(std::sync::Arc::clone(&permits));
        let native = NativeRelayListener::bind(
            &inputs.directory.join("codex-native.sock"),
            publication.admission_gate(),
        )?
        .with_connection_budget(std::sync::Arc::clone(&permits));
        let stored = std::sync::Arc::new(communication_service::NativeStoredSessions::new(
            inputs.codex_home,
            String::from(service_id.clone()),
        ));
        let acp = communication_service::AcpChannelListener::bind(
            &inputs.directory.join("codex-acp.sock"),
            publication.admission_gate(),
            stored,
        )?
        .with_connection_budget(permits);
        let publication = publication.with_acp_listener()?;
        let manifest = communication_service::ManifestPublication::publish(
            &inputs.directory,
            &communication_protocol::ServiceManifest {
                version: 1,
                service_id: service_id.clone(),
                service_epoch: service_epoch.clone(),
                control: communication_protocol::ControlSelector {
                    transport: communication_protocol::ControlTransport::UnixJsonLines,
                    path: communication_protocol::ControlSocketPath::ControlSocket,
                },
                control_schema_digest: control_schema.digest().clone(),
            },
        )?;
        let shutdown = CancellationToken::new();
        // Binding above establishes this runtime owns the listeners before recovery can mutate state.
        let automation_task = wake_worker.map(|worker| tokio::spawn(worker.run(shutdown.clone())));
        let mut tasks = JoinSet::new();
        tasks.spawn(control.run(shutdown.clone()));
        tasks.spawn(native.run(shutdown.clone()));
        tasks.spawn(acp.run(shutdown.clone()));
        let maintenance = journal.as_ref().map(|journal| {
            let maintenance_store = std::sync::Arc::clone(journal);
            let maintenance_stop = shutdown.clone();
            tokio::spawn(async move { maintenance_store.run_maintenance(maintenance_stop).await })
        });
        Ok(Self {
            directory: inputs.directory,
            control_schema,
            payload_cache: crate::native_schema_cache::NativeSchemaCache::default(),
            service_id,
            service_epoch,
            publication,
            shutdown,
            tasks,
            manifest: Some(manifest),
            journal,
            maintenance,
            automation_task,
            current_generation: None,
            observer_task: None,
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
            .record_backend(at, communication_protocol::BackendStatus::Ready)
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
        let scope = communication_protocol::ObservationScope {
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
                        let observation = communication_protocol::LifecycleObservation {
                            observed_at,
                            source: communication_protocol::ObservationSource::ObserverLifecycle,
                            scope,
                            subject: communication_protocol::LifecycleSubject::Backend,
                            change: communication_protocol::LifecycleChange::CoverageLost,
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
            .record_backend(at, communication_protocol::BackendStatus::Unavailable)
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
        status: communication_protocol::BackendStatus,
    ) -> io::Result<()> {
        let Some(journal) = &self.journal else {
            return Ok(());
        };
        let observation = communication_protocol::LifecycleObservation {
            observed_at: at,
            source: communication_protocol::ObservationSource::HostLifecycle,
            scope: communication_protocol::ObservationScope {
                endpoint: EndpointRef {
                    service_id: self.service_id.clone(),
                    endpoint_id: EndpointId::try_from("codex-local".to_owned())
                        .map_err(io::Error::other)?,
                },
                generation: self.current_generation.clone(),
                observer_id: self.service_epoch.clone(),
            },
            subject: communication_protocol::LifecycleSubject::Backend,
            change: communication_protocol::LifecycleChange::BackendStatus { status },
        };
        journal
            .append(&observation, unix_seconds()?)
            .await
            .map_err(io::Error::other)?;
        Ok(())
    }
    /// The lifecycle event loop must monitor this future; listener death is not healthy readiness.
    pub async fn listener_failure(&mut self) -> io::Error {
        let failure = match self.tasks.join_next().await {
            Some(Ok(Err(error))) => error,
            Some(Err(_)) => io::Error::other("communication listener task failed"),
            _ => io::Error::other("communication listener stopped unexpectedly"),
        };
        self.manifest.take();
        self.shutdown.cancel();
        let _retire = self.publication.admission_gate().retire();
        failure
    }
    pub async fn shutdown(mut self) -> io::Result<()> {
        self.manifest.take();
        self.shutdown.cancel();
        self.publication.admission_gate().retire()?;
        self.drain_native_observer().await;
        let mut failure = None;
        while let Some(result) = self.tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert(io::Error::other("communication task shutdown failed"));
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
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
impl Drop for CommunicationRuntime {
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
