use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
use std::time::Duration;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProxyLocalTokenPolicy {
    Optional,
    Required,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProxyQuotaRefreshPolicy {
    Disabled,
    Enabled,
}
/// Existing Serve configuration projected without parsing or presentation ownership.
#[derive(Clone, Debug)]
pub struct ProxyRoleConfig {
    pub core: LoopbackRouterRuntimeConfig,
    pub local_token: ProxyLocalTokenPolicy,
    pub quota_refresh: ProxyQuotaRefreshPolicy,
    pub quota_refresh_interval: Duration,
    pub max_connections: usize,
}
