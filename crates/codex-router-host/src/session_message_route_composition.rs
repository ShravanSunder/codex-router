//! Compose client-specific message routes behind one Host-owned router.
use crate::{ClaudeCodePeerDeliveryRoute, ExternalProviderSupervisor, ProviderAcpDeliveryRoute};
use claude_code_peer_messaging::{ClaudeCodePeerSocket, ClaudeCodeSessionRegistry};
use collaboration_protocol::{EndpointId, EndpointRef};
use collaboration_service::{
    EndpointDirectory, NativeControlBackend, ProviderOperationStore, SessionDeliveryRoute,
    SessionDeliveryRouter,
};
use std::{collections::HashSet, io, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

pub(crate) struct SessionMessageRouteComposition {
    pub(crate) router: Arc<SessionDeliveryRouter>,
    pub(crate) provider_route: Option<Arc<ProviderAcpDeliveryRoute>>,
}

pub(crate) struct SessionMessageRouteInputs {
    pub(crate) provider_endpoints: HashSet<EndpointRef>,
    pub(crate) directory: EndpointDirectory,
    pub(crate) native_backend: NativeControlBackend,
    pub(crate) unmaterialized_threads: Arc<collaboration_service::UnmaterializedThreadHolder>,
    pub(crate) provider_supervisor: Option<Arc<ExternalProviderSupervisor>>,
    pub(crate) provider_store: Option<Arc<Mutex<ProviderOperationStore>>>,
    pub(crate) peer_registry_directory: PathBuf,
    pub(crate) display_names: collaboration_service::SessionDisplayNameCache,
}

pub(crate) fn compose_session_message_routes(
    inputs: SessionMessageRouteInputs,
) -> io::Result<SessionMessageRouteComposition> {
    let SessionMessageRouteInputs {
        provider_endpoints,
        directory,
        native_backend,
        unmaterialized_threads,
        provider_supervisor,
        provider_store,
        peer_registry_directory,
        display_names,
    } = inputs;
    let service_id = native_backend.endpoint.service_id.clone();
    let codex_route: Arc<dyn SessionDeliveryRoute> = Arc::new(
        collaboration_service::CodexAppServerDeliveryRoute::new(
            service_id.clone(),
            directory.clone(),
            native_backend,
            unmaterialized_threads,
        )
        .with_display_names(display_names.clone()),
    );
    let peer_route = Arc::new(
        ClaudeCodePeerDeliveryRoute::new(
            EndpointRef {
                service_id: service_id.clone(),
                endpoint_id: EndpointId::try_from("claude-local".to_owned())
                    .map_err(io::Error::other)?,
            },
            Arc::new(ClaudeCodeSessionRegistry::new(
                peer_registry_directory.clone(),
            )),
            Arc::new(ClaudeCodePeerSocket::new(peer_registry_directory)),
        )
        .with_display_names(display_names),
    );
    let mut routes = vec![codex_route];
    let provider_route = match (provider_supervisor, provider_store) {
        (Some(supervisor), Some(store)) => {
            let route = Arc::new(ProviderAcpDeliveryRoute::new(
                service_id,
                provider_endpoints,
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
