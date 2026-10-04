/// Runtime configuration for the assembled loopback router.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopbackRouterRuntimeConfig {
    bind_address: LoopbackBindAddress,
    upstream_endpoint: UpstreamEndpoint,
    #[cfg(debug_assertions)]
    debug_claude_upstream_endpoint: Option<ClaudeUpstreamEndpoint>,
    state_database_path: PathBuf,
    secret_store_root: PathBuf,
    local_token: Option<LocalRouterTokenRecord>,
    claude_edge_runtime_config: Option<ClaudeEdgeRuntimeConfig>,
    fixed_now_unix_seconds: Option<u64>,
    max_snapshot_age_seconds: u64,
    session_pin_idle_ttl: Duration,
    claude_five_hour_reserve_percent: ClaudeFiveHourReservePercent,
    audit_file_path: Option<PathBuf>,
    websocket_registry_report_file: Option<PathBuf>,
}

/// Receives diagnostics from detached loopback connection tasks.
pub trait LoopbackConnectionErrorReporter: Send + Sync {
    /// Reports one redacted loopback connection diagnostic.
    fn report_connection_error(&self, diagnostic: &str);
}

#[derive(Clone, Debug)]
struct RuntimeWritableStateStores {
    credential_state_store: AsyncSqliteStateStore,
    db_write_state_store: AsyncSqliteStateStore,
    maintenance_state_store: AsyncSqliteStateStore,
}

async fn open_runtime_writable_state_stores(
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StderrLoopbackConnectionErrorReporter;

impl LoopbackConnectionErrorReporter for StderrLoopbackConnectionErrorReporter {
    fn report_connection_error(&self, diagnostic: &str) {
        // The caller renders only fixed severity, class, and reason labels.
        // Retain those details in Router telemetry when a Host captures stderr.
        tracing::warn!(
            event.name = "codex_router.proxy.loopback_connection_error",
            diagnostic = %diagnostic,
            "loopback connection failed"
        );
        eprintln!("{diagnostic}");
    }
}

impl LoopbackRouterRuntimeConfig {
    /// Creates runtime configuration with conservative quota freshness defaults.
    #[must_use]
    pub const fn new(
        bind_address: LoopbackBindAddress,
        upstream_endpoint: UpstreamEndpoint,
        state_database_path: PathBuf,
        secret_store_root: PathBuf,
        local_token: LocalRouterTokenRecord,
    ) -> Self {
        Self {
            bind_address,
            upstream_endpoint,
            #[cfg(debug_assertions)]
            debug_claude_upstream_endpoint: None,
            state_database_path,
            secret_store_root,
            local_token: Some(local_token),
            claude_edge_runtime_config: None,
            fixed_now_unix_seconds: None,
            max_snapshot_age_seconds: 300,
            session_pin_idle_ttl: DEFAULT_SESSION_PIN_IDLE_TTL,
            claude_five_hour_reserve_percent: DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT,
            audit_file_path: None,
            websocket_registry_report_file: None,
        }
    }

    /// Creates runtime configuration without local bearer-token auth.
    #[must_use]
    pub const fn new_tokenless(
        bind_address: LoopbackBindAddress,
        upstream_endpoint: UpstreamEndpoint,
        state_database_path: PathBuf,
        secret_store_root: PathBuf,
    ) -> Self {
        Self {
            bind_address,
            upstream_endpoint,
            #[cfg(debug_assertions)]
            debug_claude_upstream_endpoint: None,
            state_database_path,
            secret_store_root,
            local_token: None,
            claude_edge_runtime_config: None,
            fixed_now_unix_seconds: None,
            max_snapshot_age_seconds: 300,
            session_pin_idle_ttl: DEFAULT_SESSION_PIN_IDLE_TTL,
            claude_five_hour_reserve_percent: DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT,
            audit_file_path: None,
            websocket_registry_report_file: None,
        }
    }

    /// Requires the caller to present a local bearer token before routing.
    #[must_use]
    pub fn with_required_local_token(mut self, local_token: LocalRouterTokenRecord) -> Self {
        self.local_token = Some(local_token);
        self
    }

    /// Requires a local bearer token and freshness interval for Claude edge requests.
    #[must_use]
    pub fn with_claude_edge_local_token(
        mut self,
        local_token: LocalRouterTokenRecord,
        quota_refresh_interval: Duration,
    ) -> Self {
        self.claude_edge_runtime_config = Some(ClaudeEdgeRuntimeConfig {
            local_token,
            quota_refresh_interval,
        });
        self
    }

    /// Applies a validated Claude destination only for an isolated debug runtime.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn with_debug_claude_upstream_endpoint(mut self, endpoint: ClaudeUpstreamEndpoint) -> Self {
        self.debug_claude_upstream_endpoint = Some(endpoint);
        self
    }

    /// Sets the selector's quota freshness clock.
    #[must_use]
    pub const fn with_quota_clock(
        mut self,
        now_unix_seconds: u64,
        max_snapshot_age_seconds: u64,
    ) -> Self {
        self.fixed_now_unix_seconds = Some(now_unix_seconds);
        self.max_snapshot_age_seconds = max_snapshot_age_seconds;
        self
    }

    /// Sets the shared provider session-pin idle lifetime.
    #[must_use]
    pub const fn with_session_pin_idle_ttl(mut self, idle_ttl: Duration) -> Self {
        self.session_pin_idle_ttl = idle_ttl;
        self
    }

    /// Sets the Claude five-hour Reserve threshold used by the route profile.
    #[must_use]
    pub const fn with_claude_five_hour_reserve_percent(
        mut self,
        percent: ClaudeFiveHourReservePercent,
    ) -> Self {
        self.claude_five_hour_reserve_percent = percent;
        self
    }

    fn session_account_affinity_cache(&self) -> SharedSessionAccountAffinityCache {
        SessionAccountAffinityCache::shared(self.session_pin_idle_ttl)
    }

    /// Sets the private audit JSONL file path.
    #[must_use]
    pub fn with_audit_file(mut self, audit_file_path: PathBuf) -> Self {
        self.audit_file_path = Some(audit_file_path);
        self
    }

    /// Sets the internal WebSocket registry JSON report path.
    #[must_use]
    pub fn with_websocket_registry_report_file(mut self, report_file: PathBuf) -> Self {
        self.websocket_registry_report_file = Some(report_file);
        self
    }
}

/// Assembled loopback router runtime for HTTP/SSE forwarding.
pub struct LoopbackRouterRuntime {
    runtime: Option<tokio::runtime::Runtime>,
    caller_dispatcher: tracing::dispatcher::Dispatch,
    server: AsyncLoopbackServerRuntime,
    credential_state_store: AsyncSqliteStateStore,
    provider_error_state_store: AsyncSqliteStateStore,
    selection_state_store: AsyncSqliteStateStore,
    credential_store_availability: CredentialStoreAvailability,
    credential_factory: AsyncProxyCredentialResolverFactory,
    affinity_secret_provider: RuntimeAffinitySecretProvider,
    affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    claude_edge_auth_gate: Option<crate::local_auth::ProxyLocalAuthGate>,
    claude_edge_runtime_config: Option<ClaudeEdgeRuntimeConfig>,
    local_model_authentication_required: bool,
    upstream: HyperHttpUpstreamTransport,
    upstream_endpoint: UpstreamEndpoint,
    websocket_revocations: WebSocketRevocationRegistry,
    audit_sink: Option<AuditFileSink>,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    active_reservations: RouteBandReservationBooks,
    selection_reservation_lock: SelectionReservationLock,
    session_affinity_cache: SharedSessionAccountAffinityCache,
    claude_five_hour_reserve_percent: ClaudeFiveHourReservePercent,
    runtime_exhaustions: RouteBandRuntimeExhaustions,
    route_band_queue_health: RouteBandQueueHealth,
    db_write_actor: DbWriteActor,
    maintenance_actor: MaintenanceActor,
    last_session_affinity_cleanup_utc_day: AtomicU64,
    fixed_now_unix_seconds: Option<u64>,
    connection_error_reporter: Arc<dyn LoopbackConnectionErrorReporter>,
    credential_refresh_shutdown_drain: Duration,
}

impl Drop for LoopbackRouterRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::ZERO);
        }
    }
}

impl LoopbackRouterRuntime {
    /// Opens router-owned state and binds using the process-owned encrypted credential handle.
    pub fn start(
        config: LoopbackRouterRuntimeConfig,
        credential_store: EncryptedCredentialStore,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let caller_dispatcher = tracing::dispatcher::get_default(|dispatcher| dispatcher.clone());
        Self::start_with_test_maintenance_completion_sender(
            config,
            credential_store,
            None,
            caller_dispatcher,
        )
    }

    #[cfg(test)]
    pub(crate) fn start_for_test(
        config: LoopbackRouterRuntimeConfig,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let credential_store = test_credential_store_for_config(&config)?;
        Self::start(config, credential_store)
    }

    #[cfg(test)]
    pub(crate) fn start_for_test_with_maintenance_completion_sender(
        config: LoopbackRouterRuntimeConfig,
        completion_sender: std::sync::mpsc::Sender<crate::maintenance_actor::MaintenanceCompletion>,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let credential_store = test_credential_store_for_config(&config)?;
        Self::start_with_maintenance_completion_sender(config, credential_store, completion_sender)
    }

    #[cfg(test)]
    pub(crate) fn start_with_maintenance_completion_sender(
        config: LoopbackRouterRuntimeConfig,
        credential_store: EncryptedCredentialStore,
        completion_sender: std::sync::mpsc::Sender<crate::maintenance_actor::MaintenanceCompletion>,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let caller_dispatcher = tracing::dispatcher::get_default(|dispatcher| dispatcher.clone());
        Self::start_with_test_maintenance_completion_sender(
            config,
            credential_store,
            Some(completion_sender),
            caller_dispatcher,
        )
    }

    fn start_with_test_maintenance_completion_sender(
        config: LoopbackRouterRuntimeConfig,
        credential_store: EncryptedCredentialStore,
        #[cfg(test)] completion_sender: Option<
            std::sync::mpsc::Sender<crate::maintenance_actor::MaintenanceCompletion>,
        >,
        #[cfg(not(test))] _completion_sender: Option<()>,
        caller_dispatcher: tracing::dispatcher::Dispatch,
    ) -> Result<Self, LoopbackRouterRuntimeError> {
        let session_affinity_cache = config.session_account_affinity_cache();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(LoopbackRouterRuntimeError::TokioRuntime)?;
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
        let writable_state_stores = runtime.block_on(open_runtime_writable_state_stores(
            &config.state_database_path,
        ))?;
        let selection_state_store = runtime.block_on(AsyncSqliteStateStore::open_read_only(
            &config.state_database_path,
        ))?;
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
        let server = runtime.block_on(AsyncLoopbackServerRuntime::bind(config.bind_address))?;
        let audit_sink = config.audit_file_path.map(AuditFileSink::new);
        let websocket_revocations = WebSocketRevocationRegistry::new();
        let route_band_queue_health = RouteBandQueueHealth::default();
        let selection_reservation_lock = SelectionReservationLock::default();
        let db_write_actor = DbWriteActor::start_on_handle(
            runtime.handle(),
            Arc::new(SqliteDbWriteRepository::new(
                writable_state_stores.db_write_state_store.clone(),
            )),
            Arc::clone(&route_band_queue_health),
            PROVIDER_EXHAUSTION_QUEUE_CAPACITY,
        );
        let affinity_owner_recorder =
            Arc::new(DbWriteAffinityOwnerRecorder::new(db_write_actor.clone()));
        let maintenance_actor = MaintenanceActor::start_on_handle(
            runtime.handle(),
            Arc::new(writable_state_stores.maintenance_state_store.clone()),
            MAINTENANCE_QUEUE_CAPACITY,
        );
        #[cfg(test)]
        if let Some(completion_sender) = completion_sender {
            maintenance_actor.register_completion_sender(completion_sender);
        }

        let loopback_runtime = Self {
            runtime: Some(runtime),
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

    /// Serves a bounded number of HTTP/SSE connections.
    #[cfg(test)]
    pub fn serve_http_connections(
        &self,
        max_connections: usize,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        self.serve_protocol_connections(max_connections)
    }

    /// Serves a bounded number of HTTP/SSE or WebSocket connections.
    pub fn serve_protocol_connections(
        &self,
        max_connections: usize,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        self.runtime
            .as_ref()
            .ok_or_else(|| {
                LoopbackRouterRuntimeError::TokioRuntime(std::io::Error::other(
                    "router runtime unavailable",
                ))
            })?
            .block_on(
                self.serve_protocol_connections_async(max_connections, None)
                    .with_subscriber(self.caller_dispatcher.clone()),
            )
    }

    /// Serves HTTP/SSE or WebSocket connections until the bound or cancellation.
    pub fn serve_protocol_connections_until_cancelled(
        &self,
        max_connections: usize,
        shutdown: CancellationToken,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        self.runtime
            .as_ref()
            .ok_or_else(|| {
                LoopbackRouterRuntimeError::TokioRuntime(std::io::Error::other(
                    "router runtime unavailable",
                ))
            })?
            .block_on(
                self.serve_protocol_connections_async(max_connections, Some(shutdown))
                    .with_subscriber(self.caller_dispatcher.clone()),
            )
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_connection_error_reporter(
        mut self,
        reporter: Arc<dyn LoopbackConnectionErrorReporter>,
    ) -> Self {
        self.connection_error_reporter = reporter;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_credential_refresh_shutdown_drain(mut self, limit: Duration) -> Self {
        self.credential_refresh_shutdown_drain = limit;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_test_claude_refresh_client<C>(mut self, refresh_client: C) -> Self
    where
        C: codex_router_auth::resolver::CredentialRefreshClient + Clone + Send + Sync + 'static,
    {
        self.credential_factory
            .set_test_claude_refresh_client(refresh_client);
        self
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_affinity_owner_recorder(
        mut self,
        recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    ) -> Self {
        self.affinity_owner_recorder = recorder;
        self
    }

    async fn serve_protocol_connections_async(
        &self,
        max_connections: usize,
        shutdown: Option<CancellationToken>,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let mut handled_connections = 0_usize;
        let mut handlers = JoinSet::new();
        let mut first_connection_error = None;
        let session_shutdown = shutdown.clone().unwrap_or_default();
        let affinity_record_tasks = TaskTracker::new();
        let connection_handler =
            Arc::new(self.protocol_connection_handler(
                session_shutdown.clone(),
                affinity_record_tasks.clone(),
            ));
        while handled_connections < max_connections {
            let stream = if let Some(shutdown) = shutdown.as_ref() {
                loop {
                    tokio::select! {
                        () = shutdown.cancelled() => break None,
                        joined = handlers.join_next(), if !handlers.is_empty() => {
                            if store_optional_connection_join_error(
                                &mut first_connection_error,
                                joined,
                            ) {
                                session_shutdown.cancel();
                                break None;
                            }
                        }
                        accepted = self.server.listener.accept() => {
                            let (stream, _peer_addr) = accepted.map_err(LoopbackRouterRuntimeError::Accept)?;
                            break Some(stream);
                        }
                    }
                }
            } else {
                loop {
                    tokio::select! {
                        joined = handlers.join_next(), if !handlers.is_empty() => {
                            if store_optional_connection_join_error(
                                &mut first_connection_error,
                                joined,
                            ) {
                                session_shutdown.cancel();
                                break None;
                            }
                        }
                        accepted = self.server.listener.accept() => {
                            let (stream, _peer_addr) = accepted.map_err(LoopbackRouterRuntimeError::Accept)?;
                            break Some(stream);
                        }
                    }
                }
            };
            let Some(stream) = stream else {
                break;
            };
            let handler_context = Arc::clone(&connection_handler);
            let handler = tokio::spawn(
                async move { handler_context.handle_hyper_connection(stream).await }
                    .with_subscriber(self.caller_dispatcher.clone()),
            );
            if max_connections == usize::MAX && shutdown.is_none() {
                supervise_detached_connection_handler(
                    handler,
                    Arc::clone(&self.connection_error_reporter),
                );
            } else {
                handlers.spawn(async move {
                    handler
                        .await
                        .map_err(LoopbackRouterRuntimeError::ConnectionJoin)?
                });
            }
            handled_connections += 1;
            self.enqueue_runtime_maintenance_hints(
                self.fixed_now_unix_seconds
                    .unwrap_or_else(|| current_unix_seconds().unwrap_or(0)),
            );
        }

        if first_connection_error.is_some()
            || matches!(shutdown.as_ref(), Some(shutdown) if shutdown.is_cancelled())
        {
            session_shutdown.cancel();
        }

        while let Some(joined) = handlers.join_next().await {
            store_connection_join_error(&mut first_connection_error, joined);
        }
        affinity_record_tasks.close();
        affinity_record_tasks.wait().await;
        if !self
            .credential_factory
            .drain_refresh_tasks(self.credential_refresh_shutdown_drain)
            .await
        {
            tracing::warn!(
                "credential refresh drain timed out; unresolved claims remain authoritative"
            );
        }
        self.db_write_actor.shutdown().await;
        self.maintenance_actor.shutdown().await;

        match first_connection_error {
            Some(error) => Err(error),
            None => Ok(handled_connections),
        }
    }

    fn protocol_connection_handler(
        &self,
        session_shutdown: CancellationToken,
        affinity_record_tasks: TaskTracker,
    ) -> LoopbackProtocolConnectionHandler {
        LoopbackProtocolConnectionHandler {
            credential_state_store: self.credential_state_store.clone(),
            provider_error_state_store: self.provider_error_state_store.clone(),
            selection_state_store: self.selection_state_store.clone(),
            credential_store_availability: self.credential_store_availability,
            credential_factory: self.credential_factory.clone(),
            affinity_secret_provider: self.affinity_secret_provider.clone(),
            affinity_owner_recorder: Arc::clone(&self.affinity_owner_recorder),
            affinity_record_tasks,
            auth_gate: self.auth_gate.clone(),
            claude_edge_auth_gate: self.claude_edge_auth_gate.clone(),
            claude_edge_runtime_config: self.claude_edge_runtime_config.clone(),
            local_model_authentication_required: self.local_model_authentication_required,
            upstream: self.upstream.clone(),
            upstream_endpoint: self.upstream_endpoint.clone(),
            websocket_revocations: self.websocket_revocations.clone(),
            audit_sink: self.audit_sink.clone(),
            weighted_selectors: Arc::clone(&self.weighted_selectors),
            account_holds: Arc::clone(&self.account_holds),
            active_reservations: Arc::clone(&self.active_reservations),
            selection_reservation_lock: Arc::clone(&self.selection_reservation_lock),
            session_affinity_cache: Arc::clone(&self.session_affinity_cache),
            claude_five_hour_reserve_percent: self.claude_five_hour_reserve_percent,
            runtime_exhaustions: Arc::clone(&self.runtime_exhaustions),
            route_band_queue_health: Arc::clone(&self.route_band_queue_health),
            db_write_actor: self.db_write_actor.clone(),
            fixed_now_unix_seconds: self.fixed_now_unix_seconds,
            session_shutdown,
        }
    }

    fn enqueue_runtime_maintenance_hints(&self, now_unix_seconds: u64) {
        const ROLLUP_BUCKET_SECONDS: u64 = 300;
        const ACTIVE_CLIENT_STALE_AFTER_SECONDS: u64 = 600;
        const ACTIVE_SESSION_RETENTION_SECONDS: u64 = 86_400;
        const SESSION_ACCOUNT_AFFINITY_RETENTION_SECONDS: u64 = 7 * 86_400;

        if claim_session_affinity_cleanup_day(
            &self.last_session_affinity_cleanup_utc_day,
            now_unix_seconds,
        ) {
            let _cleanup_result = self.maintenance_actor.try_enqueue(
                MaintenanceHint::CleanupStaleSessionAccountAffinities {
                    stale_before_unix_seconds: now_unix_seconds
                        .saturating_sub(SESSION_ACCOUNT_AFFINITY_RETENTION_SECONDS),
                },
            );
        }

        let interval_start_unix_seconds =
            now_unix_seconds.saturating_sub(now_unix_seconds % ROLLUP_BUCKET_SECONDS);
        let interval_end_unix_seconds = interval_start_unix_seconds + ROLLUP_BUCKET_SECONDS;
        for route_band in [
            RouteBand::Responses,
            RouteBand::ResponsesCompact,
            RouteBand::Models,
            RouteBand::MemoriesTraceSummarize,
        ] {
            let _cleanup_result =
                self.maintenance_actor
                    .try_enqueue(MaintenanceHint::CleanupStaleActiveClients {
                        route_band,
                        stale_before_unix_seconds: now_unix_seconds
                            .saturating_sub(ACTIVE_CLIENT_STALE_AFTER_SECONDS),
                    });
            let _rollup_result =
                self.maintenance_actor
                    .try_enqueue(MaintenanceHint::RefreshActiveSessionRollups {
                        route_band,
                        interval_start_unix_seconds,
                        interval_end_unix_seconds,
                        bucket_seconds: ROLLUP_BUCKET_SECONDS,
                    });
            let _retention_result =
                self.maintenance_actor
                    .try_enqueue(MaintenanceHint::ApplyActiveSessionRetention {
                        route_band,
                        retain_after_unix_seconds: now_unix_seconds
                            .saturating_sub(ACTIVE_SESSION_RETENTION_SECONDS),
                    });
            let _compaction_result =
                self.maintenance_actor
                    .try_enqueue(MaintenanceHint::CompactActiveSessionHistory {
                        route_band,
                        compact_before_unix_seconds: active_session_event_compaction_before(
                            now_unix_seconds,
                        ),
                    });
        }
    }
}
