//! HTTP and SSE proxy handling without network binding.

use bytes::Bytes;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_core::affinity::PreviousResponseId;
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::affinity::hash_previous_response_id;
use codex_router_core::audit::AuditEvent;
use codex_router_core::audit::AuditEventFields;
use codex_router_core::audit::AuditFileSink;
use codex_router_core::audit::AuditOutcome;
use codex_router_core::audit::AuditSinkError;
use codex_router_core::audit::LocalAuthAuditResult;
use codex_router_core::audit::ResponseCommitState;
use codex_router_core::audit::RouteKind as AuditRouteKind;
use codex_router_core::audit::TransportKind;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::RequestId;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::redaction::SecretString;
use codex_router_core::routes::RouteBand;
use codex_router_state::affinity_owner::AffinitySourceTransport;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use futures_util::future::BoxFuture;
use http_body_util::combinators::BoxBody;
use std::collections::hash_map::DefaultHasher;
use std::error::Error as StdError;
use std::hash::Hash;
use std::hash::Hasher;
use std::io;
use std::io::Cursor;
use std::io::Read;
use std::sync::Arc;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use thiserror::Error;

use crate::account_selection::AccountDecisionSelector;
use crate::account_selection::QuotaAwareAccountSelectorError;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::local_auth::ProxyLocalAuthGate;
use crate::local_auth::extract_presented_local_token_from_request;
use crate::routes::Method;
use crate::routes::RouteClass;
use crate::routes::RouteKind;
use crate::routes::classify_route;
use crate::upstream::UpstreamRequestBuilder;

/// Reports local audit append failures without exposing request or token material.
pub trait AuditFailureReporter {
    /// Reports one redacted audit failure diagnostic.
    fn report_audit_failure(&self, diagnostic: &str);
}

/// Production audit failure reporter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StderrAuditFailureReporter;

impl AuditFailureReporter for StderrAuditFailureReporter {
    fn report_audit_failure(&self, diagnostic: &str) {
        eprintln!("{diagnostic}");
    }
}

/// Appends one audit event and reports a redacted local diagnostic on failure.
pub fn append_audit_event_with_reporter(
    audit_sink: &AuditFileSink,
    event: &AuditEvent,
    reporter: &impl AuditFailureReporter,
) {
    if let Err(error) = audit_sink.append(event) {
        reporter.report_audit_failure(&audit_failure_diagnostic(&error));
    }
}

fn audit_failure_diagnostic(error: &AuditSinkError) -> String {
    format!("audit append failed: {error}")
}

/// Client HTTP request DTO used by server adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpProxyRequest {
    method: Method,
    path: String,
    websocket_upgrade: bool,
    headers: Vec<Header>,
    body: Vec<u8>,
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

    /// Returns first header value by normalized name.
    #[must_use]
    pub fn header_value(&self, name: &str) -> Option<&str> {
        let normalized = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|header| header.name() == normalized)
            .map(Header::value)
    }
}

/// Upstream HTTP request after proxy sanitization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamHttpRequest {
    method: Method,
    path: String,
    route_kind: RouteKind,
    headers: HeaderCollection,
    body: Vec<u8>,
}

impl UpstreamHttpRequest {
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
}

/// HTTP proxy response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpProxyResponse {
    status: u16,
    headers: HeaderCollection,
    body: Vec<u8>,
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
    status: u16,
    headers: HeaderCollection,
    body: Box<dyn Read + Send>,
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
    status: u16,
    headers: HeaderCollection,
    body: BoxBody<Bytes, AsyncHttpBodyError>,
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
        request: UpstreamHttpRequest,
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
    upstream_request: UpstreamHttpRequest,
    completion: StreamingHttpProxyCompletion,
}

impl PreparedStreamingHttpProxyRequest {
    /// Consumes the prepared request into the upstream request and completion data.
    #[must_use]
    pub fn into_parts(self) -> (UpstreamHttpRequest, StreamingHttpProxyCompletion) {
        (self.upstream_request, self.completion)
    }
}

/// Metadata needed after an upstream response is committed.
pub struct StreamingHttpProxyCompletion {
    affinity_secret: Option<RouterAffinityHashSecret>,
    account_id: AccountId,
    credential_generation: u64,
    allowed_audit_event: AuditEvent,
}

impl StreamingHttpProxyCompletion {
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

/// HTTP/SSE proxy service.
#[derive(Clone, Copy, Debug)]
pub struct HttpProxyService<'a, T> {
    upstream: &'a T,
}

impl<'a, T> HttpProxyService<'a, T> {
    /// Creates a proxy service.
    #[must_use]
    pub const fn new(upstream: &'a T) -> Self {
        Self { upstream }
    }

    fn build_upstream_request(
        &self,
        request: HttpProxyRequest,
        provider_bearer_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> Result<UpstreamHttpRequest, HttpProxyError> {
        let original_path = request.path.clone();
        let classification_path = path_without_query(&request.path);
        let route_kind = match classify_route(
            request.method,
            classification_path,
            request.websocket_upgrade,
        ) {
            RouteClass::Supported(route_kind) => route_kind,
            RouteClass::Rejected { reason } => return Err(HttpProxyError::Rejected { reason }),
        };
        let upstream_request = request
            .headers
            .into_iter()
            .fold(
                UpstreamRequestBuilder::new(route_kind),
                |builder, header| builder.with_header(header),
            )
            .with_body(request.body)
            .build_with_chatgpt_account_id(provider_bearer_token, chatgpt_account_id);

        Ok(UpstreamHttpRequest {
            method: request.method,
            path: original_path,
            route_kind,
            headers: upstream_request.headers().clone(),
            body: upstream_request.body().to_vec(),
        })
    }
}

impl<'a, T> HttpProxyService<'a, T>
where
    T: UpstreamHttpTransport,
{
    /// Handles one HTTP/SSE request.
    pub fn handle(
        &self,
        request: HttpProxyRequest,
        provider_bearer_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> Result<HttpProxyResponse, HttpProxyError> {
        self.build_upstream_request(request, provider_bearer_token, chatgpt_account_id)
            .and_then(|request| self.upstream.send(request))
    }
}

impl<'a, T> HttpProxyService<'a, T>
where
    T: UpstreamHttpTransport + StreamingUpstreamHttpTransport,
{
    /// Handles one HTTP/SSE request without buffering the response body.
    pub fn handle_streaming(
        &self,
        request: HttpProxyRequest,
        provider_bearer_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
        self.build_upstream_request(request, provider_bearer_token, chatgpt_account_id)
            .and_then(|request| self.upstream.send_streaming(request))
    }
}

/// HTTP/SSE service that composes local auth, account selection, and forwarding.
#[derive(Clone)]
pub struct AuthenticatedHttpProxyService<'a, T, S, C> {
    auth_gate: &'a ProxyLocalAuthGate,
    selector: &'a S,
    credential_resolver: &'a C,
    proxy: HttpProxyService<'a, T>,
    audit_sink: Option<&'a AuditFileSink>,
    affinity_secret_provider: Option<&'a dyn HttpAffinitySecretProvider>,
    affinity_owner_recorder: Option<Arc<dyn HttpAffinityOwnerRecorder>>,
}

impl<'a, T, S, C> AuthenticatedHttpProxyService<'a, T, S, C> {
    /// Creates an authenticated HTTP proxy service.
    #[must_use]
    pub const fn new(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        upstream: &'a T,
    ) -> Self {
        Self {
            auth_gate,
            selector,
            credential_resolver,
            proxy: HttpProxyService::new(upstream),
            audit_sink: None,
            affinity_secret_provider: None,
            affinity_owner_recorder: None,
        }
    }

    /// Adds a private audit sink.
    #[must_use]
    pub const fn with_audit_sink(mut self, audit_sink: &'a AuditFileSink) -> Self {
        self.audit_sink = Some(audit_sink);
        self
    }

    /// Adds the router-owned affinity secret provider.
    #[must_use]
    pub const fn with_affinity_secret_provider(
        mut self,
        affinity_secret_provider: &'a dyn HttpAffinitySecretProvider,
    ) -> Self {
        self.affinity_secret_provider = Some(affinity_secret_provider);
        self
    }

    /// Adds the response owner recorder.
    #[must_use]
    pub fn with_affinity_owner_recorder(
        mut self,
        affinity_owner_recorder: Arc<dyn HttpAffinityOwnerRecorder>,
    ) -> Self {
        self.affinity_owner_recorder = Some(affinity_owner_recorder);
        self
    }

    fn load_affinity_secret_for_request(
        &self,
        request: &HttpProxyRequest,
    ) -> Result<Option<RouterAffinityHashSecret>, HttpProxyError> {
        if !request_route_kind(request)?.previous_response_affinity_capable() {
            return Ok(None);
        }

        let provider = self
            .affinity_secret_provider
            .ok_or(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            })?;
        provider.load_or_create_affinity_secret().map(Some)
    }

    fn emit_audit_event(&self, event: AuditEvent) {
        if let Some(audit_sink) = self.audit_sink {
            append_audit_event_with_reporter(audit_sink, &event, &StderrAuditFailureReporter);
        }
    }
}

impl<T, S, C> AuthenticatedHttpProxyService<'_, T, S, C>
where
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    /// Prepares one sanitized upstream HTTP/SSE request without opening upstream.
    pub fn prepare_streaming_request(
        &self,
        request: HttpProxyRequest,
    ) -> Result<PreparedStreamingHttpProxyRequest, HttpProxyError> {
        let audit_route_kind = audit_route_kind_for_request(&request);
        let presented_token = match extract_presented_local_token_from_request(
            request.header_value("x-codex-router-token"),
            request.header_value("authorization"),
            request.header_value("cookie"),
            request.path(),
            request.body(),
            true,
        ) {
            Ok(presented_token) => presented_token,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::Http,
                    audit_route_kind,
                    reason,
                ));
                return Err(HttpProxyError::LocalAuth { reason });
            }
        };
        let token_generation = match self.auth_gate.authorize(presented_token) {
            Ok(generation) => generation,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::Http,
                    audit_route_kind,
                    reason,
                ));
                return Err(HttpProxyError::LocalAuth { reason });
            }
        };
        let affinity_secret = self.load_affinity_secret_for_request(&request)?;
        let selected = self
            .selector
            .select_upstream_account(&request, token_generation, affinity_secret.as_ref())
            .inspect_err(|_error| {
                self.emit_audit_event(http_selection_rejection_audit_event(audit_route_kind));
            })?;
        let account_hash = redacted_account_hash(selected.account_id());
        let resolved = self
            .credential_resolver
            .resolve_provider_credentials(selected.account_id())
            .map_err(|reason| {
                self.emit_audit_event(http_credential_rejection_audit_event(
                    audit_route_kind,
                    account_hash.clone(),
                ));
                HttpProxyError::ProviderCredential { reason }
            })?;
        let upstream_request = self.proxy.build_upstream_request(
            request,
            resolved.access_token().clone(),
            resolved.chatgpt_account_id(),
        )?;
        let completion = StreamingHttpProxyCompletion {
            affinity_secret,
            account_id: selected.account_id().clone(),
            credential_generation: resolved.credential_generation(),
            allowed_audit_event: allowed_audit_event(
                TransportKind::Http,
                audit_route_kind,
                account_hash,
            ),
        };

        Ok(PreparedStreamingHttpProxyRequest {
            upstream_request,
            completion,
        })
    }
}

impl<T, S, C> HttpRequestHandler for AuthenticatedHttpProxyService<'_, T, S, C>
where
    T: UpstreamHttpTransport,
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    fn handle_request(
        &self,
        request: HttpProxyRequest,
    ) -> Result<HttpProxyResponse, HttpProxyError> {
        let audit_route_kind = audit_route_kind_for_request(&request);
        let presented_token = match extract_presented_local_token_from_request(
            request.header_value("x-codex-router-token"),
            request.header_value("authorization"),
            request.header_value("cookie"),
            request.path(),
            request.body(),
            true,
        ) {
            Ok(presented_token) => presented_token,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::Http,
                    audit_route_kind,
                    reason,
                ));
                return Err(HttpProxyError::LocalAuth { reason });
            }
        };
        let token_generation = match self.auth_gate.authorize(presented_token) {
            Ok(generation) => generation,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::Http,
                    audit_route_kind,
                    reason,
                ));
                return Err(HttpProxyError::LocalAuth { reason });
            }
        };
        let affinity_secret = self.load_affinity_secret_for_request(&request)?;
        let selected = self
            .selector
            .select_upstream_account(&request, token_generation, affinity_secret.as_ref())
            .inspect_err(|_error| {
                self.emit_audit_event(http_selection_rejection_audit_event(audit_route_kind));
            })?;
        let account_hash = redacted_account_hash(selected.account_id());
        let resolved = self
            .credential_resolver
            .resolve_provider_credentials(selected.account_id())
            .map_err(|reason| {
                self.emit_audit_event(http_credential_rejection_audit_event(
                    audit_route_kind,
                    account_hash.clone(),
                ));
                HttpProxyError::ProviderCredential { reason }
            })?;

        let response = self.proxy.handle(
            request,
            resolved.access_token().clone(),
            resolved.chatgpt_account_id(),
        )?;
        self.record_buffered_response_owner(
            &response,
            affinity_secret.as_ref(),
            selected.account_id(),
            resolved.credential_generation(),
        )?;
        self.emit_audit_event(allowed_audit_event(
            TransportKind::Http,
            audit_route_kind,
            account_hash,
        ));

        Ok(response)
    }
}

impl<T, S, C> StreamingHttpRequestHandler for AuthenticatedHttpProxyService<'_, T, S, C>
where
    T: UpstreamHttpTransport + StreamingUpstreamHttpTransport,
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    fn handle_streaming_request(
        &self,
        request: HttpProxyRequest,
    ) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
        let prepared = self.prepare_streaming_request(request)?;
        let (upstream_request, completion) = prepared.into_parts();
        let response = self.proxy.upstream.send_streaming(upstream_request)?;
        let response = self.wrap_streaming_response_owner_recorder(
            response,
            completion.affinity_secret,
            completion.account_id,
            completion.credential_generation,
        );
        self.emit_audit_event(completion.allowed_audit_event);

        Ok(response)
    }
}

impl<T, S, C> AuthenticatedHttpProxyService<'_, T, S, C>
where
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    fn record_buffered_response_owner(
        &self,
        response: &HttpProxyResponse,
        affinity_secret: Option<&RouterAffinityHashSecret>,
        account_id: &AccountId,
        credential_generation: u64,
    ) -> Result<(), HttpProxyError> {
        let Some(affinity_secret) = affinity_secret else {
            return Ok(());
        };
        let Some(response_id) = extract_response_id_from_body(response.body())? else {
            return Ok(());
        };
        self.record_response_id_owner(
            affinity_secret,
            &response_id,
            account_id,
            credential_generation,
        )
    }

    fn wrap_streaming_response_owner_recorder(
        &self,
        response: StreamingHttpProxyResponse,
        affinity_secret: Option<RouterAffinityHashSecret>,
        account_id: AccountId,
        credential_generation: u64,
    ) -> StreamingHttpProxyResponse {
        let Some(affinity_secret) = affinity_secret else {
            return response;
        };
        let Some(recorder) = self.affinity_owner_recorder.as_ref() else {
            return response;
        };

        StreamingHttpProxyResponse::new(
            response.status,
            response.headers,
            Box::new(AffinityOwnerRecordingBody::new(
                response.body,
                Arc::clone(recorder),
                affinity_secret,
                account_id,
                credential_generation,
            )),
        )
    }

    fn record_response_id_owner(
        &self,
        affinity_secret: &RouterAffinityHashSecret,
        response_id: &PreviousResponseId,
        account_id: &AccountId,
        credential_generation: u64,
    ) -> Result<(), HttpProxyError> {
        let Some(recorder) = self.affinity_owner_recorder.as_ref() else {
            return Ok(());
        };
        let affinity_key_hash =
            hash_previous_response_id(affinity_secret, response_id).map_err(|_error| {
                HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
                }
            })?;
        let owner = PreviousResponseAffinityOwnerRecord::new(
            affinity_key_hash,
            account_id.clone(),
            credential_generation,
            RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            current_unix_seconds(),
        );
        recorder.record_affinity_owner(&owner)
    }
}

struct AffinityOwnerRecordingBody {
    inner: Box<dyn Read + Send>,
    recorder: Arc<dyn HttpAffinityOwnerRecorder>,
    affinity_secret: RouterAffinityHashSecret,
    account_id: AccountId,
    credential_generation: u64,
    buffered: Vec<u8>,
    recorded: bool,
}

impl AffinityOwnerRecordingBody {
    fn new(
        inner: Box<dyn Read + Send>,
        recorder: Arc<dyn HttpAffinityOwnerRecorder>,
        affinity_secret: RouterAffinityHashSecret,
        account_id: AccountId,
        credential_generation: u64,
    ) -> Self {
        Self {
            inner,
            recorder,
            affinity_secret,
            account_id,
            credential_generation,
            buffered: Vec::new(),
            recorded: false,
        }
    }

    fn record_if_ready(&mut self) -> io::Result<()> {
        if self.recorded {
            return Ok(());
        }
        let Some(response_id) = extract_response_id_from_body(&self.buffered)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
        else {
            return Ok(());
        };
        self.recorded = true;
        let affinity_key_hash = hash_previous_response_id(&self.affinity_secret, &response_id)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let owner = PreviousResponseAffinityOwnerRecord::new(
            affinity_key_hash,
            self.account_id.clone(),
            self.credential_generation,
            RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            current_unix_seconds(),
        );
        self.recorder
            .record_affinity_owner(&owner)
            .map_err(|error| io::Error::other(error.to_string()))
    }
}

impl Read for AffinityOwnerRecordingBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        if read == 0 {
            self.record_if_ready()?;
            return Ok(0);
        }
        self.buffered.extend_from_slice(&buffer[..read]);
        self.record_if_ready()?;
        Ok(read)
    }
}

pub(crate) fn redacted_account_hash(account_id: &AccountId) -> String {
    let mut hasher = DefaultHasher::new();
    account_id.as_str().hash(&mut hasher);

    format!("acct_{:016x}", hasher.finish())
}

pub(crate) const fn local_auth_audit_result(reason: LocalAuthError) -> LocalAuthAuditResult {
    match reason {
        LocalAuthError::Missing => LocalAuthAuditResult::Missing,
        LocalAuthError::Empty => LocalAuthAuditResult::Empty,
        LocalAuthError::Old => LocalAuthAuditResult::Old,
        LocalAuthError::Wrong => LocalAuthAuditResult::Wrong,
    }
}

pub(crate) const fn local_auth_decision_reason(reason: LocalAuthError) -> &'static str {
    match reason {
        LocalAuthError::Missing => "local_auth_missing",
        LocalAuthError::Empty => "local_auth_empty",
        LocalAuthError::Old => "local_auth_old",
        LocalAuthError::Wrong => "local_auth_wrong",
    }
}

pub(crate) fn local_auth_rejection_audit_event(
    transport_kind: TransportKind,
    route_kind: AuditRouteKind,
    reason: LocalAuthError,
) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind,
        transport_kind,
        local_auth_result: local_auth_audit_result(reason),
        outcome: AuditOutcome::Rejected,
        decision_reason: local_auth_decision_reason(reason),
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: None,
        error_class: Some("local_auth"),
    })
}

pub(crate) fn allowed_audit_event(
    transport_kind: TransportKind,
    route_kind: AuditRouteKind,
    account_hash: String,
) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind,
        transport_kind,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Allowed,
        decision_reason: "forwarded",
        response_commit_state: ResponseCommitState::Committed,
        account_hash: Some(account_hash),
        error_class: None,
    })
}

fn http_selection_rejection_audit_event(route_kind: AuditRouteKind) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind,
        transport_kind: TransportKind::Http,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "selection_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: None,
        error_class: Some("selection"),
    })
}

fn http_credential_rejection_audit_event(
    route_kind: AuditRouteKind,
    account_hash: String,
) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind,
        transport_kind: TransportKind::Http,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "credential_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: Some(account_hash),
        error_class: Some("provider_credential"),
    })
}

fn audit_route_kind_for_request(request: &HttpProxyRequest) -> AuditRouteKind {
    match request_route_kind(request) {
        Ok(route_kind) => match route_kind {
            RouteKind::Responses => AuditRouteKind::Responses,
            RouteKind::ResponsesWebSocket => AuditRouteKind::ResponsesWebSocket,
            RouteKind::Models => AuditRouteKind::Models,
            RouteKind::MemoriesTraceSummarize => AuditRouteKind::MemoryTrace,
            RouteKind::ResponsesCompact => AuditRouteKind::Compact,
        },
        Err(_error) => AuditRouteKind::Responses,
    }
}

pub(crate) fn extract_response_id_from_body(
    body: &[u8],
) -> Result<Option<PreviousResponseId>, HttpProxyError> {
    if let Some(response_id) = extract_json_response_id(body)? {
        return Ok(Some(response_id));
    }

    for line in body.split(|byte| *byte == b'\n') {
        let line = trim_ascii(line);
        let Some(data) = line.strip_prefix(b"data:") else {
            continue;
        };
        let data = trim_ascii(data);
        if data == b"[DONE]" || data.is_empty() {
            continue;
        }
        if let Some(response_id) = extract_json_response_id(data)? {
            return Ok(Some(response_id));
        }
    }

    Ok(None)
}

fn extract_json_response_id(body: &[u8]) -> Result<Option<PreviousResponseId>, HttpProxyError> {
    let value = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) => value,
        Err(_error) => return Ok(None),
    };
    let Some(response_id) = value.get("id").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    if response_id.is_empty() {
        return Ok(None);
    }
    PreviousResponseId::new(response_id.to_owned())
        .map(Some)
        .map_err(|_error| HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
        })
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
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

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn request_route_kind(request: &HttpProxyRequest) -> Result<RouteKind, HttpProxyError> {
    match classify_route(
        request.method,
        path_without_query(request.path()),
        request.websocket_upgrade,
    ) {
        RouteClass::Supported(route_kind) => Ok(route_kind),
        RouteClass::Rejected { reason } => Err(HttpProxyError::Rejected { reason }),
    }
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?')
        .map_or(path, |(path_component, _query)| path_component)
}
