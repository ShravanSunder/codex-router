//! Loopback-only server runtime primitives.

use std::collections::HashMap;
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
use std::sync::Arc;
use std::sync::Mutex;
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
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener as TokioTcpListener;
use tokio_util::sync::CancellationToken;

use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::affinity::hash_previous_response_id;
use codex_router_core::audit::AuditFileSink;
use codex_router_core::audit::RouteKind as AuditRouteKind;
use codex_router_core::audit::TransportKind;
use codex_router_core::ids::AccountId;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::local_auth::LocalRouterAuth;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::routes::RouteBand;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::affinity_secret::load_or_create_router_affinity_hash_secret;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::account::AccountStatus;
use codex_router_state::affinity_owner::AffinitySourceTransport;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::repositories::AffinityRepository;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;

use crate::account_selection::AsyncRepositoryBackedAccountSelector;
use crate::account_selection::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS;
use crate::account_selection::RouteBandAccountHolds;
use crate::account_selection::RouteBandWeightedSelectors;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::http_sse::AsyncHttpBodyError;
use crate::http_sse::AsyncProviderCredentialResolver;
use crate::http_sse::AsyncStreamingHttpProxyResponse;
use crate::http_sse::AsyncStreamingUpstreamHttpTransport;
use crate::http_sse::AuthenticatedHttpProxyService;
use crate::http_sse::HttpAffinityOwnerRecorder;
use crate::http_sse::HttpAffinitySecretProvider;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
#[cfg(test)]
use crate::http_sse::HttpProxyResponse;
#[cfg(test)]
use crate::http_sse::HttpRequestHandler;
use crate::http_sse::PreparedStreamingHttpProxyRequest;
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
use crate::routes::Method;
use crate::routes::RouteClass;
use crate::routes::classify_route;
use crate::secret_store_factory::ProxyRuntimeSecretStore;
use crate::secret_store_factory::open_proxy_secret_store;
use crate::upstream::HyperHttpUpstreamTransport;
use crate::upstream::UpstreamEndpoint;
use crate::websocket::AsyncWebSocketTunnel;
use crate::websocket::FirstFramePolicy;
use crate::websocket::WebSocketHandshakeRequest;
use crate::websocket::WebSocketProtocolRouter;
use crate::websocket::WebSocketRegistrySnapshot;
use crate::websocket::WebSocketRevocationRegistry;
use codex_router_auth::resolver::CredentialRefreshClient;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::OpenAiOAuthRefreshClient;
use codex_router_auth::resolver::ResolvedProviderCredential;

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
    max_websocket_upstream_messages: usize,
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
            max_websocket_upstream_messages: usize::MAX,
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
            max_websocket_upstream_messages: usize::MAX,
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

    /// Sets the bounded upstream-to-local WebSocket frame count.
    #[must_use]
    pub const fn with_max_websocket_upstream_messages(
        mut self,
        max_websocket_upstream_messages: usize,
    ) -> Self {
        self.max_websocket_upstream_messages = max_websocket_upstream_messages;
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
    secret_store: ProxyRuntimeSecretStore,
    affinity_owner_recorder: Arc<dyn HttpAffinityOwnerRecorder>,
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    upstream: HyperHttpUpstreamTransport,
    upstream_endpoint: UpstreamEndpoint,
    max_websocket_upstream_messages: usize,
    websocket_revocations: WebSocketRevocationRegistry,
    audit_sink: Option<AuditFileSink>,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    credential_refresh_leases: AsyncCredentialRefreshLeases,
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
        let secret_store = open_proxy_secret_store(&config.secret_store_root)?;
        let affinity_owner_recorder = Arc::new(SqliteAffinityOwnerRecorder::new(
            config.state_database_path.clone(),
        ));
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
            secret_store,
            affinity_owner_recorder,
            auth_gate,
            upstream,
            upstream_endpoint,
            max_websocket_upstream_messages: config.max_websocket_upstream_messages,
            websocket_revocations,
            audit_sink,
            weighted_selectors: Default::default(),
            account_holds: Default::default(),
            credential_refresh_leases: Default::default(),
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

    async fn serve_protocol_connections_async(
        &self,
        max_connections: usize,
        shutdown: Option<CancellationToken>,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let mut handled_connections = 0_usize;
        let mut handlers = Vec::new();
        let session_shutdown = shutdown.clone().unwrap_or_default();
        let connection_handler =
            Arc::new(self.protocol_connection_handler(session_shutdown.clone()));
        while handled_connections < max_connections {
            let (stream, _peer_addr) = if let Some(shutdown) = shutdown.as_ref() {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    accepted = self.server.listener.accept() => {
                        accepted.map_err(LoopbackRouterRuntimeError::Accept)?
                    }
                }
            } else {
                self.server
                    .listener
                    .accept()
                    .await
                    .map_err(LoopbackRouterRuntimeError::Accept)?
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
                handlers.push(handler);
            }
            handled_connections += 1;
        }

        if let Some(shutdown) = shutdown.as_ref()
            && shutdown.is_cancelled()
        {
            session_shutdown.cancel();
        }

        for handler in handlers {
            match handler.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error),
                Err(source) => return Err(LoopbackRouterRuntimeError::ConnectionJoin(source)),
            }
        }

        Ok(handled_connections)
    }

    fn protocol_connection_handler(
        &self,
        session_shutdown: CancellationToken,
    ) -> LoopbackProtocolConnectionHandler {
        LoopbackProtocolConnectionHandler {
            state_database_path: self.state_database_path.clone(),
            secret_store: self.secret_store.clone(),
            affinity_owner_recorder: Arc::clone(&self.affinity_owner_recorder),
            auth_gate: self.auth_gate.clone(),
            upstream: self.upstream.clone(),
            upstream_endpoint: self.upstream_endpoint.clone(),
            max_websocket_upstream_messages: self.max_websocket_upstream_messages,
            websocket_revocations: self.websocket_revocations.clone(),
            audit_sink: self.audit_sink.clone(),
            weighted_selectors: Arc::clone(&self.weighted_selectors),
            account_holds: Arc::clone(&self.account_holds),
            credential_refresh_leases: self.credential_refresh_leases.clone(),
            fixed_now_unix_seconds: self.fixed_now_unix_seconds,
            session_shutdown,
        }
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
    secret_store: ProxyRuntimeSecretStore,
    affinity_owner_recorder: Arc<dyn HttpAffinityOwnerRecorder>,
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    upstream: HyperHttpUpstreamTransport,
    upstream_endpoint: UpstreamEndpoint,
    max_websocket_upstream_messages: usize,
    websocket_revocations: WebSocketRevocationRegistry,
    audit_sink: Option<AuditFileSink>,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    credential_refresh_leases: AsyncCredentialRefreshLeases,
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
                        .handle_hyper_request(request, request_upgrade_tasks)
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
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        match HyperProtocolSwitchpoint::classify(request.method(), request.uri(), request.headers())
        {
            HyperProtocolDispatch::WebSocketUpgrade => {
                self.handle_hyper_websocket_request(request, upgrade_tasks)
                    .await
            }
            HyperProtocolDispatch::Http => self.handle_hyper_http_request(request).await,
        }
    }

    async fn handle_hyper_websocket_request(
        self: Arc<Self>,
        mut request: HttpRequest<Incoming>,
        upgrade_tasks: SharedUpgradeTasks,
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
                        .handle_hyper_websocket_upgraded(local_websocket, handshake, path)
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
    ) -> Result<(), LoopbackRouterRuntimeError> {
        let state_store = AsyncSqliteStateStore::open(&self.state_database_path).await?;
        let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
            &state_store,
            Arc::clone(&self.weighted_selectors),
            Arc::clone(&self.account_holds),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            self.runtime_clock(),
        );
        let credential_resolver = AsyncProxyCredentialResolver::new(
            state_store.clone(),
            self.secret_store.clone(),
            self.credential_refresh_leases.clone(),
            self.fixed_now_unix_seconds,
        );
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
        .with_affinity_secret_provider(&self.secret_store)
        .with_affinity_owner_recorder(Arc::clone(&self.affinity_owner_recorder));
        let upstream_url = self.upstream_endpoint.websocket_url_for_path(&path);
        tunnel
            .handle_upgraded_connection(
                local_websocket,
                handshake,
                upstream_url.as_str(),
                self.max_websocket_upstream_messages,
            )
            .await
            .map_err(LoopbackRouterRuntimeError::WebSocket)
    }

    async fn handle_hyper_http_request(
        self: Arc<Self>,
        request: HttpRequest<Incoming>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        let request = match hyper_request_to_proxy_request(request).await {
            Ok(request) => request,
            Err(_error) => return empty_response(StatusCode::BAD_REQUEST),
        };
        let prepared = match self.prepare_streaming_http_request_async(request).await {
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

    async fn prepare_streaming_http_request_async(
        &self,
        request: HttpProxyRequest,
    ) -> Result<PreparedStreamingHttpProxyRequest, HttpProxyError> {
        let state_store = AsyncSqliteStateStore::open(&self.state_database_path)
            .await
            .map_err(|_error| HttpProxyError::Selection {
                reason: crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
            })?;
        let credential_resolver = AsyncProxyCredentialResolver::new(
            state_store.clone(),
            self.secret_store.clone(),
            self.credential_refresh_leases.clone(),
            self.fixed_now_unix_seconds,
        );
        let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
            &state_store,
            Arc::clone(&self.weighted_selectors),
            Arc::clone(&self.account_holds),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            self.runtime_clock(),
        );
        let service = AuthenticatedHttpProxyService::new(
            &self.auth_gate,
            &selector,
            &credential_resolver,
            &self.upstream,
        )
        .with_affinity_secret_provider(&self.secret_store)
        .with_affinity_owner_recorder(Arc::clone(&self.affinity_owner_recorder));
        let service = if let Some(audit_sink) = &self.audit_sink {
            service.with_audit_sink(audit_sink)
        } else {
            service
        };
        service.prepare_streaming_request_async(request).await
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

async fn hyper_request_to_proxy_request(
    request: HttpRequest<Incoming>,
) -> Result<HttpProxyRequest, LoopbackRouterRuntimeError> {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str)
        .to_owned();
    let body = body
        .collect()
        .await
        .map_err(LoopbackRouterRuntimeError::HyperBody)?
        .to_bytes()
        .to_vec();
    let mut proxy_request = HttpProxyRequest::new(method_from_hyper(&parts.method), path);
    for (name, value) in &parts.headers {
        if let Ok(value) = value.to_str() {
            proxy_request = proxy_request.with_header(Header::new(name.as_str(), value));
        }
    }

    Ok(proxy_request.with_body(body))
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
    affinity_owner_recorder: Arc<dyn HttpAffinityOwnerRecorder>,
) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    let body = record_affinity_owner_from_async_body(body, completion, affinity_owner_recorder);
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
    affinity_owner_recorder: Arc<dyn HttpAffinityOwnerRecorder>,
) -> BoxBody<Bytes, AsyncHttpBodyError> {
    let Some(affinity_secret) = completion.affinity_secret().cloned() else {
        return body;
    };
    let account_id = completion.account_id().clone();
    let credential_generation = completion.credential_generation();
    let mut buffered = Vec::new();
    let mut recorded = false;

    body.map_frame(move |frame| {
        if !recorded && let Some(data) = frame.data_ref() {
            buffered.extend_from_slice(data);
            if let Ok(Some(response_id)) = extract_response_id_from_body(&buffered) {
                recorded = true;
                let recorder = Arc::clone(&affinity_owner_recorder);
                let account_id = account_id.clone();
                let affinity_secret = affinity_secret.clone();
                tokio::task::spawn_blocking(move || {
                    let Ok(affinity_key_hash) =
                        hash_previous_response_id(&affinity_secret, &response_id)
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
                    let _record_result = recorder.record_affinity_owner(&owner);
                });
            }
        }

        frame
    })
    .boxed()
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

#[derive(Clone, Debug, Default)]
struct AsyncCredentialRefreshLeases {
    leases: Arc<Mutex<HashMap<AccountId, Arc<tokio::sync::Mutex<()>>>>>,
}

impl AsyncCredentialRefreshLeases {
    fn lease_for(&self, account_id: &AccountId) -> Arc<tokio::sync::Mutex<()>> {
        let mut leases = self
            .leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            leases
                .entry(account_id.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }
}

#[derive(Clone, Debug)]
struct AsyncProxyCredentialResolver {
    state_store: AsyncSqliteStateStore,
    secret_store: ProxyRuntimeSecretStore,
    refresh_leases: AsyncCredentialRefreshLeases,
    refresh_client: OpenAiOAuthRefreshClient,
    fixed_now_unix_seconds: Option<u64>,
}

impl AsyncProxyCredentialResolver {
    fn new(
        state_store: AsyncSqliteStateStore,
        secret_store: ProxyRuntimeSecretStore,
        refresh_leases: AsyncCredentialRefreshLeases,
        fixed_now_unix_seconds: Option<u64>,
    ) -> Self {
        Self {
            state_store,
            secret_store,
            refresh_leases,
            refresh_client: OpenAiOAuthRefreshClient::new(),
            fixed_now_unix_seconds,
        }
    }

    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        let now_unix_seconds = match self.fixed_now_unix_seconds {
            Some(now_unix_seconds) => now_unix_seconds,
            None => current_unix_seconds()
                .map_err(|_error| CredentialResolverError::RefreshUnavailable)?,
        };
        let (active_generation, bundle) = self.read_active_bundle(account_id).await?;
        if self.bundle_is_expired(&bundle, now_unix_seconds) {
            let lease = self.refresh_leases.lease_for(account_id);
            let _guard = lease.lock().await;
            let (current_generation, current_bundle) = self.read_active_bundle(account_id).await?;
            let (resolved_generation, refreshed) =
                if self.bundle_is_expired(&current_bundle, now_unix_seconds) {
                    self.refresh_expired_bundle(account_id, &current_bundle)
                        .await?
                } else {
                    (current_generation, current_bundle)
                };
            return Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                refreshed.access_token().clone(),
                resolved_generation,
            )
            .with_chatgpt_account_id(refreshed.chatgpt_account_id()));
        }

        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            bundle.access_token().clone(),
            active_generation,
        )
        .with_chatgpt_account_id(bundle.chatgpt_account_id()))
    }

    async fn read_active_bundle(
        &self,
        account_id: &AccountId,
    ) -> Result<(u64, AccountCredentialBundle), CredentialResolverError> {
        let account = self
            .state_store
            .load_account(account_id)
            .await
            .map_err(map_state_error)?
            .ok_or(CredentialResolverError::AccountUnavailable)?;
        if account.status() != AccountStatus::Enabled {
            return Err(CredentialResolverError::AccountIneligible);
        }
        let active_generation = account
            .active_credential_generation()
            .ok_or(CredentialResolverError::AccountIneligible)?;
        let bundle_key = account_credential_bundle_key(account_id, active_generation)
            .map_err(map_secret_error)?;
        let secret_store = self.secret_store.clone();
        let bundle = tokio::task::spawn_blocking(move || {
            let secret = secret_store
                .read_secret(&bundle_key)
                .map_err(map_secret_error)?;
            AccountCredentialBundle::from_secret_string(secret).map_err(map_secret_error)
        })
        .await
        .map_err(|_error| CredentialResolverError::SecretUnavailable)??;

        Ok((active_generation, bundle))
    }

    fn bundle_is_expired(&self, bundle: &AccountCredentialBundle, now_unix_seconds: u64) -> bool {
        bundle
            .expires_unix_seconds()
            .is_some_and(|expires| expires <= now_unix_seconds)
    }

    async fn refresh_expired_bundle(
        &self,
        account_id: &AccountId,
        bundle: &AccountCredentialBundle,
    ) -> Result<(u64, AccountCredentialBundle), CredentialResolverError> {
        let refresh_token = bundle
            .refresh_token()
            .ok_or(CredentialResolverError::RefreshUnavailable)?
            .clone();
        let refresh_client = self.refresh_client.clone();
        let account_id_for_refresh = account_id.clone();
        let mut refreshed = tokio::task::spawn_blocking(move || {
            refresh_client.refresh_credentials(&account_id_for_refresh, &refresh_token)
        })
        .await
        .map_err(|_error| CredentialResolverError::RefreshUnavailable)??;
        if refreshed.chatgpt_account_id().is_none()
            && let Some(chatgpt_account_id) = bundle.chatgpt_account_id()
        {
            refreshed = refreshed.with_chatgpt_account_id(chatgpt_account_id);
        }
        let refreshed_generation = self
            .state_store
            .next_credential_generation(account_id)
            .await
            .map_err(map_state_error)?;
        let refreshed_key = account_credential_bundle_key(account_id, refreshed_generation)
            .map_err(map_secret_error)?;
        let refreshed_secret = refreshed.to_secret_string().map_err(map_secret_error)?;
        let secret_store = self.secret_store.clone();
        tokio::task::spawn_blocking(move || {
            secret_store
                .write_secret(&refreshed_key, &refreshed_secret)
                .map_err(map_secret_error)
        })
        .await
        .map_err(|_error| CredentialResolverError::SecretUnavailable)??;
        self.state_store
            .activate_account_credential_generation_and_invalidate_quota(
                account_id,
                refreshed_generation,
                AccountStatus::Enabled,
            )
            .await
            .map_err(map_state_error)?;

        Ok((refreshed_generation, refreshed))
    }
}

impl AsyncProviderCredentialResolver for AsyncProxyCredentialResolver {
    fn resolve_provider_credentials<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        Box::pin(async move { self.resolve_provider_credentials_async(account_id).await })
    }
}

fn map_state_error(_error: StateStoreError) -> CredentialResolverError {
    CredentialResolverError::AccountUnavailable
}

fn map_secret_error(_error: SecretStoreError) -> CredentialResolverError {
    CredentialResolverError::SecretUnavailable
}

impl HttpAffinitySecretProvider for ProxyRuntimeSecretStore {
    fn load_or_create_affinity_secret(&self) -> Result<RouterAffinityHashSecret, HttpProxyError> {
        load_or_create_router_affinity_hash_secret(self)
            .map(|loaded| loaded.secret().clone())
            .map_err(|_error| HttpProxyError::Selection {
                reason: crate::account_selection::QuotaAwareAccountSelectorError::SecretUnavailable,
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SqliteAffinityOwnerRecorder {
    state_database_path: PathBuf,
}

impl SqliteAffinityOwnerRecorder {
    fn new(state_database_path: PathBuf) -> Self {
        Self {
            state_database_path,
        }
    }
}

impl HttpAffinityOwnerRecorder for SqliteAffinityOwnerRecorder {
    fn record_affinity_owner(
        &self,
        owner: &PreviousResponseAffinityOwnerRecord,
    ) -> Result<(), HttpProxyError> {
        let state_store = SqliteStateStore::open(&self.state_database_path).map_err(|_error| {
            HttpProxyError::Selection {
                reason: crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
            }
        })?;
        AffinityRepository::write_previous_response_owner(&state_store, owner).map_err(|_error| {
            HttpProxyError::Selection {
                reason: crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
            }
        })
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
    /// Opening or reading the router secret store failed.
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
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
