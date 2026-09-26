//! Compose client-specific message routes behind one Host-owned router.
use crate::{ClaudeCodePeerDeliveryRoute, ExternalProviderSupervisor, ProviderAcpDeliveryRoute};
use claude_code_peer_messaging::{ClaudeCodePeerSocket, ClaudeCodeSessionRegistry};
use collaboration_protocol::UuidIdentity;
use collaboration_service::{
    EndpointDirectory, NativeControlBackend, ProviderOperationStore, SessionDeliveryRoute,
    SessionDeliveryRouter,
};
use std::{io, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

pub(crate) struct SessionMessageRouteComposition {
    pub(crate) router: Arc<SessionDeliveryRouter>,
    pub(crate) provider_route: Option<Arc<ProviderAcpDeliveryRoute>>,
}

pub(crate) fn compose_session_message_routes(
    service_id: UuidIdentity,
    directory: EndpointDirectory,
    native_backend: NativeControlBackend,
    unmaterialized_threads: Arc<collaboration_service::UnmaterializedThreadHolder>,
    provider_supervisor: Option<Arc<ExternalProviderSupervisor>>,
    provider_store: Option<Arc<Mutex<ProviderOperationStore>>>,
    peer_registry_directory: PathBuf,
) -> io::Result<SessionMessageRouteComposition> {
    let codex_route: Arc<dyn SessionDeliveryRoute> =
        Arc::new(collaboration_service::CodexAppServerDeliveryRoute::new(
            service_id.clone(),
            directory.clone(),
            native_backend,
            unmaterialized_threads,
        ));
    let peer_route = Arc::new(ClaudeCodePeerDeliveryRoute::new(
        service_id.clone(),
        Arc::new(ClaudeCodeSessionRegistry::new(
            peer_registry_directory.clone(),
        )),
        Arc::new(ClaudeCodePeerSocket::new(peer_registry_directory)),
    ));
    let mut routes = vec![codex_route];
    let provider_route = match (provider_supervisor, provider_store) {
        (Some(supervisor), Some(store)) => {
            let route = Arc::new(ProviderAcpDeliveryRoute::new(
                service_id,
                directory,
                supervisor,
                store,
                peer_route.clone(),
            ));
            routes.push(route.clone());
            Some(route)
        }
        (None, _) => None,
        (Some(_), None) => {
            return Err(io::Error::other(
                "provider delivery requires the provider operation store",
            ));
        }
    };
    routes.push(peer_route);
    Ok(SessionMessageRouteComposition {
        router: Arc::new(SessionDeliveryRouter::new(routes)),
        provider_route,
    })
}

pub(crate) fn default_peer_registry_directory() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("HOME is required for Claude Code peer discovery"))?;
    Ok(home.join(".claude/sessions"))
}
