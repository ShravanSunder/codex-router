use super::*;
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_descriptor_boundary::{DescriptorGate, OwnedListener};
/// In-memory, non-serving proxy prepared against an already-granted listener.
/// Activation consumes this owner; no actors or writable state handles exist yet.
pub struct PreparedLoopbackRouterRuntime {
    pub(super) config: LoopbackRouterRuntimeConfig,
    pub(super) server: AsyncLoopbackServerRuntime,
    pub(super) credential_store_availability: CredentialStoreAvailability,
    pub(super) credential_factory: AsyncProxyCredentialResolverFactory,
    pub(super) affinity_secret_provider: RuntimeAffinitySecretProvider,
    pub(super) auth_gate: crate::local_auth::ProxyLocalAuthGate,
    pub(super) claude_edge_auth_gate: Option<crate::local_auth::ProxyLocalAuthGate>,
    pub(super) upstream: HyperHttpUpstreamTransport,
    pub(super) session_affinity_cache: SharedSessionAccountAffinityCache,
    pub(super) caller_runtime: tokio::runtime::Handle,
    pub(super) caller_dispatcher: tracing::dispatcher::Dispatch,
}
impl LoopbackRouterRuntime {
    /// Builds memory-only resources and adopts an owned listener without rebinding or accepting.
    pub async fn prepare(
        config: LoopbackRouterRuntimeConfig,
        credential_store: EncryptedCredentialStore,
        affinity_secret: RouterAffinityHashSecret,
        listener: OwnedListener,
        gate: &DescriptorGate,
    ) -> Result<PreparedLoopbackRouterRuntime, LoopbackRouterRuntimeError> {
        let caller_runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            LoopbackRouterRuntimeError::TokioRuntime(std::io::Error::other(error))
        })?;
        let caller_dispatcher = tracing::dispatcher::get_default(|dispatcher| dispatcher.clone());
        let session_affinity_cache = config.session_account_affinity_cache();
        let credential_store_availability = match credential_store.status() {
            EncryptedCredentialStoreStatus::Ready => CredentialStoreAvailability::Available,
            EncryptedCredentialStoreStatus::KeyUnavailable => {
                CredentialStoreAvailability::KeyUnreadable
            }
            EncryptedCredentialStoreStatus::MigrationIncomplete { .. } => {
                CredentialStoreAvailability::MigrationIncomplete
            }
        };
        let resources = ProxyRuntimeCredentialResources::from_preloaded(
            credential_store,
            config.fixed_now_unix_seconds,
            affinity_secret,
        );
        let auth_gate = match config.local_token.clone() {
            Some(token) => {
                crate::local_auth::ProxyLocalAuthGate::new(LocalRouterAuth::new(token, Vec::new()))
            }
            None => crate::local_auth::ProxyLocalAuthGate::disabled(),
        };
        let claude_edge_auth_gate = config.claude_edge_runtime_config.as_ref().map(|config| {
            crate::local_auth::ProxyLocalAuthGate::required(LocalRouterAuth::new(
                config.local_token.clone(),
                Vec::new(),
            ))
        });
        let upstream = HyperHttpUpstreamTransport::new(config.upstream_endpoint.clone());
        #[cfg(debug_assertions)]
        let upstream = match config.debug_claude_upstream_endpoint.clone() {
            Some(endpoint) => upstream.with_debug_claude_upstream_endpoint(endpoint),
            None => upstream,
        };
        let server =
            AsyncLoopbackServerRuntime::from_granted(listener, config.bind_address, gate).await?;
        Ok(PreparedLoopbackRouterRuntime {
            config,
            server,
            credential_store_availability,
            credential_factory: resources.credential_factory(),
            affinity_secret_provider: resources.affinity_secret_provider(),
            auth_gate,
            claude_edge_auth_gate,
            upstream,
            session_affinity_cache,
            caller_runtime,
            caller_dispatcher,
        })
    }
}
impl PreparedLoopbackRouterRuntime {
    #[must_use]
    pub fn credential_refresh_task_supervisor(&self) -> CredentialRefreshTaskSupervisor {
        self.credential_factory.credential_refresh_task_supervisor()
    }
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.server.local_addr()
    }
}
