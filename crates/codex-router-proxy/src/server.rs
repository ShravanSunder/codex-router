//! Loopback-only server runtime primitives.

use std::io::Read;
use std::io::Write;
use std::net::AddrParseError;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::audit::AuditFileSink;
use codex_router_core::audit::RouteKind as AuditRouteKind;
use codex_router_core::audit::TransportKind;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::local_auth::LocalRouterAuth;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_secret_store::affinity_secret::load_or_create_router_affinity_hash_secret;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::repositories::AffinityRepository;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;

use crate::account_selection::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS;
use crate::account_selection::RepositoryBackedAccountSelector;
use crate::account_selection::RouteBandAccountHolds;
use crate::account_selection::RouteBandWeightedSelectors;
use crate::credential_runtime::ProxyCredentialResolver;
use crate::credential_runtime::ProxyCredentialResolverOpenError;
use crate::headers::Header;
use crate::http_sse::AuthenticatedHttpProxyService;
use crate::http_sse::HttpAffinityOwnerRecorder;
use crate::http_sse::HttpAffinitySecretProvider;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
use crate::http_sse::HttpProxyResponse;
use crate::http_sse::HttpRequestHandler;
use crate::http_sse::StderrAuditFailureReporter;
use crate::http_sse::StreamingHttpProxyResponse;
use crate::http_sse::StreamingHttpRequestHandler;
use crate::http_sse::append_audit_event_with_reporter;
use crate::http_sse::local_auth_rejection_audit_event;
use crate::local_auth::extract_presented_local_token_from_request;
use crate::routes::Method;
use crate::routes::RouteClass;
use crate::routes::classify_route;
use crate::secret_store_factory::ProxyRuntimeSecretStore;
use crate::secret_store_factory::open_proxy_secret_store;
use crate::upstream::HttpUpstreamTransport;
use crate::upstream::UpstreamEndpoint;
use crate::websocket::BlockingWebSocketTunnel;
use crate::websocket::FirstFramePolicy;
use crate::websocket::WebSocketProtocolRouter;
use crate::websocket::WebSocketRevocationRegistry;

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
#[derive(Debug)]
pub struct LoopbackServerRuntime {
    listener: TcpListener,
    local_addr: SocketAddr,
}

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
}

/// Assembled loopback router runtime for HTTP/SSE forwarding.
pub struct LoopbackRouterRuntime {
    server: LoopbackServerRuntime,
    state_database_path: PathBuf,
    secret_store_root: PathBuf,
    state_store: SqliteStateStore,
    secret_store: ProxyRuntimeSecretStore,
    affinity_owner_recorder: Arc<dyn HttpAffinityOwnerRecorder>,
    credential_resolver: ProxyCredentialResolver,
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    upstream: HttpUpstreamTransport,
    upstream_endpoint: UpstreamEndpoint,
    max_websocket_upstream_messages: usize,
    websocket_revocations: WebSocketRevocationRegistry,
    audit_sink: Option<AuditFileSink>,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    fixed_now_unix_seconds: Option<u64>,
}

impl LoopbackRouterRuntime {
    /// Opens router-owned state/secrets and binds the loopback listener.
    pub fn start(config: LoopbackRouterRuntimeConfig) -> Result<Self, LoopbackRouterRuntimeError> {
        let state_store = SqliteStateStore::open(&config.state_database_path)?;
        let secret_store = open_proxy_secret_store(&config.secret_store_root)?;
        let affinity_owner_recorder = Arc::new(SqliteAffinityOwnerRecorder::new(
            config.state_database_path.clone(),
        ));
        let runtime_start_unix_seconds = match config.fixed_now_unix_seconds {
            Some(now_unix_seconds) => now_unix_seconds,
            None => current_unix_seconds().map_err(LoopbackRouterRuntimeError::SystemClock)?,
        };
        let credential_resolver = ProxyCredentialResolver::open(
            &config.state_database_path,
            &config.secret_store_root,
            runtime_start_unix_seconds,
        )?;
        let auth_gate = match config.local_token {
            Some(local_token) => crate::local_auth::ProxyLocalAuthGate::new(LocalRouterAuth::new(
                local_token,
                Vec::new(),
            )),
            None => crate::local_auth::ProxyLocalAuthGate::disabled(),
        };
        let upstream_endpoint = config.upstream_endpoint;
        let upstream = HttpUpstreamTransport::new(upstream_endpoint.clone());
        let server = LoopbackServerRuntime::bind(config.bind_address)?;
        let audit_sink = config.audit_file_path.map(AuditFileSink::new);

        Ok(Self {
            server,
            state_database_path: config.state_database_path,
            secret_store_root: config.secret_store_root,
            state_store,
            secret_store,
            affinity_owner_recorder,
            credential_resolver,
            auth_gate,
            upstream,
            upstream_endpoint,
            max_websocket_upstream_messages: config.max_websocket_upstream_messages,
            websocket_revocations: WebSocketRevocationRegistry::new(),
            audit_sink,
            weighted_selectors: Default::default(),
            account_holds: Default::default(),
            fixed_now_unix_seconds: config.fixed_now_unix_seconds,
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
    pub fn serve_http_connections(
        &self,
        max_connections: usize,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let listener = self
            .server
            .listener()
            .try_clone()
            .map_err(LoopbackRouterRuntimeError::ListenerClone)?;
        let selector = self.repository_backed_account_selector();
        let service = AuthenticatedHttpProxyService::new(
            &self.auth_gate,
            &selector,
            &self.credential_resolver,
            &self.upstream,
        )
        .with_affinity_secret_provider(&self.secret_store)
        .with_affinity_owner_recorder(Arc::clone(&self.affinity_owner_recorder));
        let service = if let Some(audit_sink) = &self.audit_sink {
            service.with_audit_sink(audit_sink)
        } else {
            service
        };

        LoopbackHttpServer::serve_streaming_connections(listener, &service, max_connections)
            .map_err(LoopbackRouterRuntimeError::Connection)
    }

    /// Serves a bounded number of HTTP/SSE or WebSocket connections.
    pub fn serve_protocol_connections(
        &self,
        max_connections: usize,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        let listener = self
            .server
            .listener()
            .try_clone()
            .map_err(LoopbackRouterRuntimeError::ListenerClone)?;
        let mut handled_connections = 0_usize;
        let mut handlers = Vec::new();
        let connection_handler = self.protocol_connection_handler();
        while handled_connections < max_connections {
            let (stream, _peer_addr) = listener
                .accept()
                .map_err(LoopbackRouterRuntimeError::Accept)?;
            let handler_context = connection_handler.clone();
            let handler =
                std::thread::spawn(move || handler_context.handle_protocol_connection(stream));
            if max_connections == usize::MAX {
                drop(handler);
            } else {
                handlers.push(handler);
            }
            handled_connections += 1;
        }

        for handler in handlers {
            match handler.join() {
                Ok(_result) => {}
                Err(_panic) => return Err(LoopbackRouterRuntimeError::ConnectionPanic),
            }
        }

        Ok(handled_connections)
    }

    fn protocol_connection_handler(&self) -> LoopbackProtocolConnectionHandler {
        LoopbackProtocolConnectionHandler {
            state_database_path: self.state_database_path.clone(),
            secret_store_root: self.secret_store_root.clone(),
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
            fixed_now_unix_seconds: self.fixed_now_unix_seconds,
        }
    }

    fn repository_backed_account_selector(
        &self,
    ) -> RepositoryBackedAccountSelector<'_, SqliteStateStore> {
        let fixed_now_unix_seconds = self.fixed_now_unix_seconds;
        RepositoryBackedAccountSelector::new_with_runtime(
            &self.state_store,
            Arc::clone(&self.weighted_selectors),
            Arc::clone(&self.account_holds),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            Arc::new(move || {
                fixed_now_unix_seconds.unwrap_or_else(|| match current_unix_seconds() {
                    Ok(now_unix_seconds) => now_unix_seconds,
                    Err(error) => {
                        panic!("system clock must remain after Unix epoch for routing: {error}")
                    }
                })
            }),
        )
    }
}

#[derive(Clone)]
struct LoopbackProtocolConnectionHandler {
    state_database_path: PathBuf,
    secret_store_root: PathBuf,
    secret_store: ProxyRuntimeSecretStore,
    affinity_owner_recorder: Arc<dyn HttpAffinityOwnerRecorder>,
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    upstream: HttpUpstreamTransport,
    upstream_endpoint: UpstreamEndpoint,
    max_websocket_upstream_messages: usize,
    websocket_revocations: WebSocketRevocationRegistry,
    audit_sink: Option<AuditFileSink>,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    fixed_now_unix_seconds: Option<u64>,
}

impl LoopbackProtocolConnectionHandler {
    fn handle_protocol_connection(
        &self,
        stream: TcpStream,
    ) -> Result<(), LoopbackRouterRuntimeError> {
        if let Some(preflight) = websocket_handshake_preflight(&stream)? {
            let presented_token = match preflight.presented_token.as_ref().map(Option::as_deref) {
                Ok(presented_token) => presented_token,
                Err(reason) => {
                    if let Some(audit_sink) = &self.audit_sink {
                        let event = local_auth_rejection_audit_event(
                            TransportKind::WebSocket,
                            AuditRouteKind::ResponsesWebSocket,
                            *reason,
                        );
                        append_audit_event_with_reporter(
                            audit_sink,
                            &event,
                            &StderrAuditFailureReporter,
                        );
                    }
                    write_websocket_rejection(stream, 401, "Unauthorized")?;
                    return Ok(());
                }
            };
            match self.auth_gate.authorize(presented_token) {
                Ok(_generation) => {}
                Err(reason) => {
                    if let Some(audit_sink) = &self.audit_sink {
                        let event = local_auth_rejection_audit_event(
                            TransportKind::WebSocket,
                            AuditRouteKind::ResponsesWebSocket,
                            reason,
                        );
                        append_audit_event_with_reporter(
                            audit_sink,
                            &event,
                            &StderrAuditFailureReporter,
                        );
                    }
                    write_websocket_rejection(stream, 401, "Unauthorized")?;
                    return Ok(());
                }
            }
            match classify_route(Method::Post, path_without_query(&preflight.path), true) {
                RouteClass::Supported(_) => {}
                RouteClass::Rejected { .. } => {
                    write_websocket_rejection(stream, 404, "Not Found")?;
                    return Ok(());
                }
            }

            let state_store = SqliteStateStore::open(&self.state_database_path)?;
            let credential_resolver = self.open_credential_resolver()?;
            let selector = self.repository_backed_account_selector(&state_store);
            let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024 * 1024));
            let tunnel = if let Some(audit_sink) = &self.audit_sink {
                BlockingWebSocketTunnel::new_with_revocation_registry_and_audit_sink(
                    &self.auth_gate,
                    &selector,
                    &credential_resolver,
                    &protocol_router,
                    self.websocket_revocations.clone(),
                    audit_sink,
                )
                .with_affinity_secret_provider(&self.secret_store)
                .with_affinity_owner_recorder(self.affinity_owner_recorder.as_ref())
            } else {
                BlockingWebSocketTunnel::new_with_revocation_registry(
                    &self.auth_gate,
                    &selector,
                    &credential_resolver,
                    &protocol_router,
                    self.websocket_revocations.clone(),
                )
                .with_affinity_secret_provider(&self.secret_store)
                .with_affinity_owner_recorder(self.affinity_owner_recorder.as_ref())
            };
            let upstream_url = self
                .upstream_endpoint
                .websocket_url_for_path(&preflight.path);
            tunnel
                .handle_connection(
                    stream,
                    upstream_url.as_str(),
                    self.max_websocket_upstream_messages,
                )
                .map_err(LoopbackRouterRuntimeError::WebSocket)?;

            return Ok(());
        }

        let state_store = SqliteStateStore::open(&self.state_database_path)?;
        let credential_resolver = self.open_credential_resolver()?;
        let selector = self.repository_backed_account_selector(&state_store);
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
        LoopbackHttpAdapter::handle_streaming_connection(stream, &service)
            .map_err(LoopbackRouterRuntimeError::Connection)
    }

    fn repository_backed_account_selector<'a>(
        &self,
        state_store: &'a SqliteStateStore,
    ) -> RepositoryBackedAccountSelector<'a, SqliteStateStore> {
        let fixed_now_unix_seconds = self.fixed_now_unix_seconds;
        RepositoryBackedAccountSelector::new_with_runtime(
            state_store,
            Arc::clone(&self.weighted_selectors),
            Arc::clone(&self.account_holds),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            Arc::new(move || {
                fixed_now_unix_seconds.unwrap_or_else(|| match current_unix_seconds() {
                    Ok(now_unix_seconds) => now_unix_seconds,
                    Err(error) => {
                        panic!("system clock must remain after Unix epoch for routing: {error}")
                    }
                })
            }),
        )
    }

    fn open_credential_resolver(
        &self,
    ) -> Result<ProxyCredentialResolver, LoopbackRouterRuntimeError> {
        let now_unix_seconds = match self.fixed_now_unix_seconds {
            Some(now_unix_seconds) => now_unix_seconds,
            None => current_unix_seconds().map_err(LoopbackRouterRuntimeError::SystemClock)?,
        };
        ProxyCredentialResolver::open(
            &self.state_database_path,
            &self.secret_store_root,
            now_unix_seconds,
        )
        .map_err(LoopbackRouterRuntimeError::CredentialResolver)
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

#[derive(Debug)]
struct WebSocketHandshakePreflight {
    path: String,
    presented_token: Result<Option<String>, LocalAuthError>,
}

fn websocket_handshake_preflight(
    stream: &TcpStream,
) -> Result<Option<WebSocketHandshakePreflight>, LoopbackRouterRuntimeError> {
    let previous_timeout = stream
        .read_timeout()
        .map_err(LoopbackRouterRuntimeError::Peek)?;
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(LoopbackRouterRuntimeError::Peek)?;
    let started = Instant::now();
    let mut buffer = vec![0_u8; MAX_HTTP_HEADER_BYTES];
    let read = loop {
        let read = match stream.peek(&mut buffer) {
            Ok(read) => read,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                stream
                    .set_read_timeout(previous_timeout)
                    .map_err(LoopbackRouterRuntimeError::Peek)?;
                return Ok(None);
            }
            Err(error) => {
                stream
                    .set_read_timeout(previous_timeout)
                    .map_err(LoopbackRouterRuntimeError::Peek)?;
                return Err(LoopbackRouterRuntimeError::Peek(error));
            }
        };
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut parsed_request = httparse::Request::new(&mut headers);
        match parsed_request.parse(&buffer[..read]) {
            Ok(httparse::Status::Complete(_header_length)) => break read,
            Ok(httparse::Status::Partial) if started.elapsed() < Duration::from_millis(250) => {
                std::thread::yield_now();
            }
            Ok(httparse::Status::Partial) => {
                stream
                    .set_read_timeout(previous_timeout)
                    .map_err(LoopbackRouterRuntimeError::Peek)?;
                return Ok(None);
            }
            Err(source) => {
                stream
                    .set_read_timeout(previous_timeout)
                    .map_err(LoopbackRouterRuntimeError::Peek)?;
                return Err(LoopbackRouterRuntimeError::PreflightParse(source));
            }
        }
    };
    stream
        .set_read_timeout(previous_timeout)
        .map_err(LoopbackRouterRuntimeError::Peek)?;
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed_request = httparse::Request::new(&mut headers);
    let header_length = match parsed_request.parse(&buffer[..read]) {
        Ok(httparse::Status::Complete(header_length)) => header_length,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(source) => return Err(LoopbackRouterRuntimeError::PreflightParse(source)),
    };
    let head = String::from_utf8_lossy(&buffer[..header_length]).to_ascii_lowercase();
    if !head.contains("upgrade: websocket") {
        return Ok(None);
    }
    let path = parsed_request
        .path
        .ok_or(LoopbackRouterRuntimeError::PreflightMissingPath)?
        .to_owned();
    let router_token = parsed_request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("x-codex-router-token"))
        .and_then(|header| std::str::from_utf8(header.value).ok())
        .map(str::to_owned);
    let authorization = parsed_request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("authorization"))
        .and_then(|header| std::str::from_utf8(header.value).ok())
        .map(str::to_owned);
    let cookie = parsed_request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("cookie"))
        .and_then(|header| std::str::from_utf8(header.value).ok())
        .map(str::to_owned);
    let subprotocol = parsed_request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("sec-websocket-protocol"))
        .and_then(|header| std::str::from_utf8(header.value).ok())
        .map(str::to_owned);
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
            &path,
            &[],
            false,
        )
        .map(|token| token.map(str::to_owned))
    };

    Ok(Some(WebSocketHandshakePreflight {
        path,
        presented_token,
    }))
}

fn has_forbidden_websocket_subprotocol_auth_carrier(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("token") || value.contains("bearer") || value.contains("authorization")
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?').map_or(path, |(path, _query)| path)
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

fn write_websocket_rejection(
    mut stream: TcpStream,
    status: u16,
    reason: &str,
) -> Result<(), LoopbackRouterRuntimeError> {
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .map_err(LoopbackRouterRuntimeError::RejectWrite)
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
    /// Peeking the accepted connection failed.
    #[error("failed peeking loopback router connection")]
    Peek(#[source] std::io::Error),
    /// WebSocket preflight HTTP parse failed.
    #[error("failed parsing websocket preflight request")]
    PreflightParse(#[source] httparse::Error),
    /// WebSocket preflight request had no path.
    #[error("websocket preflight request path was missing")]
    PreflightMissingPath,
    /// WebSocket preflight rejection could not be written.
    #[error("failed writing websocket preflight rejection")]
    RejectWrite(#[source] std::io::Error),
    /// Opening or reading SQLite state failed.
    #[error(transparent)]
    State(#[from] StateStoreError),
    /// Opening or reading the router secret store failed.
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    /// Opening runtime credential state failed.
    #[error(transparent)]
    CredentialResolver(#[from] ProxyCredentialResolverOpenError),
    /// Runtime system clock is before Unix epoch.
    #[error("system clock is before Unix epoch")]
    SystemClock(#[source] std::time::SystemTimeError),
    /// Listener cloning failed before the bounded serve loop.
    #[error("failed to clone loopback listener")]
    ListenerClone(#[source] std::io::Error),
    /// A loopback connection handler panicked.
    #[error("loopback router connection handler panicked")]
    ConnectionPanic,
    /// Serving a loopback connection failed.
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
#[derive(Clone, Copy, Debug)]
pub struct LoopbackHttpAdapter;

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
#[derive(Clone, Copy, Debug)]
pub struct LoopbackHttpServer;

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
struct ParsedHttpRequestHead {
    method: Method,
    path: String,
    headers: Vec<Header>,
    header_length: usize,
    content_length: usize,
}

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

fn method_from_http(method: &str) -> Method {
    match method {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => Method::Other,
    }
}

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
