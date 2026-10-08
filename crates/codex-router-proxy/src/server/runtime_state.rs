use super::*;

/// Assembled loopback router runtime for HTTP/SSE forwarding.
pub struct LoopbackRouterRuntime {
    pub(super) caller_dispatcher: tracing::dispatcher::Dispatch,
    pub(super) server: AsyncLoopbackServerRuntime,
    pub(super) credential_state_store: AsyncSqliteStateStore,
    pub(super) provider_error_state_store: AsyncSqliteStateStore,
    pub(super) selection_state_store: AsyncSqliteStateStore,
    pub(super) credential_store_availability: CredentialStoreAvailability,
    pub(super) credential_factory: AsyncProxyCredentialResolverFactory,
    pub(super) affinity_secret_provider: RuntimeAffinitySecretProvider,
    pub(super) affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    pub(super) auth_gate: crate::local_auth::ProxyLocalAuthGate,
    pub(super) claude_edge_auth_gate: Option<crate::local_auth::ProxyLocalAuthGate>,
    pub(super) claude_edge_runtime_config: Option<ClaudeEdgeRuntimeConfig>,
    pub(super) local_model_authentication_required: bool,
    pub(super) upstream: HyperHttpUpstreamTransport,
    pub(super) upstream_endpoint: UpstreamEndpoint,
    pub(super) websocket_revocations: WebSocketRevocationRegistry,
    pub(super) audit_sink: Option<AuditFileSink>,
    pub(super) weighted_selectors: RouteBandWeightedSelectors,
    pub(super) account_holds: RouteBandAccountHolds,
    pub(super) active_reservations: RouteBandReservationBooks,
    pub(super) selection_reservation_lock: SelectionReservationLock,
    pub(super) session_affinity_cache: SharedSessionAccountAffinityCache,
    pub(super) claude_five_hour_reserve_percent: ClaudeFiveHourReservePercent,
    pub(super) runtime_exhaustions: RouteBandRuntimeExhaustions,
    pub(super) route_band_queue_health: RouteBandQueueHealth,
    pub(super) db_write_actor: DbWriteActor,
    pub(super) maintenance_actor: MaintenanceActor,
    pub(super) last_session_affinity_cleanup_utc_day: AtomicU64,
    pub(super) fixed_now_unix_seconds: Option<u64>,
    pub(super) connection_error_reporter: Arc<dyn LoopbackConnectionErrorReporter>,
    pub(super) credential_refresh_shutdown_drain: Duration,
}

impl Drop for LoopbackRouterRuntime {
    fn drop(&mut self) {
        self.db_write_actor.request_shutdown();
        self.maintenance_actor.request_shutdown();
    }
}
