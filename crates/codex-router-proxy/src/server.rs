//! Loopback-only server runtime primitives.

use std::collections::VecDeque;
use std::convert::Infallible;
#[cfg(test)]
use std::io::Read;
#[cfg(test)]
use std::io::Write;
use std::net::AddrParseError;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
#[cfg(test)]
use std::net::TcpListener;
#[cfg(test)]
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use bytes::Bytes;
use futures_util::future::BoxFuture;
use http::HeaderMap;
use http::Method as HttpMethod;
use http::Request as HttpRequest;
use http::Response as HttpResponse;
use http::StatusCode;
use http::Uri;
use http_body_util::BodyExt;
use http_body_util::Empty;
use http_body_util::Full;
use http_body_util::combinators::BoxBody;
use hyper::body::Body as HyperBody;
use hyper::body::Frame;
use hyper::body::Incoming;
use hyper::body::SizeHint;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener as TokioTcpListener;
use tokio::task::JoinError;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use codex_router_core::affinity::hash_previous_response_id;
use codex_router_core::audit::AuditFileSink;
use codex_router_core::audit::RouteKind as AuditRouteKind;
use codex_router_core::audit::TransportKind;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::local_auth::LocalRouterAuth;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::routes::RouteBand;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::affinity_owner::AffinitySourceTransport;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;

use crate::account_selection::AsyncAccountSelectorRuntimeState;
use crate::account_selection::AsyncRepositoryBackedAccountSelector;
use crate::account_selection::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS;
use crate::account_selection::RouteBandAccountHolds;
use crate::account_selection::RouteBandQueueHealth;
use crate::account_selection::RouteBandReservationBooks;
use crate::account_selection::RouteBandRuntimeExhaustions;
use crate::account_selection::RouteBandWeightedSelectors;
use crate::account_selection::SelectionReservationLock;
use crate::account_selection::SqliteActiveClientLeaseReporter;
use crate::account_selection::mark_runtime_quota_exhausted;
use crate::account_selection::route_band_post_exhaustion_outcome;
use crate::credential_runtime::AsyncProxyCredentialResolverFactory;
use crate::credential_runtime::ProxyRuntimeCredentialResources;
use crate::credential_runtime::ProxyRuntimeCredentialResourcesOpenError;
use crate::credential_runtime::RuntimeAffinitySecretProvider;
use crate::db_write_actor::DbWriteActor;
use crate::db_write_actor::DbWriteCommand;
use crate::db_write_actor::DbWriteEnqueueResult;
use crate::db_write_actor::PROVIDER_EXHAUSTION_QUEUE_CAPACITY;
use crate::db_write_actor::SqliteDbWriteRepository;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::http_sse::AsyncHttpAffinityOwnerRecorder;
use crate::http_sse::AsyncHttpBodyError;
use crate::http_sse::AsyncStreamingHttpProxyResponse;
use crate::http_sse::AsyncStreamingUpstreamHttpTransport;
use crate::http_sse::AuthenticatedHttpProxyService;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
#[cfg(test)]
use crate::http_sse::HttpProxyResponse;
#[cfg(test)]
use crate::http_sse::HttpRequestHandler;
use crate::http_sse::PreparedAsyncStreamingHttpProxyRequest;
use crate::http_sse::StderrAuditFailureReporter;
use crate::http_sse::StreamingHttpProxyCompletion;
#[cfg(test)]
use crate::http_sse::StreamingHttpProxyResponse;
#[cfg(test)]
use crate::http_sse::StreamingHttpRequestHandler;
use crate::http_sse::append_audit_event_with_reporter;
use crate::http_sse::extract_response_id_from_body;
use crate::http_sse::local_auth_rejection_audit_event;
use crate::local_auth::extract_presented_local_token_from_request;
use crate::maintenance_actor::MAINTENANCE_QUEUE_CAPACITY;
use crate::maintenance_actor::MaintenanceActor;
use crate::maintenance_actor::MaintenanceHint;
use crate::provider_error::AsyncProviderErrorObserver;
use crate::provider_error::ProviderErrorClassification;
use crate::provider_error::ProviderErrorObservationError;
use crate::provider_error::classify_provider_error_envelope;
use crate::provider_error::record_provider_error_observation;
use crate::routes::Method;
use crate::routes::RouteClass;
use crate::routes::classify_route;
use crate::upstream::HyperHttpUpstreamTransport;
use crate::upstream::UpstreamEndpoint;
use crate::websocket::AsyncWebSocketTunnel;
use crate::websocket::WebSocketHandshakeRequest;
use crate::websocket::WebSocketProtocolRouter;
use crate::websocket::WebSocketQuotaFloorNotifier;
use crate::websocket::WebSocketRegistrySnapshot;
use crate::websocket::WebSocketRevocationRegistry;
use crate::websocket::router_websocket_config;

#[cfg(test)]
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;

/// Address validated for the v1 loopback-only proxy server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopbackBindAddress {
    host: IpAddr,
    port: u16,
}

impl LoopbackBindAddress {
    /// Creates a bind address after rejecting non-loopback hosts.
    pub fn new(host: impl AsRef<str>, port: u16) -> Result<Self, ServerBindError> {
        let host_text = host.as_ref();
        let host_address = parse_loopback_candidate(host_text)?;

        if !host_address.is_loopback() {
            return Err(ServerBindError::NonLoopback {
                host: host_text.to_owned(),
            });
        }

        Ok(Self {
            host: host_address,
            port,
        })
    }

    /// Returns the socket address used for binding.
    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

fn parse_loopback_candidate(host: &str) -> Result<IpAddr, ServerBindError> {
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    host.parse::<IpAddr>()
        .map_err(|source| ServerBindError::InvalidHost {
            host: host.to_owned(),
            source,
        })
}

/// Bound loopback listener kept alive by the router runtime.
#[cfg(test)]
#[derive(Debug)]
pub struct LoopbackServerRuntime {
    listener: TcpListener,
    local_addr: SocketAddr,
}

#[cfg(test)]
impl LoopbackServerRuntime {
    /// Binds a TCP listener to a validated loopback address.
    pub fn bind(address: LoopbackBindAddress) -> Result<Self, ServerBindError> {
        let socket_addr = address.socket_addr();
        let listener = TcpListener::bind(socket_addr).map_err(|source| ServerBindError::Bind {
            address: socket_addr,
            source,
        })?;
        let local_addr = listener
            .local_addr()
            .map_err(|source| ServerBindError::Bind {
                address: socket_addr,
                source,
            })?;

        Ok(Self {
            listener,
            local_addr,
        })
    }

    /// Returns the actual local address, including kernel-assigned port.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Returns the bound listener.
    #[must_use]
    pub fn listener(&self) -> &TcpListener {
        &self.listener
    }
}

/// Tokio-owned loopback listener substrate for the async release runtime.
///
/// This is intentionally only the T1 listener/task shell. HTTP/SSE routing,
/// WebSocket upgrade handling, and pump behavior are cut over in later slices.
#[derive(Debug)]
pub struct AsyncLoopbackServerRuntime {
    listener: TokioTcpListener,
    local_addr: SocketAddr,
}

impl AsyncLoopbackServerRuntime {
    /// Binds a Tokio TCP listener to a validated loopback address.
    pub async fn bind(address: LoopbackBindAddress) -> Result<Self, ServerBindError> {
        let socket_addr = address.socket_addr();
        let listener = TokioTcpListener::bind(socket_addr)
            .await
            .map_err(|source| ServerBindError::Bind {
                address: socket_addr,
                source,
            })?;
        let local_addr = listener
            .local_addr()
            .map_err(|source| ServerBindError::Bind {
                address: socket_addr,
                source,
            })?;

        Ok(Self {
            listener,
            local_addr,
        })
    }

    /// Returns the actual local address, including kernel-assigned port.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Runs the async accept shell until cancellation.
    ///
    /// T1 accepts and immediately drops streams because the Hyper service,
    /// HTTP/SSE body forwarding, and WebSocket pumps are later plan slices.
    pub async fn serve_until_cancelled(
        self,
        shutdown: CancellationToken,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let mut handled_connections = 0_usize;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return Ok(handled_connections),
                accepted = self.listener.accept() => {
                    let (_stream, _peer_addr) = accepted
                        .map_err(LoopbackRouterRuntimeError::Accept)?;
                    handled_connections += 1;
                }
            }
        }
    }
}

/// First routing decision made by the future Hyper service switchpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HyperProtocolDispatch {
    /// Ordinary HTTP/SSE request path.
    Http,
    /// WebSocket upgrade request path.
    WebSocketUpgrade,
}

/// Shared Hyper request switchpoint for HTTP/SSE and WebSocket paths.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HyperProtocolSwitchpoint;

impl HyperProtocolSwitchpoint {
    /// Classifies a Hyper request head without consuming or buffering the body.
    #[must_use]
    pub fn classify(
        _method: &HttpMethod,
        _uri: &Uri,
        headers: &HeaderMap,
    ) -> HyperProtocolDispatch {
        if is_websocket_upgrade(headers) {
            HyperProtocolDispatch::WebSocketUpgrade
        } else {
            HyperProtocolDispatch::Http
        }
    }
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    let has_upgrade_header = headers
        .get(http::header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let has_connection_upgrade = headers
        .get(http::header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
        });

    has_upgrade_header && has_connection_upgrade
}

/// Runtime configuration for the assembled loopback router.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopbackRouterRuntimeConfig {
    bind_address: LoopbackBindAddress,
    upstream_endpoint: UpstreamEndpoint,
    state_database_path: PathBuf,
    secret_store_root: PathBuf,
    local_token: Option<LocalRouterTokenRecord>,
    fixed_now_unix_seconds: Option<u64>,
    max_snapshot_age_seconds: u64,
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
            state_database_path,
            secret_store_root,
            local_token: Some(local_token),
            fixed_now_unix_seconds: None,
            max_snapshot_age_seconds: 300,
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
            state_database_path,
            secret_store_root,
            local_token: None,
            fixed_now_unix_seconds: None,
            max_snapshot_age_seconds: 300,
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
    runtime: tokio::runtime::Runtime,
    server: AsyncLoopbackServerRuntime,
    credential_state_store: AsyncSqliteStateStore,
    provider_error_state_store: AsyncSqliteStateStore,
    selection_state_store: AsyncSqliteStateStore,
    credential_factory: AsyncProxyCredentialResolverFactory,
    affinity_secret_provider: RuntimeAffinitySecretProvider,
    affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    upstream: HyperHttpUpstreamTransport,
    upstream_endpoint: UpstreamEndpoint,
    websocket_revocations: WebSocketRevocationRegistry,
    audit_sink: Option<AuditFileSink>,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    active_reservations: RouteBandReservationBooks,
    selection_reservation_lock: SelectionReservationLock,
    runtime_exhaustions: RouteBandRuntimeExhaustions,
    route_band_queue_health: RouteBandQueueHealth,
    db_write_actor: DbWriteActor,
    maintenance_actor: MaintenanceActor,
    fixed_now_unix_seconds: Option<u64>,
    connection_error_reporter: Arc<dyn LoopbackConnectionErrorReporter>,
}

impl LoopbackRouterRuntime {
    /// Opens router-owned state/secrets and binds the loopback listener.
    pub fn start(config: LoopbackRouterRuntimeConfig) -> Result<Self, LoopbackRouterRuntimeError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(LoopbackRouterRuntimeError::TokioRuntime)?;
        let fixed_now_unix_seconds = config.fixed_now_unix_seconds;
        let credential_resources = ProxyRuntimeCredentialResources::open(
            &config.secret_store_root,
            fixed_now_unix_seconds,
        )?;
        let affinity_secret_provider = credential_resources.affinity_secret_provider();
        let credential_factory = credential_resources.credential_factory();
        let writable_state_stores = runtime.block_on(open_runtime_writable_state_stores(
            &config.state_database_path,
        ))?;
        let selection_state_store = runtime.block_on(AsyncSqliteStateStore::open_read_only(
            &config.state_database_path,
        ))?;
        let auth_gate = match config.local_token {
            Some(local_token) => crate::local_auth::ProxyLocalAuthGate::new(LocalRouterAuth::new(
                local_token,
                Vec::new(),
            )),
            None => crate::local_auth::ProxyLocalAuthGate::disabled(),
        };
        let upstream_endpoint = config.upstream_endpoint;
        let upstream = HyperHttpUpstreamTransport::new(upstream_endpoint.clone());
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

        let loopback_runtime = Self {
            runtime,
            server,
            credential_state_store: writable_state_stores.credential_state_store,
            provider_error_state_store: writable_state_stores.db_write_state_store,
            selection_state_store,
            affinity_secret_provider,
            affinity_owner_recorder,
            auth_gate,
            upstream,
            upstream_endpoint,
            websocket_revocations,
            audit_sink,
            weighted_selectors: Default::default(),
            account_holds: Default::default(),
            active_reservations: Default::default(),
            selection_reservation_lock,
            runtime_exhaustions: Default::default(),
            route_band_queue_health,
            db_write_actor,
            maintenance_actor,
            credential_factory,
            fixed_now_unix_seconds,
            connection_error_reporter: Arc::new(StderrLoopbackConnectionErrorReporter),
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
            .block_on(self.serve_protocol_connections_async(max_connections, None))
    }

    /// Serves HTTP/SSE or WebSocket connections until the bound or cancellation.
    pub fn serve_protocol_connections_until_cancelled(
        &self,
        max_connections: usize,
        shutdown: CancellationToken,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        self.runtime
            .block_on(self.serve_protocol_connections_async(max_connections, Some(shutdown)))
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
            let handler =
                tokio::spawn(async move { handler_context.handle_hyper_connection(stream).await });
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
            credential_factory: self.credential_factory.clone(),
            affinity_secret_provider: self.affinity_secret_provider.clone(),
            affinity_owner_recorder: Arc::clone(&self.affinity_owner_recorder),
            affinity_record_tasks,
            auth_gate: self.auth_gate.clone(),
            upstream: self.upstream.clone(),
            upstream_endpoint: self.upstream_endpoint.clone(),
            websocket_revocations: self.websocket_revocations.clone(),
            audit_sink: self.audit_sink.clone(),
            weighted_selectors: Arc::clone(&self.weighted_selectors),
            account_holds: Arc::clone(&self.account_holds),
            active_reservations: Arc::clone(&self.active_reservations),
            selection_reservation_lock: Arc::clone(&self.selection_reservation_lock),
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
        const ACTIVE_SESSION_COMPACTION_SECONDS: u64 = 86_400;

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
                        compact_before_unix_seconds: now_unix_seconds
                            .saturating_sub(ACTIVE_SESSION_COMPACTION_SECONDS),
                    });
        }
    }
}

fn handle_connection_join_result(
    joined: Result<Result<(), LoopbackRouterRuntimeError>, JoinError>,
) -> Result<(), LoopbackRouterRuntimeError> {
    match joined {
        Ok(Ok(())) => Ok(()),
        Ok(Err(LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::Transport(ref error),
        ))) if crate::websocket::is_normal_websocket_cleanup_close(error) => Ok(()),
        Ok(Err(error)) => Err(error),
        Err(source) => Err(LoopbackRouterRuntimeError::ConnectionJoin(source)),
    }
}

fn store_connection_join_error(
    first_connection_error: &mut Option<LoopbackRouterRuntimeError>,
    joined: Result<Result<(), LoopbackRouterRuntimeError>, JoinError>,
) -> bool {
    match handle_connection_join_result(joined) {
        Ok(()) => false,
        Err(error) => {
            if first_connection_error.is_none() {
                *first_connection_error = Some(error);
            }
            true
        }
    }
}

fn store_optional_connection_join_error(
    first_connection_error: &mut Option<LoopbackRouterRuntimeError>,
    joined: Option<Result<Result<(), LoopbackRouterRuntimeError>, JoinError>>,
) -> bool {
    match joined {
        Some(joined) => store_connection_join_error(first_connection_error, joined),
        None => false,
    }
}

fn supervise_detached_connection_handler(
    handler: UpgradeTaskHandle,
    reporter: Arc<dyn LoopbackConnectionErrorReporter>,
) {
    tokio::spawn(async move {
        match handler.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                reporter.report_connection_error(&loopback_connection_diagnostic(&error).render());
            }
            Err(_source) => reporter.report_connection_error(
                &LoopbackConnectionDiagnostic::new("join_failure", "task_join", "error").render(),
            ),
        }
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LoopbackConnectionDiagnostic {
    class: &'static str,
    safe_reason: &'static str,
    severity: &'static str,
}

impl LoopbackConnectionDiagnostic {
    const fn new(class: &'static str, safe_reason: &'static str, severity: &'static str) -> Self {
        Self {
            class,
            safe_reason,
            severity,
        }
    }

    #[cfg(test)]
    const fn class(self) -> &'static str {
        self.class
    }

    #[cfg(test)]
    const fn safe_reason(self) -> &'static str {
        self.safe_reason
    }

    #[cfg(test)]
    const fn severity(self) -> &'static str {
        self.severity
    }

    fn render(self) -> String {
        format!(
            "codex-router loopback connection failed: severity={} class={} reason={}",
            self.severity, self.class, self.safe_reason
        )
    }
}

fn loopback_connection_diagnostic(
    error: &LoopbackRouterRuntimeError,
) -> LoopbackConnectionDiagnostic {
    match error {
        LoopbackRouterRuntimeError::HyperConnection(source)
        | LoopbackRouterRuntimeError::HyperBody(source) => hyper_loopback_error_diagnostic(source),
        LoopbackRouterRuntimeError::ConnectionJoin(_) => {
            LoopbackConnectionDiagnostic::new("join_failure", "task_join", "error")
        }
        LoopbackRouterRuntimeError::WebSocket(_) => LoopbackConnectionDiagnostic::new(
            "upstream_tunnel_failure",
            websocket_runtime_error_kind(error),
            "error",
        ),
        _ => LoopbackConnectionDiagnostic::new(
            "router_runtime_failure",
            websocket_runtime_error_kind(error),
            "error",
        ),
    }
}

fn hyper_loopback_error_diagnostic(error: &hyper::Error) -> LoopbackConnectionDiagnostic {
    if error.is_incomplete_message() {
        return LoopbackConnectionDiagnostic::new(
            "client_disconnect",
            "hyper_incomplete_message",
            "debug",
        );
    }
    if error.is_canceled() {
        return LoopbackConnectionDiagnostic::new("client_disconnect", "hyper_canceled", "debug");
    }
    if error.is_closed() {
        return LoopbackConnectionDiagnostic::new("client_disconnect", "hyper_closed", "debug");
    }
    if error.is_body_write_aborted() {
        return LoopbackConnectionDiagnostic::new(
            "client_disconnect",
            "hyper_body_write_aborted",
            "debug",
        );
    }
    if error.is_shutdown() {
        return LoopbackConnectionDiagnostic::new("client_disconnect", "hyper_shutdown", "debug");
    }
    if hyper_error_source_chain_contains(error, "end of file before message length reached") {
        return LoopbackConnectionDiagnostic::new(
            "client_disconnect",
            "hyper_incomplete_message",
            "debug",
        );
    }
    if error.is_parse() {
        return LoopbackConnectionDiagnostic::new("malformed_request", "hyper_parse", "warn");
    }
    if error.is_timeout() {
        return LoopbackConnectionDiagnostic::new("client_disconnect", "hyper_timeout", "debug");
    }

    LoopbackConnectionDiagnostic::new("unknown", "hyper_unknown", "error")
}

fn hyper_error_source_chain_contains(error: &hyper::Error, needle: &str) -> bool {
    let mut source = std::error::Error::source(error);
    while let Some(error_source) = source {
        if error_source.to_string().contains(needle) {
            return true;
        }
        source = error_source.source();
    }

    false
}

#[derive(Clone)]
struct LoopbackProtocolConnectionHandler {
    credential_state_store: AsyncSqliteStateStore,
    provider_error_state_store: AsyncSqliteStateStore,
    selection_state_store: AsyncSqliteStateStore,
    credential_factory: AsyncProxyCredentialResolverFactory,
    affinity_secret_provider: RuntimeAffinitySecretProvider,
    affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    affinity_record_tasks: TaskTracker,
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    upstream: HyperHttpUpstreamTransport,
    upstream_endpoint: UpstreamEndpoint,
    websocket_revocations: WebSocketRevocationRegistry,
    audit_sink: Option<AuditFileSink>,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    active_reservations: RouteBandReservationBooks,
    selection_reservation_lock: SelectionReservationLock,
    runtime_exhaustions: RouteBandRuntimeExhaustions,
    route_band_queue_health: RouteBandQueueHealth,
    db_write_actor: DbWriteActor,
    fixed_now_unix_seconds: Option<u64>,
    session_shutdown: CancellationToken,
}

type UpgradeTaskResult = Result<(), LoopbackRouterRuntimeError>;
type UpgradeTaskHandle = tokio::task::JoinHandle<UpgradeTaskResult>;
type SharedUpgradeTasks = Arc<tokio::sync::Mutex<Vec<UpgradeTaskHandle>>>;

impl LoopbackProtocolConnectionHandler {
    async fn handle_hyper_connection(
        self: Arc<Self>,
        stream: tokio::net::TcpStream,
    ) -> Result<(), LoopbackRouterRuntimeError> {
        let local_peer_addr = stream.peer_addr().ok();
        let io = TokioIo::new(stream);
        let service_context = Arc::clone(&self);
        let upgrade_tasks = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let service_upgrade_tasks = Arc::clone(&upgrade_tasks);
        let service = service_fn(move |request: HttpRequest<Incoming>| {
            let request_context = Arc::clone(&service_context);
            let request_upgrade_tasks = Arc::clone(&service_upgrade_tasks);
            async move {
                Ok::<_, Infallible>(
                    request_context
                        .handle_hyper_request(request, request_upgrade_tasks, local_peer_addr)
                        .await,
                )
            }
        });

        let mut http_builder = http1::Builder::new();
        http_builder.half_close(true);
        let serve_result = http_builder
            .serve_connection(io, service)
            .with_upgrades()
            .await
            .map_err(LoopbackRouterRuntimeError::HyperConnection);
        finish_hyper_connection_after_serve_result(serve_result, upgrade_tasks).await
    }

    async fn handle_hyper_request(
        self: Arc<Self>,
        request: HttpRequest<Incoming>,
        upgrade_tasks: SharedUpgradeTasks,
        local_peer_addr: Option<SocketAddr>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        match HyperProtocolSwitchpoint::classify(request.method(), request.uri(), request.headers())
        {
            HyperProtocolDispatch::WebSocketUpgrade => {
                self.handle_hyper_websocket_request(request, upgrade_tasks, local_peer_addr)
                    .await
            }
            HyperProtocolDispatch::Http => self.handle_hyper_http_request(request).await,
        }
    }

    async fn handle_hyper_websocket_request(
        self: Arc<Self>,
        mut request: HttpRequest<Incoming>,
        upgrade_tasks: SharedUpgradeTasks,
        local_peer_addr: Option<SocketAddr>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        let path = request
            .uri()
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str)
            .to_owned();
        let handshake = websocket_handshake_from_hyper_headers(request.headers());
        if let Some(response) = self.preflight_hyper_websocket_request(&request, &path) {
            return response;
        }
        let (upgrade_response, websocket) =
            match hyper_tungstenite::upgrade(&mut request, Some(router_websocket_config())) {
                Ok(upgrade) => upgrade,
                Err(_error) => return empty_response(StatusCode::BAD_REQUEST),
            };
        let task_context = Arc::clone(&self);
        let upgrade_task = tokio::spawn(async move {
            match websocket.await {
                Ok(local_websocket) => {
                    task_context
                        .handle_hyper_websocket_upgraded(
                            local_websocket,
                            handshake,
                            path,
                            local_peer_addr,
                        )
                        .await
                }
                Err(error) => Err(LoopbackRouterRuntimeError::WebSocket(
                    crate::websocket::WebSocketTunnelError::Transport(error),
                )),
            }
        });
        upgrade_tasks.lock().await.push(upgrade_task);

        upgrade_response.map(|body| {
            body.map_err(|never: Infallible| -> AsyncHttpBodyError { match never {} })
                .boxed()
        })
    }

    async fn handle_hyper_websocket_upgraded(
        self: Arc<Self>,
        local_websocket: hyper_tungstenite::HyperWebsocketStream,
        handshake: WebSocketHandshakeRequest,
        path: String,
        local_peer_addr: Option<SocketAddr>,
    ) -> Result<(), LoopbackRouterRuntimeError> {
        let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
            &self.selection_state_store,
            AsyncAccountSelectorRuntimeState::new_with_selection_lock(
                Arc::clone(&self.weighted_selectors),
                Arc::clone(&self.account_holds),
                Arc::clone(&self.active_reservations),
                Arc::clone(&self.runtime_exhaustions),
                Arc::clone(&self.route_band_queue_health),
                Arc::clone(&self.selection_reservation_lock),
            ),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            self.runtime_clock(),
        )
        .with_active_client_lease_reporter(Arc::new(SqliteActiveClientLeaseReporter::new(
            self.db_write_actor.clone(),
            self.runtime_clock(),
        )));
        let credential_resolver = self
            .credential_factory
            .resolver_for_state(self.credential_state_store.clone());
        let protocol_router = WebSocketProtocolRouter::new();
        let tunnel = if let Some(audit_sink) = &self.audit_sink {
            AsyncWebSocketTunnel::new_with_audit_sink(
                &self.auth_gate,
                &selector,
                &credential_resolver,
                &protocol_router,
                audit_sink,
            )
        } else {
            AsyncWebSocketTunnel::new(
                &self.auth_gate,
                &selector,
                &credential_resolver,
                &protocol_router,
            )
        }
        .with_revocation_registry(self.websocket_revocations.clone())
        .with_session_shutdown(self.session_shutdown.clone())
        .with_affinity_secret_provider(&self.affinity_secret_provider)
        .with_async_affinity_owner_recorder(Arc::clone(&self.affinity_owner_recorder))
        .with_affinity_owner_task_tracker(self.affinity_record_tasks.clone())
        .with_provider_error_observer(Arc::new(AsyncSqliteProviderErrorObserver::new(
            self.provider_error_state_store.clone(),
            self.selection_state_store.clone(),
            Arc::clone(&self.active_reservations),
            Arc::clone(&self.runtime_exhaustions),
            Arc::clone(&self.route_band_queue_health),
            self.db_write_actor.clone(),
        )))
        .with_local_peer_addr(local_peer_addr);
        let upstream_url = self.upstream_endpoint.websocket_url_for_path(&path);
        {
            crate::telemetry::record_websocket_event(RouteBand::Responses.as_str(), "open");
            let open_span = tracing::info_span!(
                "codex_router.websocket_open",
                route.path = sanitize_route_path_for_log(&path),
                peer.present = local_peer_addr.is_some(),
            );
            let _open_span_guard = open_span.enter();
            tracing::info!(
                route.path = sanitize_route_path_for_log(&path),
                peer.present = local_peer_addr.is_some(),
                "codex_router.websocket_open"
            );
        }
        let result = tunnel
            .handle_upgraded_connection(local_websocket, handshake, upstream_url.as_str())
            .await
            .map_err(LoopbackRouterRuntimeError::WebSocket);
        match &result {
            Ok(()) => {
                crate::telemetry::record_websocket_event(RouteBand::Responses.as_str(), "closed");
                let span = tracing::info_span!(
                    "codex_router.websocket_closed",
                    route.path = sanitize_route_path_for_log(&path),
                );
                let _span_guard = span.enter();
                tracing::info!(
                    route.path = sanitize_route_path_for_log(&path),
                    "codex_router.websocket_closed"
                );
            }
            Err(error) => {
                crate::telemetry::record_websocket_event(RouteBand::Responses.as_str(), "failed");
                let span = tracing::warn_span!(
                    "codex_router.websocket_failed",
                    route.path = sanitize_route_path_for_log(&path),
                    error.kind = websocket_runtime_error_kind(error),
                    error = %sanitize_error_for_log(error),
                );
                let _span_guard = span.enter();
                tracing::warn!(
                    route.path = sanitize_route_path_for_log(&path),
                    error.kind = websocket_runtime_error_kind(error),
                    error = %sanitize_error_for_log(error),
                    "codex_router.websocket_failed"
                );
            }
        }
        result
    }

    async fn handle_hyper_http_request(
        self: Arc<Self>,
        request: HttpRequest<Incoming>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        let (request, body, full_replay_body) =
            match hyper_request_to_streaming_proxy_request(request).await {
                Ok(request) => request,
                Err(_error) => return empty_response(StatusCode::BAD_REQUEST),
            };
        let replayable_request = full_replay_body
            .filter(|body| request_metadata_prefix_is_complete_json(body))
            .map(|body| request.clone().with_body(body));

        let max_account_attempts = if replayable_request.is_some() {
            match self.enabled_account_attempt_limit().await {
                Ok(limit) => limit,
                Err(_error) => return quota_state_unavailable_response(),
            }
        } else {
            1
        };
        let mut first_attempt_body = Some(body);
        for attempt_index in 0..max_account_attempts {
            let (attempt_request, attempt_body) = if attempt_index == 0 {
                let Some(first_attempt_body) = first_attempt_body.take() else {
                    return empty_response(StatusCode::SERVICE_UNAVAILABLE);
                };
                (request.clone(), first_attempt_body)
            } else if let Some(replayable_request) = replayable_request.clone() {
                let retry_body = box_body_from_bytes(replayable_request.body().to_vec());
                (replayable_request, retry_body)
            } else {
                return empty_response(StatusCode::SERVICE_UNAVAILABLE);
            };

            let prepared = match self
                .prepare_async_streaming_http_request_async(attempt_request, attempt_body)
                .await
            {
                Ok(prepared) => prepared,
                Err(error) => return http_error_response(error),
            };
            let (upstream_request, completion) = prepared.into_parts();
            let response = match self.upstream.send_streaming(upstream_request).await {
                Ok(response) => response,
                Err(error) => return http_error_response(error),
            };
            match self
                .observe_precommit_http_quota_response(response, completion)
                .await
            {
                Ok(prepared_response) => {
                    return self.async_streaming_http_response_to_hyper(
                        prepared_response.completion,
                        prepared_response.response,
                    );
                }
                Err(PrecommitHttpQuotaResponse::AccountQuotaExhausted) => {
                    if replayable_request.is_none() {
                        return quota_state_unavailable_response();
                    }
                    tracing::info!(
                        attempt = attempt_index + 1,
                        max_attempts = max_account_attempts,
                        "codex_router.http_precommit_account_exhausted_retry"
                    );
                }
                Err(PrecommitHttpQuotaResponse::ProbeFailed(error)) => {
                    return http_error_response(error);
                }
                Err(PrecommitHttpQuotaResponse::ObservationFailed) => {
                    return empty_response(StatusCode::SERVICE_UNAVAILABLE);
                }
            }
        }

        all_accounts_exhausted_response()
    }

    async fn enabled_account_attempt_limit(&self) -> Result<usize, StateStoreError> {
        enabled_account_attempt_limit_from_accounts(
            self.selection_state_store.list_accounts().await,
        )
    }
}

fn enabled_account_attempt_limit_from_accounts(
    accounts: Result<Vec<AccountRecord>, StateStoreError>,
) -> Result<usize, StateStoreError> {
    Ok(accounts?
        .iter()
        .filter(|account| account.status() == AccountStatus::Enabled)
        .count()
        .max(1))
}

impl LoopbackProtocolConnectionHandler {
    async fn prepare_async_streaming_http_request_async(
        &self,
        request: HttpProxyRequest,
        body: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> Result<PreparedAsyncStreamingHttpProxyRequest, HttpProxyError> {
        let credential_resolver = self
            .credential_factory
            .resolver_for_state(self.credential_state_store.clone());
        let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
            &self.selection_state_store,
            AsyncAccountSelectorRuntimeState::new_with_selection_lock(
                Arc::clone(&self.weighted_selectors),
                Arc::clone(&self.account_holds),
                Arc::clone(&self.active_reservations),
                Arc::clone(&self.runtime_exhaustions),
                Arc::clone(&self.route_band_queue_health),
                Arc::clone(&self.selection_reservation_lock),
            ),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            self.runtime_clock(),
        )
        .with_active_client_lease_reporter(Arc::new(SqliteActiveClientLeaseReporter::new(
            self.db_write_actor.clone(),
            self.runtime_clock(),
        )));
        let service = AuthenticatedHttpProxyService::new(
            &self.auth_gate,
            &selector,
            &credential_resolver,
            &self.upstream,
        )
        .with_affinity_secret_provider(&self.affinity_secret_provider)
        .with_provider_error_observer(Arc::new(AsyncSqliteProviderErrorObserver::new(
            self.provider_error_state_store.clone(),
            self.selection_state_store.clone(),
            Arc::clone(&self.active_reservations),
            Arc::clone(&self.runtime_exhaustions),
            Arc::clone(&self.route_band_queue_health),
            self.db_write_actor.clone(),
        )));
        let service = if let Some(audit_sink) = &self.audit_sink {
            service.with_audit_sink(audit_sink)
        } else {
            service
        };
        service
            .prepare_async_streaming_request_async(request, body)
            .await
    }

    fn async_streaming_http_response_to_hyper(
        &self,
        completion: crate::http_sse::StreamingHttpProxyCompletion,
        response: AsyncStreamingHttpProxyResponse,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        if let Some(audit_sink) = &self.audit_sink {
            append_audit_event_with_reporter(
                audit_sink,
                completion.allowed_audit_event(),
                &StderrAuditFailureReporter,
            );
        }
        let (status, headers, body) = response.into_parts();
        async_streaming_http_response_to_hyper(
            status,
            headers,
            body,
            completion,
            Arc::clone(&self.affinity_owner_recorder),
            self.affinity_record_tasks.clone(),
        )
    }

    async fn observe_precommit_http_quota_response(
        &self,
        response: AsyncStreamingHttpProxyResponse,
        completion: StreamingHttpProxyCompletion,
    ) -> Result<PreparedHttpResponseForCommit, PrecommitHttpQuotaResponse> {
        let provider_error_observer = completion.provider_error_observer().cloned();
        let account_id = completion.account_id().clone();
        let route_band = completion.route_band();
        match split_precommit_http_quota_response(response).await {
            Ok(PrecommitHttpResponseProbe::Forward(response)) => {
                Ok(PreparedHttpResponseForCommit {
                    response,
                    completion,
                })
            }
            Ok(PrecommitHttpResponseProbe::AccountQuotaExhausted { body }) => {
                drop(body);
                observe_precommit_http_quota_exhaustion_for_retry(
                    provider_error_observer,
                    account_id,
                    route_band,
                    current_unix_seconds().map_or(0, |seconds| seconds),
                )?;
                Err(PrecommitHttpQuotaResponse::AccountQuotaExhausted)
            }
            Err(error) => Err(PrecommitHttpQuotaResponse::ProbeFailed(error)),
        }
    }

    fn preflight_hyper_websocket_request(
        &self,
        request: &HttpRequest<Incoming>,
        path: &str,
    ) -> Option<HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>>> {
        let subprotocol = header_value(request.headers(), "sec-websocket-protocol");
        let router_token = header_value(request.headers(), "x-codex-router-token");
        let authorization = header_value(request.headers(), "authorization");
        let cookie = header_value(request.headers(), "cookie");
        let presented_token = if subprotocol
            .as_deref()
            .is_some_and(has_forbidden_websocket_subprotocol_auth_carrier)
        {
            Err(LocalAuthError::Wrong)
        } else {
            extract_presented_local_token_from_request(
                router_token.as_deref(),
                authorization.as_deref(),
                cookie.as_deref(),
                path,
                &[],
                false,
            )
        };
        let presented_token = match presented_token {
            Ok(presented_token) => presented_token,
            Err(reason) => {
                self.emit_websocket_local_auth_rejection(reason);
                return Some(empty_response(StatusCode::UNAUTHORIZED));
            }
        };
        if let Err(reason) = self.auth_gate.authorize(presented_token) {
            self.emit_websocket_local_auth_rejection(reason);
            return Some(empty_response(StatusCode::UNAUTHORIZED));
        }
        match classify_route(Method::Post, path_without_query(path), true) {
            RouteClass::Supported(_) => None,
            RouteClass::Rejected { .. } => Some(empty_response(StatusCode::NOT_FOUND)),
        }
    }

    fn emit_websocket_local_auth_rejection(&self, reason: LocalAuthError) {
        if let Some(audit_sink) = &self.audit_sink {
            let event = local_auth_rejection_audit_event(
                TransportKind::WebSocket,
                AuditRouteKind::ResponsesWebSocket,
                reason,
            );
            append_audit_event_with_reporter(audit_sink, &event, &StderrAuditFailureReporter);
        }
    }

    fn runtime_clock(&self) -> Arc<dyn Fn() -> u64 + Send + Sync> {
        let fixed_now_unix_seconds = self.fixed_now_unix_seconds;
        Arc::new(move || {
            fixed_now_unix_seconds.unwrap_or_else(|| match current_unix_seconds() {
                Ok(now_unix_seconds) => now_unix_seconds,
                Err(error) => {
                    tracing::error!(
                        error.class = "system_clock_before_unix_epoch",
                        error.message = %error,
                        "codex_router.runtime_clock_failed"
                    );
                    0
                }
            })
        })
    }
}

fn current_unix_seconds() -> Result<u64, std::time::SystemTimeError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
}

/// Thread-safe handle for replacing local auth without sharing the full runtime.
#[derive(Clone, Debug)]
pub struct LocalAuthReloader {
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    websocket_revocations: WebSocketRevocationRegistry,
}

impl LocalAuthReloader {
    /// Replaces local auth from an already loaded auth snapshot.
    pub fn reload_auth(&self, auth: LocalRouterAuth) {
        let active_generation = auth.current_generation();
        self.auth_gate.replace(auth);
        self.websocket_revocations
            .close_all_except(active_generation);
    }

    /// Replaces local auth and closes WebSocket connections authenticated with old generations.
    pub fn reload_local_auth(
        &self,
        current: LocalRouterTokenRecord,
        previous: Vec<LocalRouterTokenRecord>,
    ) {
        self.reload_auth(LocalRouterAuth::new(current, previous));
    }
}

fn has_forbidden_websocket_subprotocol_auth_carrier(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("token") || value.contains("bearer") || value.contains("authorization")
}

fn websocket_handshake_from_hyper_headers(headers: &HeaderMap) -> WebSocketHandshakeRequest {
    let mut handshake = WebSocketHandshakeRequest::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            handshake = handshake.with_header(Header::new(name.as_str(), value));
        }
    }

    handshake
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(header_name, _value)| header_name.as_str().eq_ignore_ascii_case(name))
        .and_then(|(_header_name, value)| value.to_str().ok())
        .map(str::to_owned)
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?').map_or(path, |(path, _query)| path)
}

const HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES: usize = 16 * 1024;
const HTTP_REQUEST_REPLAY_MAX_BYTES: usize = 2 * 1024 * 1024;
const HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES: usize = 64 * 1024;
const HTTP_RESPONSE_AFFINITY_SCAN_MAX_EVENTS: usize = 64;

async fn hyper_request_to_streaming_proxy_request(
    request: HttpRequest<Incoming>,
) -> Result<
    (
        HttpProxyRequest,
        BoxBody<Bytes, AsyncHttpBodyError>,
        Option<Vec<u8>>,
    ),
    LoopbackRouterRuntimeError,
> {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str)
        .to_owned();
    let buffered_body = bounded_request_metadata_body(body)
        .await
        .map_err(LoopbackRouterRuntimeError::HyperBody)?;
    let mut proxy_request = HttpProxyRequest::new(method_from_hyper(&parts.method), path);
    for (name, value) in &parts.headers {
        if let Ok(value) = value.to_str() {
            proxy_request = proxy_request.with_header(Header::new(name.as_str(), value));
        }
    }
    let streaming_body = buffered_body
        .streaming_body
        .map_err(incoming_body_error)
        .boxed();

    Ok((
        proxy_request.with_body(buffered_body.routing_metadata_prefix),
        streaming_body,
        buffered_body.full_replay_body,
    ))
}

struct BufferedRequestBody {
    routing_metadata_prefix: Vec<u8>,
    full_replay_body: Option<Vec<u8>>,
    streaming_body: PrefixFramesThenIncomingBody,
}

async fn bounded_request_metadata_body(
    mut body: Incoming,
) -> Result<BufferedRequestBody, hyper::Error> {
    let mut routing_metadata_prefix = Vec::new();
    let mut full_replay_body = Some(Vec::new());
    let mut replay_frames = VecDeque::new();
    loop {
        let Some(frame) = body.frame().await.transpose()? else {
            break;
        };
        if let Some(data) = frame.data_ref() {
            if routing_metadata_prefix.len() < HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES {
                let remaining_bytes =
                    HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES - routing_metadata_prefix.len();
                let bytes_to_scan = data.len().min(remaining_bytes);
                if let Some(scanned_data) = data.get(..bytes_to_scan) {
                    routing_metadata_prefix.extend_from_slice(scanned_data);
                }
            }
            if let Some(replay_body) = full_replay_body.as_mut() {
                if replay_body.len().saturating_add(data.len()) <= HTTP_REQUEST_REPLAY_MAX_BYTES {
                    replay_body.extend_from_slice(data);
                } else {
                    full_replay_body = None;
                }
            }
        } else {
            full_replay_body = None;
        }
        let replay_body_is_complete = full_replay_body
            .as_deref()
            .is_some_and(request_metadata_prefix_is_complete_json);
        replay_frames.push_back(frame);
        if replay_body_is_complete || full_replay_body.is_none() {
            break;
        }
    }

    Ok(BufferedRequestBody {
        routing_metadata_prefix,
        full_replay_body,
        streaming_body: PrefixFramesThenIncomingBody::new(replay_frames, body),
    })
}

async fn finish_hyper_connection_after_serve_result(
    serve_result: Result<(), LoopbackRouterRuntimeError>,
    upgrade_tasks: SharedUpgradeTasks,
) -> Result<(), LoopbackRouterRuntimeError> {
    let mut first_connection_error = serve_result.err();
    let mut upgrade_task_guard = upgrade_tasks.lock().await;
    let drained_upgrade_tasks = std::mem::take(&mut *upgrade_task_guard);
    drop(upgrade_task_guard);

    for upgrade_task in drained_upgrade_tasks {
        let _stored_error =
            store_connection_join_error(&mut first_connection_error, upgrade_task.await);
    }

    match first_connection_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn request_metadata_prefix_is_complete_json(metadata_prefix: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(metadata_prefix).is_ok()
}

struct PreparedHttpResponseForCommit {
    response: AsyncStreamingHttpProxyResponse,
    completion: StreamingHttpProxyCompletion,
}

#[derive(Debug)]
enum PrecommitHttpQuotaResponse {
    AccountQuotaExhausted,
    ObservationFailed,
    ProbeFailed(HttpProxyError),
}

enum PrecommitHttpResponseProbe {
    Forward(AsyncStreamingHttpProxyResponse),
    AccountQuotaExhausted { body: Vec<u8> },
}

fn observe_precommit_http_quota_exhaustion_for_retry(
    provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
    account_id: codex_router_core::ids::AccountId,
    route_band: RouteBand,
    observed_unix_seconds: u64,
) -> Result<(), PrecommitHttpQuotaResponse> {
    let Some(observer) = provider_error_observer else {
        return Ok(());
    };
    observer
        .mark_runtime_account_quota_exhausted(account_id.clone(), route_band, observed_unix_seconds)
        .map_err(|_error| PrecommitHttpQuotaResponse::ObservationFailed)?;
    match observer.enqueue_provider_quota_exhaustion(
        account_id,
        route_band,
        ProviderErrorClassification::AccountQuotaExhausted,
        observed_unix_seconds,
    ) {
        DbWriteEnqueueResult::Enqueued => Ok(()),
        DbWriteEnqueueResult::FullDegraded | DbWriteEnqueueResult::ClosedDegraded => {
            Err(PrecommitHttpQuotaResponse::ObservationFailed)
        }
    }
}

async fn split_precommit_http_quota_response(
    response: AsyncStreamingHttpProxyResponse,
) -> Result<PrecommitHttpResponseProbe, HttpProxyError> {
    let (status, headers, mut body) = response.into_parts();
    let mut replay_frames = VecDeque::new();
    let mut buffered = Vec::new();
    let mut scanned_bytes = 0_usize;
    let mut scanned_events = 0_usize;

    while scanned_bytes < HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES
        && scanned_events < HTTP_RESPONSE_AFFINITY_SCAN_MAX_EVENTS
    {
        let Some(frame) =
            body.frame()
                .await
                .transpose()
                .map_err(|error| HttpProxyError::Upstream {
                    message: error.to_string(),
                })?
        else {
            break;
        };
        scanned_events += 1;
        if let Some(data) = frame.data_ref() {
            let remaining_bytes = HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES - scanned_bytes;
            let bytes_to_scan = data.len().min(remaining_bytes);
            if let Some(scanned_data) = data.get(..bytes_to_scan) {
                buffered.extend_from_slice(scanned_data);
            }
            scanned_bytes += bytes_to_scan;
            if let Some(provider_error_body) = provider_error_body_from_http_buffer(&buffered)
                && classify_provider_error_envelope(&provider_error_body)
                    == ProviderErrorClassification::AccountQuotaExhausted
            {
                return Ok(PrecommitHttpResponseProbe::AccountQuotaExhausted {
                    body: provider_error_body,
                });
            }
        }
        replay_frames.push_back(frame);
        if status < 400 && !precommit_probe_should_continue_for_success_status(&buffered) {
            break;
        }
    }

    let replay_body = PrefixFramesThenBoxBody::new(replay_frames, body).boxed();
    Ok(PrecommitHttpResponseProbe::Forward(
        AsyncStreamingHttpProxyResponse::new(status, headers, replay_body),
    ))
}

fn precommit_probe_should_continue_for_success_status(buffered: &[u8]) -> bool {
    let trimmed = trim_ascii_bytes(buffered);
    trimmed.starts_with(b"event: error") && !trimmed.windows(2).any(|window| window == b"\n\n")
}

fn box_body_from_bytes(bytes: Vec<u8>) -> BoxBody<Bytes, AsyncHttpBodyError> {
    Full::new(Bytes::from(bytes))
        .map_err(|never: Infallible| -> AsyncHttpBodyError { match never {} })
        .boxed()
}

struct PrefixFramesThenBoxBody {
    prefix_frames: VecDeque<Frame<Bytes>>,
    inner: BoxBody<Bytes, AsyncHttpBodyError>,
}

impl PrefixFramesThenBoxBody {
    fn new(
        prefix_frames: VecDeque<Frame<Bytes>>,
        inner: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> Self {
        Self {
            prefix_frames,
            inner,
        }
    }
}

impl HyperBody for PrefixFramesThenBoxBody {
    type Data = Bytes;
    type Error = AsyncHttpBodyError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if let Some(frame) = self.prefix_frames.pop_front() {
            return Poll::Ready(Some(Ok(frame)));
        }

        Pin::new(&mut self.inner).poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.prefix_frames.is_empty() && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        prefix_frames_size_hint(&self.prefix_frames, self.inner.size_hint())
    }
}

struct PrefixFramesThenIncomingBody {
    prefix_frames: VecDeque<Frame<Bytes>>,
    inner: Incoming,
}

impl PrefixFramesThenIncomingBody {
    fn new(prefix_frames: VecDeque<Frame<Bytes>>, inner: Incoming) -> Self {
        Self {
            prefix_frames,
            inner,
        }
    }
}

impl HyperBody for PrefixFramesThenIncomingBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if let Some(frame) = self.prefix_frames.pop_front() {
            return Poll::Ready(Some(Ok(frame)));
        }

        Pin::new(&mut self.inner).poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.prefix_frames.is_empty() && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        prefix_frames_size_hint(&self.prefix_frames, self.inner.size_hint())
    }
}

fn prefix_frames_size_hint(
    prefix_frames: &VecDeque<Frame<Bytes>>,
    inner_hint: SizeHint,
) -> SizeHint {
    let prefix_data_length = prefix_frames
        .iter()
        .filter_map(Frame::data_ref)
        .map(|data| u64::try_from(data.len()).unwrap_or(u64::MAX))
        .fold(0_u64, u64::saturating_add);
    let mut size_hint = SizeHint::new();
    size_hint.set_lower(prefix_data_length.saturating_add(inner_hint.lower()));
    if let Some(inner_upper) = inner_hint.upper() {
        size_hint.set_upper(prefix_data_length.saturating_add(inner_upper));
    }

    size_hint
}

fn method_from_hyper(method: &HttpMethod) -> Method {
    match *method {
        HttpMethod::GET => Method::Get,
        HttpMethod::POST => Method::Post,
        _ => Method::Other,
    }
}

fn async_streaming_http_response_to_hyper(
    status: u16,
    headers: HeaderCollection,
    body: BoxBody<Bytes, AsyncHttpBodyError>,
    completion: StreamingHttpProxyCompletion,
    affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    affinity_record_tasks: TaskTracker,
) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    let body = record_affinity_owner_from_async_body(
        body,
        completion,
        affinity_owner_recorder,
        None,
        affinity_record_tasks,
    );
    let mut builder = HttpResponse::builder().status(status);
    for header in headers.as_slice() {
        builder = builder.header(header.name(), header.value());
    }
    builder
        .body(body)
        .unwrap_or_else(|_error| empty_response(StatusCode::BAD_GATEWAY))
}

fn record_affinity_owner_from_async_body(
    body: BoxBody<Bytes, AsyncHttpBodyError>,
    completion: StreamingHttpProxyCompletion,
    affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
    affinity_record_tasks: TaskTracker,
) -> BoxBody<Bytes, AsyncHttpBodyError> {
    let active_reservation_guard = completion.active_reservation_guard().cloned();
    let affinity_secret = completion.affinity_secret().cloned();
    let provider_error_observer =
        provider_error_observer.or_else(|| completion.provider_error_observer().cloned());
    if affinity_secret.is_none() && provider_error_observer.is_none() {
        return hold_active_reservation_until_body_drop(body, active_reservation_guard);
    }
    let account_id = completion.account_id().clone();
    let route_band = completion.route_band();
    let credential_generation = completion.credential_generation();
    let mut buffered = Vec::new();
    let mut affinity_recorded = false;
    let mut provider_error_recorded = false;
    let mut scanned_bytes = 0_usize;
    let mut scanned_events = 0_usize;

    body.map_frame(move |frame| {
        let _active_reservation_guard = &active_reservation_guard;
        let should_scan_affinity = affinity_secret.is_some() && !affinity_recorded;
        let should_scan_provider_error =
            provider_error_observer.is_some() && !provider_error_recorded;
        if (should_scan_affinity || should_scan_provider_error)
            && scanned_bytes < HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES
            && scanned_events < HTTP_RESPONSE_AFFINITY_SCAN_MAX_EVENTS
            && let Some(data) = frame.data_ref()
        {
            scanned_events += 1;
            let remaining_bytes = HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES - scanned_bytes;
            let bytes_to_scan = data.len().min(remaining_bytes);
            if let Some(scanned_data) = data.get(..bytes_to_scan) {
                buffered.extend_from_slice(scanned_data);
            }
            scanned_bytes += bytes_to_scan;
            if should_scan_affinity
                && let Some(secret) = affinity_secret.as_ref()
                && let Ok(Some(response_id)) = extract_response_id_from_body(&buffered)
            {
                affinity_recorded = true;
                spawn_async_affinity_owner_record(
                    Arc::clone(&affinity_owner_recorder),
                    secret.clone(),
                    account_id.clone(),
                    credential_generation,
                    response_id,
                    affinity_record_tasks.clone(),
                );
            }
            if should_scan_provider_error
                && let Some(provider_error_body) = provider_error_body_from_http_buffer(&buffered)
            {
                provider_error_recorded = true;
                let provider_error_classification =
                    classify_provider_error_envelope(&provider_error_body);
                if let Some(observer) = provider_error_observer.as_ref() {
                    spawn_async_provider_error_observation(
                        Arc::clone(observer),
                        account_id.clone(),
                        route_band,
                        provider_error_classification,
                        affinity_record_tasks.clone(),
                    );
                }
            }
        }

        frame
    })
    .boxed()
}

fn provider_error_body_from_http_buffer(buffered: &[u8]) -> Option<Vec<u8>> {
    if classify_provider_error_envelope(buffered) != ProviderErrorClassification::Unknown {
        return Some(buffered.to_vec());
    }

    for line in buffered.split(|byte| *byte == b'\n') {
        let line = trim_ascii_bytes(line);
        let Some(data) = line.strip_prefix(b"data:") else {
            continue;
        };
        let data = trim_ascii_bytes(data);
        if data == b"[DONE]" || data.is_empty() {
            continue;
        }
        if classify_provider_error_envelope(data) != ProviderErrorClassification::Unknown {
            return Some(data.to_vec());
        }
    }

    None
}

fn trim_ascii_bytes(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    match bytes.get(start..end) {
        Some(trimmed) => trimmed,
        None => &[],
    }
}

fn hold_active_reservation_until_body_drop(
    body: BoxBody<Bytes, AsyncHttpBodyError>,
    active_reservation_guard: Option<crate::account_selection::ActiveReservationGuard>,
) -> BoxBody<Bytes, AsyncHttpBodyError> {
    if active_reservation_guard.is_none() {
        return body;
    }

    body.map_frame(move |frame| {
        let _active_reservation_guard = &active_reservation_guard;
        frame
    })
    .boxed()
}

fn spawn_async_affinity_owner_record(
    recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    affinity_secret: codex_router_core::affinity::RouterAffinityHashSecret,
    account_id: codex_router_core::ids::AccountId,
    credential_generation: u64,
    response_id: codex_router_core::affinity::PreviousResponseId,
    affinity_record_tasks: TaskTracker,
) {
    affinity_record_tasks.spawn(async move {
        let Ok(affinity_key_hash) = hash_previous_response_id(&affinity_secret, &response_id)
        else {
            return;
        };
        let owner = PreviousResponseAffinityOwnerRecord::new(
            affinity_key_hash,
            account_id,
            credential_generation,
            RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            current_unix_seconds().map_or(0, |seconds| seconds),
        );
        let _record_result = recorder.record_affinity_owner(owner).await;
    });
}

fn spawn_async_provider_error_observation(
    observer: Arc<dyn AsyncProviderErrorObserver>,
    account_id: codex_router_core::ids::AccountId,
    route_band: RouteBand,
    classification: ProviderErrorClassification,
    affinity_record_tasks: TaskTracker,
) {
    let observed_unix_seconds = current_unix_seconds().map_or(0, |seconds| seconds);
    if classification == ProviderErrorClassification::AccountQuotaExhausted {
        let _runtime_mark_result = observer.mark_runtime_account_quota_exhausted(
            account_id.clone(),
            route_band,
            observed_unix_seconds,
        );
        let _enqueue_result = observer.enqueue_provider_quota_exhaustion(
            account_id,
            route_band,
            classification,
            observed_unix_seconds,
        );
        return;
    }

    affinity_record_tasks.spawn(async move {
        let _observation_result = observer
            .observe_provider_error(
                account_id,
                route_band,
                classification,
                observed_unix_seconds,
            )
            .await;
    });
}

fn incoming_body_error(error: hyper::Error) -> AsyncHttpBodyError {
    Box::new(error)
}

fn sanitize_route_path_for_log(path: &str) -> &'static str {
    if path.ends_with("/responses") {
        "/v1/responses"
    } else if path.ends_with("/models") {
        "/v1/models"
    } else {
        "other"
    }
}

fn websocket_runtime_error_kind(error: &LoopbackRouterRuntimeError) -> &'static str {
    match error {
        LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::Transport(_),
        ) => "websocket_transport",
        LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::Handshake,
        ) => "websocket_handshake",
        LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::CloseReason(_),
        ) => "websocket_close_before_upstream",
        LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::ConnectionTracking(_),
        ) => "websocket_connection_tracking",
        LoopbackRouterRuntimeError::WebSocket(_) => "websocket_other",
        LoopbackRouterRuntimeError::HyperConnection(_) => "hyper_connection",
        LoopbackRouterRuntimeError::HyperBody(_) => "hyper_body",
        _ => "router_runtime",
    }
}

fn sanitize_error_for_log(error: &LoopbackRouterRuntimeError) -> String {
    let rendered_error = error.to_string();
    if rendered_error.contains("BadRecordMac") {
        "websocket transport failed: BadRecordMac".to_owned()
    } else if rendered_error.contains("FirstFrameTimeout") {
        "websocket closed before upstream open: FirstFrameTimeout".to_owned()
    } else {
        websocket_runtime_error_kind(error).to_owned()
    }
}

fn http_error_response(error: HttpProxyError) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    match error {
        HttpProxyError::LocalAuth { .. } => empty_response(StatusCode::UNAUTHORIZED),
        HttpProxyError::Selection {
            reason: crate::account_selection::QuotaAwareAccountSelectorError::NoEligibleAccounts,
        } => all_accounts_exhausted_response(),
        HttpProxyError::Selection { .. } => quota_state_unavailable_response(),
        HttpProxyError::ProviderCredential { .. } | HttpProxyError::Upstream { .. } => {
            empty_response(StatusCode::BAD_GATEWAY)
        }
        HttpProxyError::Rejected { .. } => empty_response(StatusCode::NOT_FOUND),
    }
}

fn empty_response(status: StatusCode) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    HttpResponse::builder()
        .status(status)
        .body(empty_body())
        .unwrap_or_else(|_error| HttpResponse::new(empty_body()))
}

fn all_accounts_exhausted_response() -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    HttpResponse::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .body(box_body_from_bytes(
            crate::websocket::ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL
                .as_bytes()
                .to_vec(),
        ))
        .unwrap_or_else(|_error| HttpResponse::new(empty_body()))
}

fn quota_state_unavailable_response() -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    HttpResponse::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .body(box_body_from_bytes(
            crate::websocket::ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL
                .as_bytes()
                .to_vec(),
        ))
        .unwrap_or_else(|_error| HttpResponse::new(empty_body()))
}

fn empty_body() -> BoxBody<Bytes, AsyncHttpBodyError> {
    Empty::<Bytes>::new()
        .map_err(|never: Infallible| -> AsyncHttpBodyError { match never {} })
        .boxed()
}

#[derive(Clone, Debug)]
struct DbWriteAffinityOwnerRecorder {
    db_write_actor: DbWriteActor,
}

impl DbWriteAffinityOwnerRecorder {
    const fn new(db_write_actor: DbWriteActor) -> Self {
        Self { db_write_actor }
    }
}

impl AsyncHttpAffinityOwnerRecorder for DbWriteAffinityOwnerRecorder {
    fn record_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), HttpProxyError>> {
        Box::pin(async move {
            match self
                .db_write_actor
                .try_enqueue(DbWriteCommand::previous_response_affinity_owner(owner))
            {
                DbWriteEnqueueResult::Enqueued => Ok(()),
                DbWriteEnqueueResult::FullDegraded | DbWriteEnqueueResult::ClosedDegraded => {
                    Err(HttpProxyError::Selection {
                        reason:
                            crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
                    })
                }
            }
        })
    }
}

#[derive(Clone, Debug)]
struct AsyncSqliteProviderErrorObserver {
    writable_state_store: AsyncSqliteStateStore,
    selection_state_store: AsyncSqliteStateStore,
    active_reservations: RouteBandReservationBooks,
    runtime_exhaustions: RouteBandRuntimeExhaustions,
    route_band_queue_health: RouteBandQueueHealth,
    db_write_actor: DbWriteActor,
}

impl AsyncSqliteProviderErrorObserver {
    fn new(
        writable_state_store: AsyncSqliteStateStore,
        selection_state_store: AsyncSqliteStateStore,
        active_reservations: RouteBandReservationBooks,
        runtime_exhaustions: RouteBandRuntimeExhaustions,
        route_band_queue_health: RouteBandQueueHealth,
        db_write_actor: DbWriteActor,
    ) -> Self {
        Self {
            writable_state_store,
            selection_state_store,
            active_reservations,
            runtime_exhaustions,
            route_band_queue_health,
            db_write_actor,
        }
    }
}

impl AsyncProviderErrorObserver for AsyncSqliteProviderErrorObserver {
    fn mark_runtime_account_quota_exhausted(
        &self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        observed_unix_seconds: u64,
    ) -> Result<(), ProviderErrorObservationError> {
        mark_runtime_quota_exhausted(
            &self.runtime_exhaustions,
            route_band,
            account_id,
            observed_unix_seconds,
        )
        .map_err(ProviderErrorObservationError::from)
    }

    fn observe_provider_error<'a>(
        &'a self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
        Box::pin(async move {
            record_provider_error_observation(
                &self.writable_state_store,
                &account_id,
                route_band.as_str(),
                classification,
                observed_unix_seconds,
            )
            .await
            .map(|_classification| ())
            .map_err(ProviderErrorObservationError::from)
        })
    }

    fn enqueue_provider_quota_exhaustion(
        &self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    ) -> DbWriteEnqueueResult {
        self.db_write_actor
            .try_enqueue(DbWriteCommand::provider_quota_exhausted(
                account_id,
                route_band,
                classification,
                observed_unix_seconds,
            ))
    }

    fn route_band_post_exhaustion_outcome<'a>(
        &'a self,
        exhausted_account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        observed_unix_seconds: u64,
    ) -> BoxFuture<
        'a,
        Result<
            crate::account_selection::PostExhaustionRouteBandOutcome,
            ProviderErrorObservationError,
        >,
    > {
        Box::pin(async move {
            route_band_post_exhaustion_outcome(
                &self.selection_state_store,
                Some(&self.active_reservations),
                Some(&self.runtime_exhaustions),
                Some(&self.route_band_queue_health),
                route_band,
                &exhausted_account_id,
                observed_unix_seconds,
            )
            .await
            .map_err(ProviderErrorObservationError::from)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::MutexGuard;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use http_body_util::BodyExt;
    use http_body_util::StreamBody;
    use hyper::body::Frame;
    use tokio::io::AsyncWriteExt;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Debug, Default)]
    struct RecordingAsyncAffinityOwnerRecorder {
        records: Arc<Mutex<Vec<PreviousResponseAffinityOwnerRecord>>>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedHttpProviderError {
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
    }

    #[derive(Clone, Debug, Default)]
    struct RecordingAsyncProviderErrorObserver {
        records: Arc<Mutex<Vec<RecordedHttpProviderError>>>,
    }

    impl RecordingAsyncProviderErrorObserver {
        fn records(&self) -> Vec<RecordedHttpProviderError> {
            match self.records.lock() {
                Ok(records) => records.clone(),
                Err(error) => panic!("test provider observer lock should be available: {error}"),
            }
        }
    }

    impl AsyncProviderErrorObserver for RecordingAsyncProviderErrorObserver {
        fn observe_provider_error<'a>(
            &'a self,
            account_id: codex_router_core::ids::AccountId,
            route_band: RouteBand,
            classification: ProviderErrorClassification,
            _observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
            Box::pin(async move {
                match self.records.lock() {
                    Ok(mut records) => records.push(RecordedHttpProviderError {
                        account_id,
                        route_band,
                        classification,
                    }),
                    Err(error) => {
                        panic!("test provider observer lock should be available: {error}")
                    }
                }
                Ok(())
            })
        }

        fn enqueue_provider_quota_exhaustion(
            &self,
            account_id: codex_router_core::ids::AccountId,
            route_band: RouteBand,
            classification: ProviderErrorClassification,
            _observed_unix_seconds: u64,
        ) -> DbWriteEnqueueResult {
            match self.records.lock() {
                Ok(mut records) => records.push(RecordedHttpProviderError {
                    account_id,
                    route_band,
                    classification,
                }),
                Err(error) => panic!("test provider observer lock should be available: {error}"),
            }
            DbWriteEnqueueResult::Enqueued
        }
    }

    #[derive(Clone, Debug, Default)]
    struct NonblockingHttpQuotaObserver {
        durable_observation_called: Arc<AtomicBool>,
        enqueued_records: Arc<Mutex<Vec<RecordedHttpProviderError>>>,
    }

    impl NonblockingHttpQuotaObserver {
        fn enqueued_records(&self) -> Vec<RecordedHttpProviderError> {
            lock_test_mutex(&self.enqueued_records, "http quota enqueue records").clone()
        }
    }

    impl AsyncProviderErrorObserver for NonblockingHttpQuotaObserver {
        fn mark_runtime_account_quota_exhausted(
            &self,
            _account_id: codex_router_core::ids::AccountId,
            _route_band: RouteBand,
            _observed_unix_seconds: u64,
        ) -> Result<(), ProviderErrorObservationError> {
            Ok(())
        }

        fn observe_provider_error<'a>(
            &'a self,
            _account_id: codex_router_core::ids::AccountId,
            _route_band: RouteBand,
            _classification: ProviderErrorClassification,
            _observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
            Box::pin(async move {
                self.durable_observation_called
                    .store(true, Ordering::SeqCst);
                std::future::pending().await
            })
        }

        fn enqueue_provider_quota_exhaustion(
            &self,
            account_id: codex_router_core::ids::AccountId,
            route_band: RouteBand,
            classification: ProviderErrorClassification,
            _observed_unix_seconds: u64,
        ) -> DbWriteEnqueueResult {
            lock_test_mutex(&self.enqueued_records, "http quota enqueue records").push(
                RecordedHttpProviderError {
                    account_id,
                    route_band,
                    classification,
                },
            );
            DbWriteEnqueueResult::Enqueued
        }
    }

    #[derive(Clone, Debug, Default)]
    struct RecordingLoopbackConnectionErrorReporter {
        diagnostics: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingLoopbackConnectionErrorReporter {
        fn diagnostics(&self) -> Vec<String> {
            lock_test_mutex(&self.diagnostics, "connection diagnostics").clone()
        }
    }

    impl LoopbackConnectionErrorReporter for RecordingLoopbackConnectionErrorReporter {
        fn report_connection_error(&self, diagnostic: &str) {
            lock_test_mutex(&self.diagnostics, "connection diagnostics")
                .push(diagnostic.to_owned());
        }
    }

    #[derive(Clone, Debug, Default)]
    struct RecordingActiveClientLeaseReporter {
        released: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl RecordingActiveClientLeaseReporter {
        fn released(&self) -> Vec<(String, String)> {
            lock_test_mutex(&self.released, "active lease releases").clone()
        }
    }

    impl crate::account_selection::ActiveClientLeaseReporter for RecordingActiveClientLeaseReporter {
        fn record_acquired(
            &self,
            _route_band: &str,
            _reservation_handle: &codex_router_selection::reservation::ReservationHandle,
            _acquired_unix_seconds: u64,
            _active_pressure: u32,
        ) {
        }

        fn record_released(
            &self,
            route_band: &str,
            reservation_handle: &codex_router_selection::reservation::ReservationHandle,
        ) {
            lock_test_mutex(&self.released, "active lease releases").push((
                route_band.to_owned(),
                reservation_handle.reservation_id().as_str().to_owned(),
            ));
        }
    }

    #[tokio::test]
    async fn hyper_loopback_classifier_maps_closed_canceled_incomplete_to_client_disconnect() {
        let incomplete_message = hyper_error_from_client_bytes(
            b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\n\r\nshort",
        )
        .await;

        let diagnostic = loopback_connection_diagnostic(
            &LoopbackRouterRuntimeError::HyperConnection(incomplete_message),
        );

        assert_eq!(diagnostic.class(), "client_disconnect");
        assert_eq!(diagnostic.safe_reason(), "hyper_incomplete_message");
        assert_eq!(diagnostic.severity(), "debug");

        let malformed_request =
            hyper_error_from_client_bytes(b"\x16\x03\x01not-http\r\n\r\n").await;
        let diagnostic = loopback_connection_diagnostic(
            &LoopbackRouterRuntimeError::HyperConnection(malformed_request),
        );

        assert_eq!(diagnostic.class(), "malformed_request");
        assert_eq!(diagnostic.safe_reason(), "hyper_parse");
        assert_eq!(diagnostic.severity(), "warn");
    }

    #[tokio::test]
    async fn detached_hyper_connection_reports_scrubbed_root_cause_class() {
        let incomplete_message = hyper_error_from_client_bytes(
            b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\n\r\nshort",
        )
        .await;
        let reporter = Arc::new(RecordingLoopbackConnectionErrorReporter::default());
        let detached = tokio::spawn(async move {
            Err(LoopbackRouterRuntimeError::HyperConnection(
                incomplete_message,
            ))
        });

        supervise_detached_connection_handler(detached, reporter.clone());
        let diagnostic = wait_for_connection_diagnostic(&reporter).await;

        assert!(diagnostic.contains("class=client_disconnect"));
        assert!(diagnostic.contains("reason=hyper_incomplete_message"));
        assert!(diagnostic.contains("severity=debug"));
        assert!(!diagnostic.contains("/v1/responses"));
        assert!(!diagnostic.contains("Content-Length"));
    }

    #[tokio::test]
    async fn failed_hyper_response_body_drop_releases_active_reservation_and_mirror() {
        let active_reservations = RouteBandReservationBooks::default();
        let account_id = codex_router_core::ids::AccountId::new("acct_hyper_cleanup")
            .unwrap_or_else(|error| panic!("test account id should validate: {error}"));
        let reservation_handle = {
            let mut reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .entry(RouteBand::Responses.as_str().to_owned())
                .or_default()
                .reserve_next_at(account_id.clone(), 1, 1_000)
        };
        let lease_reporter = RecordingActiveClientLeaseReporter::default();
        let active_reservation_guard =
            crate::account_selection::ActiveReservationGuard::new_with_active_client_leases(
                active_reservations.clone(),
                RouteBand::Responses.as_str().to_owned(),
                reservation_handle.clone(),
                Some(Arc::new(lease_reporter.clone())),
            );
        let body =
            hold_active_reservation_until_body_drop(empty_body(), Some(active_reservation_guard));

        drop(body);

        let active_count_after_drop = {
            let reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .get(RouteBand::Responses.as_str())
                .map_or(0, |book| book.active_session_count(&account_id))
        };
        assert_eq!(
            active_count_after_drop, 0,
            "failed Hyper serving must release process-local active reservation when the response body is dropped"
        );
        assert_eq!(
            lease_reporter.released(),
            vec![(
                RouteBand::Responses.as_str().to_owned(),
                reservation_handle.reservation_id().as_str().to_owned(),
            )],
            "failed Hyper serving must also mirror active-client release"
        );
    }

    #[tokio::test]
    async fn hyper_connection_error_still_drains_upgrade_tasks() {
        let upgrade_tasks = SharedUpgradeTasks::default();
        let upgrade_task_was_drained = Arc::new(AtomicBool::new(false));
        let drained_marker = Arc::clone(&upgrade_task_was_drained);
        upgrade_tasks.lock().await.push(tokio::spawn(async move {
            drained_marker.store(true, Ordering::SeqCst);
            Ok(())
        }));

        let result = finish_hyper_connection_after_serve_result(
            Err(LoopbackRouterRuntimeError::Connection(
                ServerConnectionError::PartialRequest,
            )),
            Arc::clone(&upgrade_tasks),
        )
        .await;

        assert!(result.is_err());
        assert!(
            upgrade_task_was_drained.load(Ordering::SeqCst),
            "Hyper connection cleanup must drain upgrade tasks even when serve_connection fails"
        );
        assert!(
            upgrade_tasks.lock().await.is_empty(),
            "drained upgrade tasks must be removed from the shared task list"
        );
    }

    #[tokio::test]
    async fn provider_error_observer_marks_route_band_queue_health_degraded_when_queue_closed() {
        let database_path = test_database_path("provider_error_queue_health_closed");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("async state store should open: {error}"));
        let route_band_queue_health = RouteBandQueueHealth::default();
        let db_write_actor = DbWriteActor::start_on_handle(
            &tokio::runtime::Handle::current(),
            Arc::new(SqliteDbWriteRepository::new(store.clone())),
            route_band_queue_health.clone(),
            1,
        );
        db_write_actor.shutdown().await;
        let observer = AsyncSqliteProviderErrorObserver::new(
            store.clone(),
            store,
            RouteBandReservationBooks::default(),
            RouteBandRuntimeExhaustions::default(),
            route_band_queue_health.clone(),
            db_write_actor,
        );
        let account_id = codex_router_core::ids::AccountId::new("acct_queue_closed")
            .unwrap_or_else(|error| panic!("test account id should validate: {error}"));

        let enqueue_result = observer.enqueue_provider_quota_exhaustion(
            account_id,
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_000,
        );

        assert_eq!(enqueue_result, DbWriteEnqueueResult::ClosedDegraded);
        assert!(
            crate::account_selection::route_band_queue_health_allows_selection(
                &route_band_queue_health,
                RouteBand::Responses,
            )
            .is_err(),
            "closed DB write queues must degrade the whole route band for new selections"
        );
    }

    #[tokio::test]
    async fn runtime_writable_state_stores_use_distinct_pools_for_credential_db_write_and_maintenance()
     {
        let database_path = test_database_path("runtime_writable_store_pool_isolation");
        let stores = super::open_runtime_writable_state_stores(&database_path)
            .await
            .unwrap_or_else(|error| panic!("runtime writable stores should open: {error}"));

        let held_maintenance_connection = stores
            .maintenance_state_store
            .acquire_connection_for_test()
            .await
            .unwrap_or_else(|error| panic!("maintenance connection should be held: {error}"));
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            stores.credential_state_store.schema_version(),
        )
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("credential store must not wait behind a held maintenance pool connection")
        })
        .unwrap_or_else(|error| panic!("credential store should remain readable: {error}"));
        drop(held_maintenance_connection);

        let held_db_write_connection = stores
            .db_write_state_store
            .acquire_connection_for_test()
            .await
            .unwrap_or_else(|error| panic!("DB-write connection should be held: {error}"));
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            stores.credential_state_store.schema_version(),
        )
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("credential store must not wait behind a held DB-write pool connection")
        })
        .unwrap_or_else(|error| panic!("credential store should remain readable: {error}"));
        drop(held_db_write_connection);

        stores
            .credential_state_store
            .close()
            .await
            .unwrap_or_else(|error| panic!("credential store should close: {error}"));

        stores
            .db_write_state_store
            .schema_version()
            .await
            .unwrap_or_else(|error| {
                panic!("DB-write store must not share the credential pool: {error}")
            });
        stores
            .maintenance_state_store
            .schema_version()
            .await
            .unwrap_or_else(|error| {
                panic!("maintenance store must not share the credential pool: {error}")
            });

        stores
            .db_write_state_store
            .close()
            .await
            .unwrap_or_else(|error| panic!("DB-write store should close: {error}"));
        stores
            .maintenance_state_store
            .schema_version()
            .await
            .unwrap_or_else(|error| {
                panic!("maintenance store must not share the DB-write pool: {error}")
            });
    }

    #[tokio::test]
    async fn state_unavailable_selection_response_is_not_all_accounts_exhausted() {
        let response = http_error_response(HttpProxyError::Selection {
            reason: crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
        });

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("router error body should collect: {error}"))
            .to_bytes();
        let rendered = String::from_utf8(body.to_vec())
            .unwrap_or_else(|error| panic!("router error body should be utf-8: {error}"));

        assert!(rendered.contains("codex_router_quota_state_unavailable"));
        assert!(!rendered.contains("codex_router_all_accounts_exhausted"));
    }

    #[tokio::test]
    async fn account_attempt_limit_read_failure_maps_to_state_unavailable_not_all_exhausted() {
        let read_failure = StateStoreError::Sqlite {
            message: "list accounts unavailable".to_owned(),
        };

        let result = enabled_account_attempt_limit_from_accounts(Err(read_failure));

        assert!(
            result.is_err(),
            "failed account-list reads must not collapse to a one-attempt retry limit"
        );
        let response = quota_state_unavailable_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("router error body should collect: {error}"))
            .to_bytes();
        let rendered = String::from_utf8(body.to_vec())
            .unwrap_or_else(|error| panic!("router error body should be utf-8: {error}"));

        assert!(rendered.contains("codex_router_quota_state_unavailable"));
        assert!(!rendered.contains("codex_router_all_accounts_exhausted"));
    }

    async fn hyper_error_from_client_bytes(bytes: &'static [u8]) -> hyper::Error {
        let listener = TokioTcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("test hyper listener should bind: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("test hyper listener address should read: {error}"));
        let server = tokio::spawn(async move {
            let (stream, _peer_addr) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("test hyper listener should accept: {error}"));
            let io = TokioIo::new(stream);
            let service = service_fn(|request: HttpRequest<Incoming>| async move {
                request.into_body().collect().await?;
                Ok::<_, hyper::Error>(HttpResponse::new(Full::new(Bytes::new())))
            });
            http1::Builder::new().serve_connection(io, service).await
        });
        let mut client = tokio::net::TcpStream::connect(address)
            .await
            .unwrap_or_else(|error| panic!("test hyper client should connect: {error}"));
        client
            .write_all(bytes)
            .await
            .unwrap_or_else(|error| panic!("test hyper client should write: {error}"));
        client
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("test hyper client should shutdown: {error}"));

        match server
            .await
            .unwrap_or_else(|error| panic!("test hyper server task should join: {error}"))
        {
            Ok(()) => panic!("test hyper server should fail for malformed client bytes"),
            Err(error) => error,
        }
    }

    async fn wait_for_connection_diagnostic(
        reporter: &RecordingLoopbackConnectionErrorReporter,
    ) -> String {
        let started_at = tokio::time::Instant::now();
        loop {
            if let Some(diagnostic) = reporter.diagnostics().into_iter().next() {
                return diagnostic;
            }
            assert!(
                started_at.elapsed() < Duration::from_secs(2),
                "connection diagnostic should be reported"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn test_database_path(name: &str) -> PathBuf {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "codex-router-proxy-server-{name}-{}-{counter}.sqlite",
            std::process::id()
        ))
    }

    fn lock_test_mutex<'a, T>(mutex: &'a Mutex<T>, label: &str) -> MutexGuard<'a, T> {
        mutex
            .lock()
            .unwrap_or_else(|error| panic!("{label} lock should be available: {error}"))
    }

    impl RecordingAsyncAffinityOwnerRecorder {
        fn records(&self) -> Vec<PreviousResponseAffinityOwnerRecord> {
            lock_test_mutex(&self.records, "affinity recorder").clone()
        }
    }

    impl AsyncHttpAffinityOwnerRecorder for RecordingAsyncAffinityOwnerRecorder {
        fn record_affinity_owner<'a>(
            &'a self,
            owner: PreviousResponseAffinityOwnerRecord,
        ) -> BoxFuture<'a, Result<(), HttpProxyError>> {
            Box::pin(async move {
                match self.records.lock() {
                    Ok(mut records) => records.push(owner),
                    Err(error) => panic!("test recorder lock should be available: {error}"),
                }
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn async_http_affinity_scan_stops_at_explicit_bounds_without_gating_body() {
        let recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
            Ok(account_id) => account_id,
            Err(error) => panic!("test account id should validate: {error}"),
        };
        let affinity_secret = match codex_router_core::affinity::RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ) {
            Ok(secret) => secret,
            Err(error) => panic!("test affinity secret should validate: {error}"),
        };
        let completion = StreamingHttpProxyCompletion::new_for_test(
            Some(affinity_secret),
            account_id,
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        );
        let late_response_id =
            Bytes::from_static(br#"data: {"id":"resp_after_bound_should_not_record"}\n\n"#);
        let chunks = vec![
            Bytes::from(vec![b'a'; HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES]),
            late_response_id.clone(),
        ];
        let body_stream = futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok::<_, AsyncHttpBodyError>(Frame::data(chunk))),
        );
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let affinity_record_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(recorder.clone()),
            None,
            affinity_record_tasks.clone(),
        );
        let forwarded_bytes = match forwarded_body.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => panic!("forwarded body should collect: {error}"),
        };
        affinity_record_tasks.close();
        affinity_record_tasks.wait().await;

        assert!(forwarded_bytes.ends_with(&late_response_id));
        assert_eq!(recorder.records(), Vec::new());
    }

    #[tokio::test]
    async fn async_http_usage_limit_body_is_forwarded_unchanged_and_observed() {
        let affinity_recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let provider_error_observer = RecordingAsyncProviderErrorObserver::default();
        let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
            Ok(account_id) => account_id,
            Err(error) => panic!("test account id should validate: {error}"),
        };
        let completion = StreamingHttpProxyCompletion::new_for_test(
            None,
            account_id.clone(),
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        )
        .with_route_band_for_test(RouteBand::Responses);
        let usage_limit_body = Bytes::from_static(
            br#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#,
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(usage_limit_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let metadata_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(affinity_recorder.clone()),
            Some(Arc::new(provider_error_observer.clone())),
            metadata_tasks.clone(),
        );

        let forwarded_bytes = match forwarded_body.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => panic!("forwarded body should collect: {error}"),
        };
        metadata_tasks.close();
        metadata_tasks.wait().await;

        assert_eq!(forwarded_bytes, usage_limit_body);
        assert_eq!(affinity_recorder.records(), Vec::new());
        assert_eq!(
            provider_error_observer.records(),
            vec![RecordedHttpProviderError {
                account_id,
                route_band: RouteBand::Responses,
                classification: ProviderErrorClassification::AccountQuotaExhausted,
            }]
        );
    }

    #[tokio::test]
    async fn precommit_http_usage_limit_body_requests_account_retry_before_commit() {
        let usage_limit_body = Bytes::from_static(
            br#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#,
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(usage_limit_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let response =
            AsyncStreamingHttpProxyResponse::new(429, HeaderCollection::new(Vec::new()), body);

        match split_precommit_http_quota_response(response).await {
            Ok(PrecommitHttpResponseProbe::AccountQuotaExhausted { body }) => {
                assert_eq!(body, usage_limit_body.to_vec());
            }
            Ok(PrecommitHttpResponseProbe::Forward(_response)) => {
                panic!("quota response should request account retry before commit");
            }
            Err(error) => panic!("quota response should classify before commit: {error}"),
        }
    }

    #[tokio::test]
    async fn precommit_http_quota_retry_enqueues_without_awaiting_durable_observation() {
        let observer = Arc::new(NonblockingHttpQuotaObserver::default());
        let account_id = codex_router_core::ids::AccountId::new("acct_http_quota")
            .unwrap_or_else(|error| panic!("test account id should validate: {error}"));

        tokio::time::timeout(Duration::from_millis(50), async {
            observe_precommit_http_quota_exhaustion_for_retry(
                Some(observer.clone()),
                account_id.clone(),
                RouteBand::Responses,
                1_000,
            )
        })
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("precommit quota retry must not await durable provider-error observation")
        })
        .unwrap_or_else(|error| panic!("precommit quota enqueue should succeed: {error:?}"));

        assert!(!observer.durable_observation_called.load(Ordering::SeqCst));
        assert_eq!(
            observer.enqueued_records(),
            vec![RecordedHttpProviderError {
                account_id,
                route_band: RouteBand::Responses,
                classification: ProviderErrorClassification::AccountQuotaExhausted,
            }]
        );
    }

    #[tokio::test]
    async fn postcommit_http_quota_observation_uses_db_write_actor_without_direct_sqlite_wait() {
        let affinity_recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let observer = Arc::new(NonblockingHttpQuotaObserver::default());
        let account_id = codex_router_core::ids::AccountId::new("acct_http_postcommit_quota")
            .unwrap_or_else(|error| panic!("test account id should validate: {error}"));
        let completion = StreamingHttpProxyCompletion::new_for_test(
            None,
            account_id.clone(),
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        )
        .with_route_band_for_test(RouteBand::Responses);
        let usage_limit_body = Bytes::from_static(
            br#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#,
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(usage_limit_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let metadata_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(affinity_recorder),
            Some(observer.clone()),
            metadata_tasks.clone(),
        );

        let forwarded_bytes = forwarded_body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("forwarded body should collect: {error}"))
            .to_bytes();
        metadata_tasks.close();
        tokio::time::timeout(Duration::from_millis(50), metadata_tasks.wait())
            .await
            .unwrap_or_else(|_elapsed| {
                panic!("postcommit quota observation must not wait on direct durable observer")
            });

        assert_eq!(forwarded_bytes, usage_limit_body);
        assert!(!observer.durable_observation_called.load(Ordering::SeqCst));
        assert_eq!(
            observer.enqueued_records(),
            vec![RecordedHttpProviderError {
                account_id,
                route_band: RouteBand::Responses,
                classification: ProviderErrorClassification::AccountQuotaExhausted,
            }]
        );
    }

    #[tokio::test]
    async fn precommit_http_non_error_body_replays_exact_bytes() {
        let response_body = Bytes::from_static(br#"data: {"id":"resp-ok"}\n\n"#);
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(response_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let response =
            AsyncStreamingHttpProxyResponse::new(200, HeaderCollection::new(Vec::new()), body);

        match split_precommit_http_quota_response(response).await {
            Ok(PrecommitHttpResponseProbe::Forward(response)) => {
                let (_status, _headers, body) = response.into_parts();
                let forwarded_bytes = match body.collect().await {
                    Ok(collected) => collected.to_bytes(),
                    Err(error) => panic!("forwarded body should collect: {error}"),
                };
                assert_eq!(forwarded_bytes, response_body);
            }
            Ok(PrecommitHttpResponseProbe::AccountQuotaExhausted { .. }) => {
                panic!("non-error response should pass through");
            }
            Err(error) => panic!("non-error response should pass through: {error}"),
        }
    }

    #[tokio::test]
    async fn async_sse_usage_limit_data_line_is_forwarded_unchanged_and_observed() {
        let affinity_recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let provider_error_observer = RecordingAsyncProviderErrorObserver::default();
        let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
            Ok(account_id) => account_id,
            Err(error) => panic!("test account id should validate: {error}"),
        };
        let completion = StreamingHttpProxyCompletion::new_for_test(
            None,
            account_id.clone(),
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        )
        .with_route_band_for_test(RouteBand::Responses);
        let provider_error_json = br#"{"type":"error","error":{"code":"usage_limit_reached"}}"#;
        let sse_body = Bytes::from(
            [
                b"event: error\n".as_slice(),
                b"data: ".as_slice(),
                provider_error_json.as_slice(),
                b"\n\n".as_slice(),
            ]
            .concat(),
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(sse_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let metadata_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(affinity_recorder.clone()),
            Some(Arc::new(provider_error_observer.clone())),
            metadata_tasks.clone(),
        );

        let forwarded_bytes = match forwarded_body.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => panic!("forwarded body should collect: {error}"),
        };
        metadata_tasks.close();
        metadata_tasks.wait().await;

        assert_eq!(forwarded_bytes, sse_body);
        assert_eq!(affinity_recorder.records(), Vec::new());
        assert_eq!(
            provider_error_observer.records(),
            vec![RecordedHttpProviderError {
                account_id,
                route_band: RouteBand::Responses,
                classification: ProviderErrorClassification::AccountQuotaExhausted,
            }]
        );
    }

    #[tokio::test]
    async fn async_http_ambiguous_quota_text_is_forwarded_unchanged_without_observation() {
        let affinity_recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let provider_error_observer = RecordingAsyncProviderErrorObserver::default();
        let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
            Ok(account_id) => account_id,
            Err(error) => panic!("test account id should validate: {error}"),
        };
        let completion = StreamingHttpProxyCompletion::new_for_test(
            None,
            account_id,
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        )
        .with_route_band_for_test(RouteBand::Responses);
        let model_text_body = Bytes::from_static(
            br#"{"type":"response.output_text.delta","delta":"usage_limit_reached is only text"}"#,
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(model_text_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let metadata_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(affinity_recorder.clone()),
            Some(Arc::new(provider_error_observer.clone())),
            metadata_tasks.clone(),
        );

        let forwarded_bytes = match forwarded_body.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => panic!("forwarded body should collect: {error}"),
        };
        metadata_tasks.close();
        metadata_tasks.wait().await;

        assert_eq!(forwarded_bytes, model_text_body);
        assert_eq!(affinity_recorder.records(), Vec::new());
        assert_eq!(provider_error_observer.records(), Vec::new());
    }
}

/// Assembled router runtime failure.
#[derive(Debug, thiserror::Error)]
pub enum LoopbackRouterRuntimeError {
    /// Binding the loopback listener failed.
    #[error(transparent)]
    Bind(#[from] ServerBindError),
    /// Accepting a loopback connection failed.
    #[error("failed accepting loopback router connection")]
    Accept(#[source] std::io::Error),
    /// Opening or reading SQLite state failed.
    #[error(transparent)]
    State(#[from] StateStoreError),
    /// Opening runtime credential resources failed.
    #[error(transparent)]
    CredentialResources(#[from] ProxyRuntimeCredentialResourcesOpenError),
    /// Runtime system clock is before Unix epoch.
    #[error("system clock is before Unix epoch")]
    SystemClock(#[source] std::time::SystemTimeError),
    /// Tokio runtime creation failed.
    #[error("failed to create Tokio runtime")]
    TokioRuntime(#[source] std::io::Error),
    /// Hyper connection serving failed.
    #[error("failed serving Hyper loopback connection")]
    HyperConnection(#[source] hyper::Error),
    /// Hyper request body collection failed.
    #[error("failed reading Hyper request body")]
    HyperBody(#[source] hyper::Error),
    /// Hyper connection task failed.
    #[error("Hyper connection task failed")]
    ConnectionJoin(#[source] tokio::task::JoinError),
    /// Serving a loopback connection failed.
    #[cfg(test)]
    #[error(transparent)]
    Connection(#[from] ServerConnectionError),
    /// Serving a WebSocket tunnel failed.
    #[error(transparent)]
    WebSocket(#[from] crate::websocket::WebSocketTunnelError),
}

/// Server bind validation and runtime errors.
#[derive(Debug, thiserror::Error)]
pub enum ServerBindError {
    /// Host was not an IP address.
    #[error("invalid listen host `{host}`")]
    InvalidHost {
        /// Original host text.
        host: String,
        /// Parse failure.
        source: AddrParseError,
    },
    /// Host was valid but not loopback.
    #[error("listen host `{host}` is not loopback")]
    NonLoopback {
        /// Rejected host text.
        host: String,
    },
    /// TCP bind failed for the validated address.
    #[error("failed to bind loopback listener at {address}")]
    Bind {
        /// Address passed to the kernel.
        address: SocketAddr,
        /// I/O failure from bind or local address lookup.
        source: std::io::Error,
    },
}

impl PartialEq for ServerBindError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::InvalidHost { host: left, .. }, Self::InvalidHost { host: right, .. }) => {
                left == right
            }
            (Self::NonLoopback { host: left }, Self::NonLoopback { host: right }) => left == right,
            (Self::Bind { address: left, .. }, Self::Bind { address: right, .. }) => left == right,
            _ => false,
        }
    }
}

/// Adapter for one loopback HTTP/1.x connection.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct LoopbackHttpAdapter;

#[cfg(test)]
impl LoopbackHttpAdapter {
    /// Handles one accepted HTTP connection.
    pub fn handle_connection<H>(
        mut stream: TcpStream,
        handler: &H,
    ) -> Result<(), ServerConnectionError>
    where
        H: HttpRequestHandler,
    {
        let request = read_http_request(&mut stream)?;
        let response = match handler.handle_request(request) {
            Ok(response) => response,
            Err(HttpProxyError::LocalAuth { .. }) => {
                write_http_error_response(&mut stream, 401, "Unauthorized")?;
                return Ok(());
            }
            Err(HttpProxyError::Selection { .. }) => {
                write_http_error_response(&mut stream, 503, "Service Unavailable")?;
                return Ok(());
            }
            Err(HttpProxyError::ProviderCredential { .. }) => {
                write_http_error_response(&mut stream, 502, "Bad Gateway")?;
                return Ok(());
            }
            Err(error) => return Err(ServerConnectionError::Proxy(error)),
        };
        write_http_response(&mut stream, response)?;

        Ok(())
    }

    /// Handles one accepted HTTP connection without buffering response bodies.
    pub fn handle_streaming_connection<H>(
        mut stream: TcpStream,
        handler: &H,
    ) -> Result<(), ServerConnectionError>
    where
        H: StreamingHttpRequestHandler,
    {
        let request = read_http_request(&mut stream)?;
        let response = match handler.handle_streaming_request(request) {
            Ok(response) => response,
            Err(HttpProxyError::LocalAuth { .. }) => {
                write_http_error_response(&mut stream, 401, "Unauthorized")?;
                return Ok(());
            }
            Err(HttpProxyError::Selection { .. }) => {
                write_http_error_response(&mut stream, 503, "Service Unavailable")?;
                return Ok(());
            }
            Err(HttpProxyError::ProviderCredential { .. }) => {
                write_http_error_response(&mut stream, 502, "Bad Gateway")?;
                return Ok(());
            }
            Err(error) => return Err(ServerConnectionError::Proxy(error)),
        };
        write_streaming_http_response(&mut stream, response)?;

        Ok(())
    }
}

/// Bounded loopback HTTP server accept loop.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct LoopbackHttpServer;

#[cfg(test)]
impl LoopbackHttpServer {
    /// Accepts and handles loopback HTTP connections until the bound is reached.
    pub fn serve_connections<H>(
        listener: TcpListener,
        handler: &H,
        max_connections: usize,
    ) -> Result<usize, ServerConnectionError>
    where
        H: HttpRequestHandler,
    {
        let mut handled_connections = 0_usize;
        while handled_connections < max_connections {
            let (stream, _peer_addr) = listener.accept().map_err(ServerConnectionError::Accept)?;
            LoopbackHttpAdapter::handle_connection(stream, handler)?;
            handled_connections += 1;
        }

        Ok(handled_connections)
    }

    /// Accepts and handles loopback HTTP connections without buffering response bodies.
    pub fn serve_streaming_connections<H>(
        listener: TcpListener,
        handler: &H,
        max_connections: usize,
    ) -> Result<usize, ServerConnectionError>
    where
        H: StreamingHttpRequestHandler,
    {
        let mut handled_connections = 0_usize;
        while handled_connections < max_connections {
            let (stream, _peer_addr) = listener.accept().map_err(ServerConnectionError::Accept)?;
            LoopbackHttpAdapter::handle_streaming_connection(stream, handler)?;
            handled_connections += 1;
        }

        Ok(handled_connections)
    }
}

#[cfg(test)]
fn read_http_request(stream: &mut TcpStream) -> Result<HttpProxyRequest, ServerConnectionError> {
    let mut request_bytes = Vec::new();
    let parsed_head = loop {
        if request_bytes.len() > MAX_HTTP_HEADER_BYTES {
            return Err(ServerConnectionError::HeaderTooLarge);
        }
        if let Some(parsed_head) = parse_http_request_head(&request_bytes)? {
            break parsed_head;
        }

        let mut buffer = [0_u8; 4096];
        let read = stream
            .read(&mut buffer)
            .map_err(ServerConnectionError::Read)?;
        if read == 0 {
            return Err(ServerConnectionError::PartialRequest);
        }
        request_bytes.extend_from_slice(&buffer[..read]);
    };
    let body_end = parsed_head
        .header_length
        .checked_add(parsed_head.content_length)
        .ok_or(ServerConnectionError::BodyTooLarge)?;
    while request_bytes.len() < body_end {
        let mut buffer = [0_u8; 4096];
        let read = stream
            .read(&mut buffer)
            .map_err(ServerConnectionError::Read)?;
        if read == 0 {
            return Err(ServerConnectionError::PartialBody);
        }
        request_bytes.extend_from_slice(&buffer[..read]);
    }

    let body = request_bytes[parsed_head.header_length..body_end].to_vec();
    let mut request = HttpProxyRequest::new(parsed_head.method, parsed_head.path);
    for header in parsed_head.headers {
        request = request.with_header(header);
    }

    Ok(request.with_body(body))
}

#[derive(Debug)]
#[cfg(test)]
struct ParsedHttpRequestHead {
    method: Method,
    path: String,
    headers: Vec<Header>,
    header_length: usize,
    content_length: usize,
}

#[cfg(test)]
fn parse_http_request_head(
    request_bytes: &[u8],
) -> Result<Option<ParsedHttpRequestHead>, ServerConnectionError> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed_request = httparse::Request::new(&mut headers);
    let header_length = match parsed_request.parse(request_bytes) {
        Ok(httparse::Status::Complete(header_length)) => header_length,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(source) => return Err(ServerConnectionError::Parse(source)),
    };
    let method = method_from_http(
        parsed_request
            .method
            .ok_or(ServerConnectionError::MissingMethod)?,
    );
    let path = parsed_request
        .path
        .ok_or(ServerConnectionError::MissingPath)?
        .to_owned();
    let content_length = parsed_request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-length"))
        .and_then(|header| std::str::from_utf8(header.value).ok())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let mut request_headers = Vec::new();
    for header in parsed_request.headers.iter() {
        let value = std::str::from_utf8(header.value).map_err(ServerConnectionError::HeaderUtf8)?;
        request_headers.push(Header::new(header.name, value));
    }

    Ok(Some(ParsedHttpRequestHead {
        method,
        path,
        headers: request_headers,
        header_length,
        content_length,
    }))
}

#[cfg(test)]
fn method_from_http(method: &str) -> Method {
    match method {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => Method::Other,
    }
}

#[cfg(test)]
fn write_http_response(
    stream: &mut TcpStream,
    response: HttpProxyResponse,
) -> Result<(), ServerConnectionError> {
    write!(stream, "HTTP/1.1 {} OK\r\n", response.status())
        .map_err(ServerConnectionError::Write)?;
    for header in response.headers().as_slice() {
        write!(stream, "{}: {}\r\n", header.name(), header.value())
            .map_err(ServerConnectionError::Write)?;
    }
    write!(stream, "Content-Length: {}\r\n\r\n", response.body().len())
        .map_err(ServerConnectionError::Write)?;
    stream
        .write_all(response.body())
        .map_err(ServerConnectionError::Write)
}

#[cfg(test)]
fn write_http_error_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
) -> Result<(), ServerConnectionError> {
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .map_err(ServerConnectionError::Write)
}

#[cfg(test)]
fn write_streaming_http_response(
    stream: &mut TcpStream,
    mut response: StreamingHttpProxyResponse,
) -> Result<(), ServerConnectionError> {
    write!(stream, "HTTP/1.1 {} OK\r\n", response.status())
        .map_err(ServerConnectionError::Write)?;
    for header in response.headers().as_slice() {
        write!(stream, "{}: {}\r\n", header.name(), header.value())
            .map_err(ServerConnectionError::Write)?;
    }
    stream
        .write_all(b"\r\n")
        .map_err(ServerConnectionError::Write)?;
    stream.flush().map_err(ServerConnectionError::Write)?;
    std::io::copy(response.body_mut(), stream).map_err(ServerConnectionError::Write)?;
    stream.flush().map_err(ServerConnectionError::Write)?;

    Ok(())
}

/// One-connection loopback HTTP adapter failure.
#[cfg(test)]
#[derive(Debug, thiserror::Error)]
pub enum ServerConnectionError {
    /// Accepting a loopback connection failed.
    #[error("failed accepting loopback HTTP connection")]
    Accept(#[source] std::io::Error),
    /// Reading from the accepted stream failed.
    #[error("failed reading HTTP connection")]
    Read(#[source] std::io::Error),
    /// Request bytes were not a complete HTTP request.
    #[error("partial HTTP request")]
    PartialRequest,
    /// Request headers exceeded the local parsing bound.
    #[error("HTTP headers too large")]
    HeaderTooLarge,
    /// Request body was incomplete.
    #[error("partial HTTP body")]
    PartialBody,
    /// Request body size overflowed local indexing.
    #[error("HTTP body too large")]
    BodyTooLarge,
    /// HTTP parser rejected request bytes.
    #[error("failed parsing HTTP request")]
    Parse(#[source] httparse::Error),
    /// Header value was not valid UTF-8.
    #[error("HTTP header value was not valid UTF-8")]
    HeaderUtf8(#[source] std::str::Utf8Error),
    /// Request path was missing.
    #[error("HTTP request path was missing")]
    MissingPath,
    /// Request method was missing.
    #[error("HTTP request method was missing")]
    MissingMethod,
    /// Proxy service rejected or failed the request.
    #[error(transparent)]
    Proxy(#[from] HttpProxyError),
    /// Writing to the accepted stream failed.
    #[error("failed writing HTTP response")]
    Write(#[source] std::io::Error),
}
