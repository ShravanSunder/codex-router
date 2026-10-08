use super::*;

#[derive(Clone, Debug)]
pub(super) struct RuntimeWritableStateStores {
    pub(super) credential_state_store: AsyncSqliteStateStore,
    pub(super) db_write_state_store: AsyncSqliteStateStore,
    pub(super) maintenance_state_store: AsyncSqliteStateStore,
}

pub(super) async fn open_runtime_writable_state_stores(
    state_database_path: &Path,
) -> Result<RuntimeWritableStateStores, StateStoreError> {
    let credential_state_store = AsyncSqliteStateStore::open(state_database_path).await?;
    let db_write_state_store = AsyncSqliteStateStore::open(state_database_path).await?;
    let maintenance_state_store = AsyncSqliteStateStore::open(state_database_path).await?;
    Ok(RuntimeWritableStateStores {
        credential_state_store,
        db_write_state_store,
        maintenance_state_store,
    })
}

impl LoopbackRouterRuntime {
    /// Opens router-owned state and binds using the process-owned encrypted credential handle.
    pub async fn start(
        config: LoopbackRouterRuntimeConfig,
        credential_store: EncryptedCredentialStore,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let caller_runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            LoopbackRouterRuntimeError::TokioRuntime(std::io::Error::other(error))
        })?;
        let caller_dispatcher = tracing::dispatcher::get_default(|dispatcher| dispatcher.clone());
        Self::start_with_test_maintenance_completion_sender(
            config,
            credential_store,
            None,
            caller_dispatcher,
            &caller_runtime,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn start_for_test(
        config: LoopbackRouterRuntimeConfig,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let credential_store = test_credential_store_for_config(&config)?;
        Self::start(config, credential_store).await
    }

    #[cfg(test)]
    pub(crate) async fn start_for_test_with_maintenance_completion_sender(
        config: LoopbackRouterRuntimeConfig,
        completion_sender: std::sync::mpsc::Sender<crate::maintenance_actor::MaintenanceCompletion>,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let credential_store = test_credential_store_for_config(&config)?;
        Self::start_with_maintenance_completion_sender(config, credential_store, completion_sender)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn start_with_maintenance_completion_sender(
        config: LoopbackRouterRuntimeConfig,
        credential_store: EncryptedCredentialStore,
        completion_sender: std::sync::mpsc::Sender<crate::maintenance_actor::MaintenanceCompletion>,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let caller_runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            LoopbackRouterRuntimeError::TokioRuntime(std::io::Error::other(error))
        })?;
        let caller_dispatcher = tracing::dispatcher::get_default(|dispatcher| dispatcher.clone());
        Self::start_with_test_maintenance_completion_sender(
            config,
            credential_store,
            Some(completion_sender),
            caller_dispatcher,
            &caller_runtime,
        )
        .await
    }

    pub(super) async fn start_with_test_maintenance_completion_sender(
        config: LoopbackRouterRuntimeConfig,
        credential_store: EncryptedCredentialStore,
        #[cfg(test)] completion_sender: Option<
            std::sync::mpsc::Sender<crate::maintenance_actor::MaintenanceCompletion>,
        >,
        #[cfg(not(test))] _completion_sender: Option<()>,
        caller_dispatcher: tracing::dispatcher::Dispatch,
        caller_runtime: &tokio::runtime::Handle,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let session_affinity_cache = config.session_account_affinity_cache();
        let fixed_now_unix_seconds = config.fixed_now_unix_seconds;
        let claude_edge_runtime_config = config.claude_edge_runtime_config.clone();
        let credential_store_availability = match credential_store.status() {
            EncryptedCredentialStoreStatus::Ready => CredentialStoreAvailability::Available,
            EncryptedCredentialStoreStatus::KeyUnavailable => {
                CredentialStoreAvailability::KeyUnreadable
            }
            EncryptedCredentialStoreStatus::MigrationIncomplete { .. } => {
                CredentialStoreAvailability::MigrationIncomplete
            }
        };
        let credential_resources =
            ProxyRuntimeCredentialResources::open(credential_store, fixed_now_unix_seconds)?;
        let affinity_secret_provider = credential_resources.affinity_secret_provider();
        let credential_factory = credential_resources.credential_factory();
        let writable_state_stores =
            open_runtime_writable_state_stores(&config.state_database_path).await?;
        let selection_state_store =
            AsyncSqliteStateStore::open_read_only(&config.state_database_path).await?;
        let local_model_authentication_required = config.local_token.is_some();
        let auth_gate = match config.local_token {
            Some(local_token) => crate::local_auth::ProxyLocalAuthGate::new(LocalRouterAuth::new(
                local_token,
                Vec::new(),
            )),
            None => crate::local_auth::ProxyLocalAuthGate::disabled(),
        };
        let claude_edge_auth_gate = claude_edge_runtime_config
            .as_ref()
            .map(|claude_edge_config| {
                crate::local_auth::ProxyLocalAuthGate::required(LocalRouterAuth::new(
                    claude_edge_config.local_token.clone(),
                    Vec::new(),
                ))
            });
        let upstream_endpoint = config.upstream_endpoint;
        let upstream = HyperHttpUpstreamTransport::new(upstream_endpoint.clone());
        #[cfg(debug_assertions)]
        let upstream = match config.debug_claude_upstream_endpoint {
            Some(endpoint) => upstream.with_debug_claude_upstream_endpoint(endpoint),
            None => upstream,
        };
        let server = AsyncLoopbackServerRuntime::bind(config.bind_address).await?;
        let audit_sink = config.audit_file_path.map(AuditFileSink::new);
        let websocket_revocations = WebSocketRevocationRegistry::new();
        let route_band_queue_health = RouteBandQueueHealth::default();
        let selection_reservation_lock = SelectionReservationLock::default();
        let db_write_actor = DbWriteActor::start_on_handle(
            caller_runtime,
            Arc::new(SqliteDbWriteRepository::new(
                writable_state_stores.db_write_state_store.clone(),
            )),
            Arc::clone(&route_band_queue_health),
            PROVIDER_EXHAUSTION_QUEUE_CAPACITY,
        );
        let affinity_owner_recorder =
            Arc::new(DbWriteAffinityOwnerRecorder::new(db_write_actor.clone()));
        let maintenance_actor = MaintenanceActor::start_on_handle(
            caller_runtime,
            Arc::new(writable_state_stores.maintenance_state_store.clone()),
            MAINTENANCE_QUEUE_CAPACITY,
        );
        #[cfg(test)]
        if let Some(completion_sender) = completion_sender {
            maintenance_actor.register_completion_sender(completion_sender);
        }

        let loopback_runtime = Self {
            caller_dispatcher,
            server,
            credential_state_store: writable_state_stores.credential_state_store,
            provider_error_state_store: writable_state_stores.db_write_state_store,
            selection_state_store,
            credential_store_availability,
            affinity_secret_provider,
            affinity_owner_recorder,
            auth_gate,
            claude_edge_auth_gate,
            claude_edge_runtime_config,
            local_model_authentication_required,
            upstream,
            upstream_endpoint,
            websocket_revocations,
            audit_sink,
            weighted_selectors: Default::default(),
            account_holds: Default::default(),
            active_reservations: Default::default(),
            selection_reservation_lock,
            session_affinity_cache,
            claude_five_hour_reserve_percent: config.claude_five_hour_reserve_percent,
            runtime_exhaustions: Default::default(),
            route_band_queue_health,
            db_write_actor,
            maintenance_actor,
            last_session_affinity_cleanup_utc_day: AtomicU64::new(u64::MAX),
            credential_factory,
            fixed_now_unix_seconds,
            connection_error_reporter: Arc::new(StderrLoopbackConnectionErrorReporter),
            credential_refresh_shutdown_drain: Duration::from_secs(30),
        };
        loopback_runtime.enqueue_runtime_maintenance_hints(
            fixed_now_unix_seconds.unwrap_or_else(|| current_unix_seconds().unwrap_or(0)),
        );
        Ok(loopback_runtime)
    }

    /// Returns the active loopback address.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.server.local_addr()
    }

    /// Returns a small handle that can reload local auth while the runtime is serving.
    #[must_use]
    pub fn local_auth_reloader(&self) -> LocalAuthReloader {
        LocalAuthReloader {
            auth_gate: self.auth_gate.clone(),
            claude_edge_auth_gate: self.claude_edge_auth_gate.clone(),
            codex_local_token_authentication_required: self.local_model_authentication_required,
            websocket_revocations: self.websocket_revocations.clone(),
        }
    }

    /// Returns redacted WebSocket registry counters for runtime proof.
    #[must_use]
    pub fn websocket_registry_snapshot(&self) -> WebSocketRegistrySnapshot {
        self.websocket_revocations.snapshot()
    }

    /// Returns a narrow handle for reconnecting sessions whose account reached its quota floor.
    #[must_use]
    pub fn websocket_quota_floor_notifier(&self) -> WebSocketQuotaFloorNotifier {
        WebSocketQuotaFloorNotifier::new(self.websocket_revocations.clone())
    }

    /// Replaces local auth and closes WebSocket connections authenticated with old generations.
    pub fn reload_local_auth(
        &self,
        current: LocalRouterTokenRecord,
        previous: Vec<LocalRouterTokenRecord>,
    ) {
        self.local_auth_reloader()
            .reload_local_auth(current, previous);
    }
}
