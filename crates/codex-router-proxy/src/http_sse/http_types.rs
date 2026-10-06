use super::*;

/// Client HTTP request DTO used by server adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpProxyRequest {
    pub(super) method: Method,
    pub(super) path: String,
    pub(super) websocket_upgrade: bool,
    pub(super) headers: Vec<Header>,
    pub(super) body: Vec<u8>,
    pub(super) excluded_accounts: Vec<AccountId>,
}

impl HttpProxyRequest {
    /// Creates an HTTP proxy request.
    #[must_use]
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            websocket_upgrade: false,
            headers: Vec::new(),
            body: Vec::new(),
            excluded_accounts: Vec::new(),
        }
    }

    /// Marks the request as a WebSocket upgrade.
    #[must_use]
    pub const fn with_websocket_upgrade(mut self, websocket_upgrade: bool) -> Self {
        self.websocket_upgrade = websocket_upgrade;
        self
    }

    /// Adds a header.
    #[must_use]
    pub fn with_header(mut self, header: Header) -> Self {
        self.headers.push(header);
        self
    }

    /// Sets the body bytes.
    #[must_use]
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    /// Excludes one account from request-local retry selection.
    #[must_use]
    pub fn with_excluded_account(mut self, account_id: AccountId) -> Self {
        if !self
            .excluded_accounts
            .iter()
            .any(|excluded_account_id| excluded_account_id == &account_id)
        {
            self.excluded_accounts.push(account_id);
        }
        self
    }

    /// Returns request path and query string.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns request method.
    #[must_use]
    pub const fn method(&self) -> Method {
        self.method
    }

    /// Returns whether this request is a WebSocket upgrade.
    #[must_use]
    pub const fn websocket_upgrade(&self) -> bool {
        self.websocket_upgrade
    }

    /// Returns body bytes.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Returns accounts excluded by request-local retry state.
    #[must_use]
    pub fn excluded_accounts(&self) -> &[AccountId] {
        &self.excluded_accounts
    }

    /// Returns client headers in their original order, including duplicate names.
    #[must_use]
    pub(crate) fn headers(&self) -> &[Header] {
        &self.headers
    }

    /// Returns first header value by normalized name.
    #[must_use]
    pub fn header_value(&self, name: &str) -> Option<&str> {
        let normalized = name.to_ascii_lowercase();
        self.headers()
            .iter()
            .find(|header| header.name() == normalized)
            .map(Header::value)
    }
}

/// Upstream HTTP request after proxy sanitization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamHttpRequest {
    pub(super) method: Method,
    pub(super) path: String,
    pub(super) route_kind: RouteKind,
    pub(super) headers: HeaderCollection,
    pub(super) body: Vec<u8>,
}

impl UpstreamHttpRequest {
    /// Creates a request after the provider edge has sanitized its headers.
    #[must_use]
    pub(crate) const fn new(
        method: Method,
        path: String,
        route_kind: RouteKind,
        headers: HeaderCollection,
        body: Vec<u8>,
    ) -> Self {
        Self {
            method,
            path,
            route_kind,
            headers,
            body,
        }
    }

    /// Creates a sanitized upstream request for transport-boundary tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn new_for_test(
        method: Method,
        path: String,
        route_kind: RouteKind,
        headers: HeaderCollection,
        body: Vec<u8>,
    ) -> Self {
        Self::new(method, path, route_kind, headers, body)
    }

    /// Returns request method.
    #[must_use]
    pub const fn method(&self) -> Method {
        self.method
    }

    /// Returns request path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns route kind.
    #[must_use]
    pub const fn route_kind(&self) -> RouteKind {
        self.route_kind
    }

    /// Returns headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderCollection {
        &self.headers
    }

    /// Returns body bytes.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Combines this sanitized request head with a Hyper-owned streaming body.
    #[must_use]
    pub fn into_streaming_body(
        self,
        body: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> StreamingUpstreamHttpRequest {
        StreamingUpstreamHttpRequest::new(
            self.method,
            self.path,
            self.route_kind,
            self.headers,
            body,
        )
    }
}

/// Upstream HTTP request whose body remains an async Hyper stream.
pub struct StreamingUpstreamHttpRequest {
    pub(super) method: Method,
    pub(super) path: String,
    pub(super) route_kind: RouteKind,
    pub(super) headers: HeaderCollection,
    pub(super) body: BoxBody<Bytes, AsyncHttpBodyError>,
}

impl StreamingUpstreamHttpRequest {
    /// Creates a streaming request after the provider edge has sanitized its headers.
    #[must_use]
    pub(crate) const fn new(
        method: Method,
        path: String,
        route_kind: RouteKind,
        headers: HeaderCollection,
        body: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> Self {
        Self {
            method,
            path,
            route_kind,
            headers,
            body,
        }
    }

    /// Creates a sanitized streaming upstream request for transport-boundary tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn new_for_test(
        method: Method,
        path: String,
        route_kind: RouteKind,
        headers: HeaderCollection,
        body: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> Self {
        Self::new(method, path, route_kind, headers, body)
    }

    /// Returns request method.
    #[must_use]
    pub const fn method(&self) -> Method {
        self.method
    }

    /// Returns request path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns route kind.
    #[must_use]
    pub const fn route_kind(&self) -> RouteKind {
        self.route_kind
    }

    /// Returns headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderCollection {
        &self.headers
    }

    /// Consumes the request body stream.
    #[must_use]
    pub fn into_body(self) -> BoxBody<Bytes, AsyncHttpBodyError> {
        self.body
    }
}

/// HTTP proxy response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpProxyResponse {
    pub(super) status: u16,
    pub(super) headers: HeaderCollection,
    pub(super) body: Vec<u8>,
}

impl HttpProxyResponse {
    /// Creates a proxy response.
    #[must_use]
    pub const fn new(status: u16, headers: HeaderCollection, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// Returns status.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Returns headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderCollection {
        &self.headers
    }

    /// Returns body bytes.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// HTTP proxy response whose body can be streamed to the local client.
pub struct StreamingHttpProxyResponse {
    pub(super) status: u16,
    pub(super) headers: HeaderCollection,
    pub(super) body: Box<dyn Read + Send>,
}

impl StreamingHttpProxyResponse {
    /// Creates a streaming proxy response.
    #[must_use]
    pub fn new(status: u16, headers: HeaderCollection, body: Box<dyn Read + Send>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// Creates a streaming response from already-buffered bytes.
    #[must_use]
    pub fn from_buffered(response: HttpProxyResponse) -> Self {
        Self::new(
            response.status,
            response.headers,
            Box::new(Cursor::new(response.body)),
        )
    }

    /// Returns status.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Returns headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderCollection {
        &self.headers
    }

    /// Returns a mutable response body reader.
    pub fn body_mut(&mut self) -> &mut dyn Read {
        self.body.as_mut()
    }

    /// Buffers a streaming response for compatibility tests.
    pub fn into_buffered(mut self) -> Result<HttpProxyResponse, HttpProxyError> {
        let mut body = Vec::new();
        self.body
            .read_to_end(&mut body)
            .map_err(|error| HttpProxyError::Upstream {
                message: error.to_string(),
            })?;

        Ok(HttpProxyResponse::new(self.status, self.headers, body))
    }
}

/// Error type used by async HTTP response bodies.
pub type AsyncHttpBodyError = Box<dyn StdError + Send + Sync>;

/// HTTP proxy response whose body is owned by Hyper async streaming.
pub struct AsyncStreamingHttpProxyResponse {
    pub(super) status: u16,
    pub(super) headers: HeaderCollection,
    pub(super) body: BoxBody<Bytes, AsyncHttpBodyError>,
}

impl AsyncStreamingHttpProxyResponse {
    /// Creates an async streaming proxy response.
    #[must_use]
    pub fn new(
        status: u16,
        headers: HeaderCollection,
        body: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// Returns status.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Returns headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderCollection {
        &self.headers
    }

    /// Consumes the response into its fields.
    #[must_use]
    pub fn into_parts(self) -> (u16, HeaderCollection, BoxBody<Bytes, AsyncHttpBodyError>) {
        (self.status, self.headers, self.body)
    }
}

/// Upstream transport boundary.
pub trait UpstreamHttpTransport {
    /// Sends a sanitized upstream request.
    fn send(&self, request: UpstreamHttpRequest) -> Result<HttpProxyResponse, HttpProxyError>;
}

/// Upstream transport boundary for streaming response bodies.
pub trait StreamingUpstreamHttpTransport {
    /// Sends a sanitized upstream request and streams the response body.
    fn send_streaming(
        &self,
        request: UpstreamHttpRequest,
    ) -> Result<StreamingHttpProxyResponse, HttpProxyError>;
}

/// Async upstream transport boundary for Hyper-owned HTTP/SSE response bodies.
pub trait AsyncStreamingUpstreamHttpTransport: Send + Sync {
    /// Sends a sanitized upstream request and streams the response body.
    fn send_streaming<'a>(
        &'a self,
        request: StreamingUpstreamHttpRequest,
    ) -> BoxFuture<'a, Result<AsyncStreamingHttpProxyResponse, HttpProxyError>>;
}

/// HTTP proxy failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum HttpProxyError {
    /// Local bearer auth rejected before selection/upstream.
    #[error("local auth rejected request: {reason}")]
    LocalAuth {
        /// Local auth failure reason.
        reason: LocalAuthError,
    },
    /// Request was rejected before selection/upstream.
    #[error("http proxy rejected request: {reason}")]
    Rejected {
        /// Static rejection reason.
        reason: &'static str,
    },
    /// Upstream failed.
    #[error("upstream failed: {message}")]
    Upstream {
        /// Redacted message.
        message: String,
    },
    /// Account selection failed before upstream open.
    #[error("account selection failed: {reason}")]
    Selection {
        /// Selection failure reason.
        reason: QuotaAwareAccountSelectorError,
    },
    /// Provider credential resolution failed before upstream egress.
    #[error("provider credential resolution failed: {reason}")]
    ProviderCredential {
        /// Credential resolver failure reason.
        reason: CredentialResolverError,
    },
}

/// Handles an HTTP request after server parsing.
pub trait HttpRequestHandler {
    /// Handles one parsed HTTP request.
    fn handle_request(
        &self,
        request: HttpProxyRequest,
    ) -> Result<HttpProxyResponse, HttpProxyError>;
}

/// Handles an HTTP request after server parsing with a streaming response body.
pub trait StreamingHttpRequestHandler {
    /// Handles one parsed HTTP request without forcing the response body into memory.
    fn handle_streaming_request(
        &self,
        request: HttpProxyRequest,
    ) -> Result<StreamingHttpProxyResponse, HttpProxyError>;
}

/// Sanitized upstream HTTP/SSE request plus response-side completion metadata.
pub struct PreparedStreamingHttpProxyRequest {
    pub(super) upstream_request: UpstreamHttpRequest,
    pub(super) completion: StreamingHttpProxyCompletion,
}

impl PreparedStreamingHttpProxyRequest {
    /// Consumes the prepared request into the upstream request and completion data.
    #[must_use]
    pub fn into_parts(self) -> (UpstreamHttpRequest, StreamingHttpProxyCompletion) {
        (self.upstream_request, self.completion)
    }
}

/// Sanitized streaming upstream HTTP/SSE request plus completion metadata.
pub struct PreparedAsyncStreamingHttpProxyRequest {
    pub(super) upstream_request: StreamingUpstreamHttpRequest,
    pub(super) completion: StreamingHttpProxyCompletion,
}

impl PreparedAsyncStreamingHttpProxyRequest {
    /// Consumes the prepared request into the upstream request and completion data.
    #[must_use]
    pub fn into_parts(self) -> (StreamingUpstreamHttpRequest, StreamingHttpProxyCompletion) {
        (self.upstream_request, self.completion)
    }
}

/// Metadata needed after an upstream response is committed.
pub struct StreamingHttpProxyCompletion {
    pub(super) affinity_secret: Option<RouterAffinityHashSecret>,
    pub(super) account_id: AccountId,
    pub(super) route_band: RouteBand,
    pub(super) credential_generation: u64,
    pub(super) allowed_audit_event: AuditEvent,
    pub(super) active_reservation_guard: Option<ActiveReservationGuard>,
    pub(super) provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
}

impl StreamingHttpProxyCompletion {
    /// Creates completion metadata for internal runtime tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn new_for_test(
        affinity_secret: Option<RouterAffinityHashSecret>,
        account_id: AccountId,
        credential_generation: u64,
        allowed_audit_event: AuditEvent,
    ) -> Self {
        Self {
            affinity_secret,
            account_id,
            route_band: RouteBand::Responses,
            credential_generation,
            allowed_audit_event,
            active_reservation_guard: None,
            provider_error_observer: None,
        }
    }

    /// Sets the route band for internal runtime tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn with_route_band_for_test(mut self, route_band: RouteBand) -> Self {
        self.route_band = route_band;
        self
    }

    /// Returns the affinity secret for response-owner recording.
    #[must_use]
    pub const fn affinity_secret(&self) -> Option<&RouterAffinityHashSecret> {
        self.affinity_secret.as_ref()
    }

    /// Returns selected account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns selected route band.
    #[must_use]
    pub const fn route_band(&self) -> RouteBand {
        self.route_band
    }

    /// Returns selected credential generation.
    #[must_use]
    pub const fn credential_generation(&self) -> u64 {
        self.credential_generation
    }

    /// Returns the allowed audit event.
    #[must_use]
    pub const fn allowed_audit_event(&self) -> &AuditEvent {
        &self.allowed_audit_event
    }

    /// Returns the active reservation guard for response lifetime ownership.
    #[must_use]
    pub const fn active_reservation_guard(&self) -> Option<&ActiveReservationGuard> {
        self.active_reservation_guard.as_ref()
    }

    /// Returns the optional provider-error observer for quota safety metadata.
    #[must_use]
    pub fn provider_error_observer(&self) -> Option<&Arc<dyn AsyncProviderErrorObserver>> {
        self.provider_error_observer.as_ref()
    }
}

/// Provides router-owned affinity secret material to HTTP/SSE selection.
pub trait HttpAffinitySecretProvider: Send + Sync {
    /// Loads or creates the router affinity secret.
    fn load_or_create_affinity_secret(&self) -> Result<RouterAffinityHashSecret, HttpProxyError>;
}

/// Records successful upstream response ids as previous-response owners.
pub trait HttpAffinityOwnerRecorder: Send + Sync {
    /// Persists one owner row.
    fn record_affinity_owner(
        &self,
        owner: &PreviousResponseAffinityOwnerRecord,
    ) -> Result<(), HttpProxyError>;
}

/// Async previous-response owner recorder for Tokio runtime callers.
pub trait AsyncHttpAffinityOwnerRecorder: Send + Sync {
    /// Persists one owner row without blocking the Tokio worker.
    fn record_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), HttpProxyError>>;
}
