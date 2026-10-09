use super::runtime_preparation::PreparedLoopbackRouterRuntime;
use super::runtime_startup::open_runtime_writable_state_stores;
use super::*;
impl PreparedLoopbackRouterRuntime {
    /// Revalidates and migrates state before starting actors or admitting traffic.
    pub async fn activate(self) -> Result<LoopbackRouterRuntime, LoopbackRouterRuntimeError> {
        let Self {
            config,
            server,
            credential_store_availability,
            credential_factory,
            affinity_secret_provider,
            auth_gate,
            claude_edge_auth_gate,
            upstream,
            session_affinity_cache,
            caller_runtime,
            caller_dispatcher,
        } = self;
        if config
            .state_database_path
            .try_exists()
            .map_err(LoopbackRouterRuntimeError::StateInspection)?
        {
            AsyncSqliteStateStore::prepare_schema(&config.state_database_path).await?;
        }
        let writable_state_stores =
            open_runtime_writable_state_stores(&config.state_database_path).await?;
        let selection_state_store =
            AsyncSqliteStateStore::open_read_only(&config.state_database_path).await?;
        let fixed_now_unix_seconds = config.fixed_now_unix_seconds;
        let claude_edge_runtime_config = config.claude_edge_runtime_config.clone();
        let local_model_authentication_required = config.local_token.is_some();
        let upstream_endpoint = config.upstream_endpoint;
        let audit_sink = config.audit_file_path.map(AuditFileSink::new);
        let websocket_revocations = WebSocketRevocationRegistry::new();
        let route_band_queue_health = RouteBandQueueHealth::default();
        let selection_reservation_lock = SelectionReservationLock::default();
        let db_write_actor = DbWriteActor::start_on_handle(
            &caller_runtime,
            Arc::new(SqliteDbWriteRepository::new(
                writable_state_stores.db_write_state_store.clone(),
            )),
            Arc::clone(&route_band_queue_health),
            PROVIDER_EXHAUSTION_QUEUE_CAPACITY,
        );
        let affinity_owner_recorder =
            Arc::new(DbWriteAffinityOwnerRecorder::new(db_write_actor.clone()));
        let maintenance_actor = MaintenanceActor::start_on_handle(
            &caller_runtime,
            Arc::new(writable_state_stores.maintenance_state_store.clone()),
            MAINTENANCE_QUEUE_CAPACITY,
        );

        let loopback_runtime = LoopbackRouterRuntime {
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
}
