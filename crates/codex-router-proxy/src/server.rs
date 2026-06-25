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
use codex_router_state::affinity_owner::AffinitySourceTransport;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;

use crate::account_selection::AsyncRepositoryBackedAccountSelector;
use crate::account_selection::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS;
use crate::account_selection::RouteBandAccountHolds;
use crate::account_selection::RouteBandReservationBooks;
use crate::account_selection::RouteBandWeightedSelectors;
use crate::credential_runtime::AsyncProxyCredentialResolverFactory;
use crate::credential_runtime::ProxyRuntimeCredentialResources;
use crate::credential_runtime::ProxyRuntimeCredentialResourcesOpenError;
use crate::credential_runtime::RuntimeAffinitySecretProvider;
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
use crate::provider_error::AsyncProviderErrorObserver;
use crate::provider_error::ProviderErrorClassification;
use crate::provider_error::classify_provider_error_envelope;
use crate::provider_error::record_provider_error_observation;
use crate::routes::Method;
use crate::routes::RouteClass;
use crate::routes::classify_route;
use crate::upstream::HyperHttpUpstreamTransport;
use crate::upstream::UpstreamEndpoint;
use crate::websocket::AsyncWebSocketTunnel;
use crate::websocket::FirstFramePolicy;
use crate::websocket::WebSocketHandshakeRequest;
use crate::websocket::WebSocketProtocolRouter;
use crate::websocket::WebSocketRegistrySnapshot;
use crate::websocket::WebSocketRevocationRegistry;

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
    state_database_path: PathBuf,
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
        let credential_resources = ProxyRuntimeCredentialResources::open(
            &config.secret_store_root,
            config.fixed_now_unix_seconds,
        )?;
        let affinity_secret_provider = credential_resources.affinity_secret_provider();
        let credential_factory = credential_resources.credential_factory();
        let affinity_state_store =
            runtime.block_on(AsyncSqliteStateStore::open(&config.state_database_path))?;
        let affinity_owner_recorder =
            Arc::new(AsyncSqliteAffinityOwnerRecorder::new(affinity_state_store));
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

        Ok(Self {
            runtime,
            server,
            state_database_path: config.state_database_path,
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
            credential_factory,
            fixed_now_unix_seconds: config.fixed_now_unix_seconds,
            connection_error_reporter: Arc::new(StderrLoopbackConnectionErrorReporter),
        })
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
            state_database_path: self.state_database_path.clone(),
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
            fixed_now_unix_seconds: self.fixed_now_unix_seconds,
            session_shutdown,
        }
    }
}

fn handle_connection_join_result(
    joined: Result<Result<(), LoopbackRouterRuntimeError>, JoinError>,
) -> Result<(), LoopbackRouterRuntimeError> {
    match joined {
        Ok(Ok(())) => Ok(()),
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
            Ok(Err(error)) => reporter.report_connection_error(&format!(
                "codex-router loopback connection failed: {error}"
            )),
            Err(source) => reporter.report_connection_error(&format!(
                "codex-router loopback connection task failed: {source}"
            )),
        }
    });
}

#[derive(Clone)]
struct LoopbackProtocolConnectionHandler {
    state_database_path: PathBuf,
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
        http_builder
            .serve_connection(io, service)
            .with_upgrades()
            .await
            .map_err(LoopbackRouterRuntimeError::HyperConnection)?;
        let mut upgrade_task_guard = upgrade_tasks.lock().await;
        let drained_upgrade_tasks = std::mem::take(&mut *upgrade_task_guard);
        drop(upgrade_task_guard);
        for upgrade_task in drained_upgrade_tasks {
            match upgrade_task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error),
                Err(source) => return Err(LoopbackRouterRuntimeError::ConnectionJoin(source)),
            }
        }

        Ok(())
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
        let (upgrade_response, websocket) = match hyper_tungstenite::upgrade(&mut request, None) {
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
        let state_store = AsyncSqliteStateStore::open(&self.state_database_path).await?;
        let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_and_reservations(
            &state_store,
            Arc::clone(&self.weighted_selectors),
            Arc::clone(&self.account_holds),
            Arc::clone(&self.active_reservations),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            self.runtime_clock(),
        );
        let credential_resolver = self
            .credential_factory
            .resolver_for_state(state_store.clone());
        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024 * 1024));
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
            state_store.clone(),
        )))
        .with_local_peer_addr(local_peer_addr);
        let upstream_url = self.upstream_endpoint.websocket_url_for_path(&path);
        tunnel
            .handle_upgraded_connection(local_websocket, handshake, upstream_url.as_str())
            .await
            .map_err(LoopbackRouterRuntimeError::WebSocket)
    }

    async fn handle_hyper_http_request(
        self: Arc<Self>,
        request: HttpRequest<Incoming>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        let (request, body) = match hyper_request_to_streaming_proxy_request(request).await {
            Ok(request) => request,
            Err(_error) => return empty_response(StatusCode::BAD_REQUEST),
        };
        let prepared = match self
            .prepare_async_streaming_http_request_async(request, body)
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

        self.async_streaming_http_response_to_hyper(response, completion)
    }

    async fn prepare_async_streaming_http_request_async(
        &self,
        request: HttpProxyRequest,
        body: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> Result<PreparedAsyncStreamingHttpProxyRequest, HttpProxyError> {
        let state_store = AsyncSqliteStateStore::open(&self.state_database_path)
            .await
            .map_err(|_error| HttpProxyError::Selection {
                reason: crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
            })?;
        let credential_resolver = self
            .credential_factory
            .resolver_for_state(state_store.clone());
        let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_and_reservations(
            &state_store,
            Arc::clone(&self.weighted_selectors),
            Arc::clone(&self.account_holds),
            Arc::clone(&self.active_reservations),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            self.runtime_clock(),
        );
        let service = AuthenticatedHttpProxyService::new(
            &self.auth_gate,
            &selector,
            &credential_resolver,
            &self.upstream,
        )
        .with_affinity_secret_provider(&self.affinity_secret_provider)
        .with_provider_error_observer(Arc::new(AsyncSqliteProviderErrorObserver::new(
            state_store.clone(),
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
        response: AsyncStreamingHttpProxyResponse,
        completion: crate::http_sse::StreamingHttpProxyCompletion,
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
                    panic!("system clock must remain after Unix epoch for routing: {error}")
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
const HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES: usize = 64 * 1024;
const HTTP_RESPONSE_AFFINITY_SCAN_MAX_EVENTS: usize = 64;

async fn hyper_request_to_streaming_proxy_request(
    request: HttpRequest<Incoming>,
) -> Result<(HttpProxyRequest, BoxBody<Bytes, AsyncHttpBodyError>), LoopbackRouterRuntimeError> {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str)
        .to_owned();
    let (metadata_prefix, replay_body) = bounded_request_metadata_body(body)
        .await
        .map_err(LoopbackRouterRuntimeError::HyperBody)?;
    let mut proxy_request = HttpProxyRequest::new(method_from_hyper(&parts.method), path);
    for (name, value) in &parts.headers {
        if let Ok(value) = value.to_str() {
            proxy_request = proxy_request.with_header(Header::new(name.as_str(), value));
        }
    }
    let streaming_body = replay_body.map_err(incoming_body_error).boxed();

    Ok((proxy_request.with_body(metadata_prefix), streaming_body))
}

async fn bounded_request_metadata_body(
    mut body: Incoming,
) -> Result<(Vec<u8>, PrefixFramesThenIncomingBody), hyper::Error> {
    let mut metadata_prefix = Vec::new();
    let mut replay_frames = VecDeque::new();
    while metadata_prefix.len() < HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES {
        let Some(frame) = body.frame().await.transpose()? else {
            break;
        };
        if let Some(data) = frame.data_ref() {
            let remaining_bytes = HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES - metadata_prefix.len();
            let bytes_to_scan = data.len().min(remaining_bytes);
            metadata_prefix.extend_from_slice(&data[..bytes_to_scan]);
        }
        let metadata_is_complete = request_metadata_prefix_is_complete_json(&metadata_prefix);
        replay_frames.push_back(frame);
        if metadata_is_complete || metadata_prefix.len() >= HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES {
            break;
        }
    }

    Ok((
        metadata_prefix,
        PrefixFramesThenIncomingBody::new(replay_frames, body),
    ))
}

fn request_metadata_prefix_is_complete_json(metadata_prefix: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(metadata_prefix).is_ok()
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
        self.inner.size_hint()
    }
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
            buffered.extend_from_slice(&data[..bytes_to_scan]);
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
                if let Some(observer) = provider_error_observer.as_ref() {
                    spawn_async_provider_error_observation(
                        Arc::clone(observer),
                        account_id.clone(),
                        route_band,
                        provider_error_body,
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
    &bytes[start..end]
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
    body: Vec<u8>,
    affinity_record_tasks: TaskTracker,
) {
    affinity_record_tasks.spawn(async move {
        observer
            .observe_provider_error(
                account_id,
                route_band,
                body,
                current_unix_seconds().map_or(0, |seconds| seconds),
            )
            .await;
    });
}

fn incoming_body_error(error: hyper::Error) -> AsyncHttpBodyError {
    Box::new(error)
}

fn http_error_response(error: HttpProxyError) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    match error {
        HttpProxyError::LocalAuth { .. } => empty_response(StatusCode::UNAUTHORIZED),
        HttpProxyError::Selection { .. } => empty_response(StatusCode::SERVICE_UNAVAILABLE),
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

fn empty_body() -> BoxBody<Bytes, AsyncHttpBodyError> {
    Empty::<Bytes>::new()
        .map_err(|never: Infallible| -> AsyncHttpBodyError { match never {} })
        .boxed()
}

#[derive(Clone, Debug)]
struct AsyncSqliteAffinityOwnerRecorder {
    state_store: AsyncSqliteStateStore,
}

impl AsyncSqliteAffinityOwnerRecorder {
    const fn new(state_store: AsyncSqliteStateStore) -> Self {
        Self { state_store }
    }
}

impl AsyncHttpAffinityOwnerRecorder for AsyncSqliteAffinityOwnerRecorder {
    fn record_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), HttpProxyError>> {
        Box::pin(async move {
            self.state_store
                .write_previous_response_owner(&owner)
                .await
                .map_err(|_error| HttpProxyError::Selection {
                    reason:
                        crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
                })
        })
    }
}

#[derive(Clone, Debug)]
struct AsyncSqliteProviderErrorObserver {
    state_store: AsyncSqliteStateStore,
}

impl AsyncSqliteProviderErrorObserver {
    const fn new(state_store: AsyncSqliteStateStore) -> Self {
        Self { state_store }
    }
}

impl AsyncProviderErrorObserver for AsyncSqliteProviderErrorObserver {
    fn observe_provider_error<'a>(
        &'a self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        body: Vec<u8>,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let _result = record_provider_error_observation(
                &self.state_store,
                &account_id,
                route_band.as_str(),
                &body,
                observed_unix_seconds,
            )
            .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    use http_body_util::BodyExt;
    use http_body_util::StreamBody;
    use hyper::body::Frame;

    #[derive(Clone, Debug, Default)]
    struct RecordingAsyncAffinityOwnerRecorder {
        records: Arc<Mutex<Vec<PreviousResponseAffinityOwnerRecord>>>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedHttpProviderError {
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        body: Vec<u8>,
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
            body: Vec<u8>,
            _observed_unix_seconds: u64,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                match self.records.lock() {
                    Ok(mut records) => records.push(RecordedHttpProviderError {
                        account_id,
                        route_band,
                        body,
                    }),
                    Err(error) => {
                        panic!("test provider observer lock should be available: {error}")
                    }
                }
            })
        }
    }

    impl RecordingAsyncAffinityOwnerRecorder {
        fn records(&self) -> Vec<PreviousResponseAffinityOwnerRecord> {
            match self.records.lock() {
                Ok(records) => records.clone(),
                Err(error) => panic!("test recorder lock should be available: {error}"),
            }
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
                body: usage_limit_body.to_vec(),
            }]
        );
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
                body: provider_error_json.to_vec(),
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
