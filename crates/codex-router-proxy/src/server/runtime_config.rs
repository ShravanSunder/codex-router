use super::*;

/// Runtime configuration for the assembled loopback router.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopbackRouterRuntimeConfig {
    pub(super) bind_address: LoopbackBindAddress,
    pub(super) upstream_endpoint: UpstreamEndpoint,
    #[cfg(debug_assertions)]
    pub(super) debug_claude_upstream_endpoint: Option<ClaudeUpstreamEndpoint>,
    pub(super) state_database_path: PathBuf,
    pub(super) secret_store_root: PathBuf,
    pub(super) local_token: Option<LocalRouterTokenRecord>,
    pub(super) claude_edge_runtime_config: Option<ClaudeEdgeRuntimeConfig>,
    pub(super) fixed_now_unix_seconds: Option<u64>,
    pub(super) max_snapshot_age_seconds: u64,
    pub(super) session_pin_idle_ttl: Duration,
    pub(super) claude_five_hour_reserve_percent: ClaudeFiveHourReservePercent,
    pub(super) audit_file_path: Option<PathBuf>,
    pub(super) websocket_registry_report_file: Option<PathBuf>,
}

impl LoopbackRouterRuntimeConfig {
    #[must_use]
    pub fn state_database_path(&self) -> &Path {
        &self.state_database_path
    }
    #[must_use]
    pub fn secret_store_root(&self) -> &Path {
        &self.secret_store_root
    }
    #[must_use]
    pub const fn bind_address(&self) -> LoopbackBindAddress {
        self.bind_address
    }

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

    pub(super) fn session_account_affinity_cache(&self) -> SharedSessionAccountAffinityCache {
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
