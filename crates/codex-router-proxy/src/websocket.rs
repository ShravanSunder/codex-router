//! WebSocket first-frame routing protocol.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::Shutdown;
use std::net::TcpStream;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_core::affinity::PreviousResponseId;
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::affinity::hash_previous_response_id;
use codex_router_core::audit::AuditEvent;
use codex_router_core::audit::AuditEventFields;
use codex_router_core::audit::AuditFileSink;
use codex_router_core::audit::AuditOutcome;
use codex_router_core::audit::LocalAuthAuditResult;
use codex_router_core::audit::ResponseCommitState;
use codex_router_core::audit::RouteKind as AuditRouteKind;
use codex_router_core::audit::TransportKind;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::RequestId;
use codex_router_core::ids::TokenGeneration;
use codex_router_core::redaction::SecretString;
use codex_router_core::routes::RouteBand;
use codex_router_state::affinity_owner::AffinitySourceTransport;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use futures_util::SinkExt;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use thiserror::Error;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tungstenite::Message;
use tungstenite::WebSocket;
use tungstenite::accept_hdr;
use tungstenite::client::IntoClientRequest;
use tungstenite::connect;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;
use tungstenite::http::HeaderName;
use tungstenite::http::HeaderValue;

use crate::account_selection::AccountDecisionSelector;
use crate::account_selection::AsyncAccountDecisionSelector;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::headers::sanitize_headers_for_upstream;
use crate::http_sse::HttpAffinityOwnerRecorder;
use crate::http_sse::HttpAffinitySecretProvider;
use crate::http_sse::HttpProxyRequest;
use crate::http_sse::StderrAuditFailureReporter;
use crate::http_sse::allowed_audit_event;
use crate::http_sse::append_audit_event_with_reporter;
use crate::http_sse::local_auth_rejection_audit_event;
use crate::http_sse::redacted_account_hash;
use crate::local_auth::ProxyLocalAuthGate;
use crate::local_auth::extract_presented_local_token_from_request;
use crate::local_auth::has_forbidden_top_level_json_auth_carrier;
use crate::routes::Method;

/// WebSocket frame subset needed before upstream connection opens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketFrame {
    /// Text frame bytes.
    Text(Vec<u8>),
    /// Binary frame bytes.
    Binary(Vec<u8>),
}

/// WebSocket first-frame resource policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirstFramePolicy {
    max_first_frame_bytes: usize,
}

impl FirstFramePolicy {
    /// Creates first-frame policy.
    #[must_use]
    pub const fn new(max_first_frame_bytes: usize) -> Self {
        Self {
            max_first_frame_bytes,
        }
    }
}

/// Local WebSocket handshake request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WebSocketHandshakeRequest {
    headers: Vec<Header>,
}

impl WebSocketHandshakeRequest {
    /// Creates an empty handshake request.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            headers: Vec::new(),
        }
    }

    /// Adds a handshake header.
    #[must_use]
    pub fn with_header(mut self, header: Header) -> Self {
        self.headers.push(header);
        self
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

/// Decision after receiving the first local WebSocket frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketFirstFrameDecision {
    /// Open upstream with sanitized headers and forward first frame unchanged.
    OpenUpstream {
        /// Local token generation used to authorize the connection.
        token_generation: TokenGeneration,
        /// Sanitized upstream handshake headers.
        headers: HeaderCollection,
        /// First frame to forward unchanged.
        first_frame: WebSocketFrame,
        /// Context for recording upstream response owners.
        affinity_owner_context: Option<WebSocketAffinityOwnerContext>,
    },
}

/// Safe metadata needed to record WebSocket previous-response owners.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketAffinityOwnerContext {
    affinity_secret: RouterAffinityHashSecret,
    account_id: AccountId,
    credential_generation: u64,
}

impl WebSocketAffinityOwnerContext {
    fn new(
        affinity_secret: RouterAffinityHashSecret,
        account_id: AccountId,
        credential_generation: u64,
    ) -> Self {
        Self {
            affinity_secret,
            account_id,
            credential_generation,
        }
    }
}

/// Local close reason before upstream is opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketCloseReason {
    /// Local bearer auth rejected before account selection/upstream open.
    LocalAuth {
        /// Local auth failure reason.
        reason: codex_router_core::local_auth::LocalAuthError,
    },
    /// Account selection failed before upstream open.
    Selection,
    /// Provider credential resolution failed before upstream open.
    ProviderCredential,
    /// First frame exceeded local resource limit.
    FirstFrameTooLarge,
    /// First frame was not text.
    UnsupportedFirstFrameType,
    /// First frame text was not valid JSON.
    MalformedFirstFrame,
    /// First frame was not a Responses WebSocket create request.
    UnexpectedFirstFrame,
    /// First frame did not arrive before the local preselection deadline.
    FirstFrameTimeout,
}

/// WebSocket first-frame router.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebSocketProtocolRouter {
    first_frame_policy: FirstFramePolicy,
}

impl WebSocketProtocolRouter {
    /// Creates a WebSocket protocol router.
    #[must_use]
    pub const fn new(first_frame_policy: FirstFramePolicy) -> Self {
        Self { first_frame_policy }
    }

    /// Validates the first frame before account selection or upstream open.
    pub fn validate_first_frame<'a>(
        &self,
        first_frame: &'a WebSocketFrame,
    ) -> Result<&'a [u8], WebSocketCloseReason> {
        let WebSocketFrame::Text(first_frame_bytes) = first_frame else {
            return Err(WebSocketCloseReason::UnsupportedFirstFrameType);
        };
        if first_frame_bytes.len() > self.first_frame_policy.max_first_frame_bytes {
            return Err(WebSocketCloseReason::FirstFrameTooLarge);
        }
        let payload = serde_json::from_slice::<serde_json::Value>(first_frame_bytes)
            .map_err(|_| WebSocketCloseReason::MalformedFirstFrame)?;
        if has_forbidden_top_level_json_auth_carrier(first_frame_bytes) {
            return Err(WebSocketCloseReason::UnexpectedFirstFrame);
        }
        if let Some(frame_type) = payload.get("type").and_then(serde_json::Value::as_str) {
            if frame_type != "response.create" {
                return Err(WebSocketCloseReason::UnexpectedFirstFrame);
            }
            return Ok(first_frame_bytes);
        }

        if !is_direct_response_create_payload(&payload) {
            return Err(WebSocketCloseReason::UnexpectedFirstFrame);
        }

        Ok(first_frame_bytes)
    }

    /// Routes the first frame, returning either sanitized upstream open data or a local close reason.
    pub fn route_first_frame(
        &self,
        handshake: WebSocketHandshakeRequest,
        first_frame: WebSocketFrame,
        provider_bearer_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> Result<WebSocketFirstFrameDecision, WebSocketCloseReason> {
        self.validate_first_frame(&first_frame)?;

        Ok(WebSocketFirstFrameDecision::OpenUpstream {
            token_generation: TokenGeneration::new(0),
            headers: sanitize_headers_for_upstream(
                handshake.headers,
                provider_bearer_token,
                chatgpt_account_id,
            ),
            first_frame,
            affinity_owner_context: None,
        })
    }
}

fn is_direct_response_create_payload(payload: &serde_json::Value) -> bool {
    payload
        .get("model")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|model| !model.is_empty())
        && payload
            .get("input")
            .and_then(serde_json::Value::as_array)
            .is_some()
        && payload.get("stream").and_then(serde_json::Value::as_bool) == Some(true)
}

/// WebSocket router that composes local auth, account selection, and first-frame routing.
#[derive(Clone, Copy)]
pub struct AuthenticatedWebSocketRouter<'a, S, C>
where
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    auth_gate: &'a ProxyLocalAuthGate,
    selector: &'a S,
    credential_resolver: &'a C,
    protocol_router: &'a WebSocketProtocolRouter,
    audit_sink: Option<&'a AuditFileSink>,
    affinity_secret_provider: Option<&'a dyn HttpAffinitySecretProvider>,
}

/// Resolves provider credentials for async WebSocket runtime callers.
pub trait AsyncProviderCredentialResolver {
    /// Resolves credentials immediately before provider egress.
    fn resolve_provider_credentials<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>>;
}

/// Async WebSocket router that composes local auth, async account selection,
/// and async credential resolution.
#[derive(Clone, Copy)]
pub struct AsyncAuthenticatedWebSocketRouter<'a, S, C>
where
    S: AsyncAccountDecisionSelector,
    C: AsyncProviderCredentialResolver,
{
    auth_gate: &'a ProxyLocalAuthGate,
    selector: &'a S,
    credential_resolver: &'a C,
    protocol_router: &'a WebSocketProtocolRouter,
    audit_sink: Option<&'a AuditFileSink>,
    affinity_secret_provider: Option<&'a dyn HttpAffinitySecretProvider>,
}

impl<'a, S, C> AuthenticatedWebSocketRouter<'a, S, C>
where
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    /// Creates an authenticated WebSocket router.
    #[must_use]
    pub const fn new(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
    ) -> Self {
        Self {
            auth_gate,
            selector,
            credential_resolver,
            protocol_router,
            audit_sink: None,
            affinity_secret_provider: None,
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

    fn emit_audit_event(&self, event: AuditEvent) {
        if let Some(audit_sink) = self.audit_sink {
            append_audit_event_with_reporter(audit_sink, &event, &StderrAuditFailureReporter);
        }
    }

    /// Routes one authenticated WebSocket first frame.
    pub fn route_first_frame(
        &self,
        handshake: WebSocketHandshakeRequest,
        first_frame: WebSocketFrame,
    ) -> Result<WebSocketFirstFrameDecision, WebSocketCloseReason> {
        let presented_token = match extract_presented_local_token_from_request(
            handshake.header_value("x-codex-router-token"),
            handshake.header_value("authorization"),
            handshake.header_value("cookie"),
            "",
            &[],
            false,
        ) {
            Ok(presented_token) => presented_token,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::WebSocket,
                    AuditRouteKind::ResponsesWebSocket,
                    reason,
                ));
                return Err(WebSocketCloseReason::LocalAuth { reason });
            }
        };
        let token_generation = match self.auth_gate.authorize(presented_token) {
            Ok(generation) => generation,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::WebSocket,
                    AuditRouteKind::ResponsesWebSocket,
                    reason,
                ));
                return Err(WebSocketCloseReason::LocalAuth { reason });
            }
        };
        let first_frame_bytes = self
            .protocol_router
            .validate_first_frame(&first_frame)
            .inspect_err(|_reason| {
                self.emit_audit_event(websocket_first_frame_rejection_audit_event(None));
            })?
            .to_vec();
        let selection_request = HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_websocket_upgrade(true)
            .with_body(first_frame_bytes);
        let affinity_secret = self.load_affinity_secret().map_err(|_reason| {
            self.emit_audit_event(websocket_selection_rejection_audit_event());
            WebSocketCloseReason::Selection
        })?;
        let selected = self
            .selector
            .select_upstream_account(&selection_request, token_generation, Some(&affinity_secret))
            .map_err(|_error| {
                self.emit_audit_event(websocket_selection_rejection_audit_event());
                WebSocketCloseReason::Selection
            })?;
        let account_hash = redacted_account_hash(selected.account_id());
        let resolved = self
            .credential_resolver
            .resolve_provider_credentials(selected.account_id())
            .map_err(|_reason| {
                self.emit_audit_event(websocket_credential_rejection_audit_event(
                    account_hash.clone(),
                ));
                WebSocketCloseReason::ProviderCredential
            })?;

        let decision = self
            .protocol_router
            .route_first_frame(
                handshake,
                first_frame,
                resolved.access_token().clone(),
                resolved.chatgpt_account_id(),
            )
            .inspect_err(|_reason| {
                self.emit_audit_event(websocket_first_frame_rejection_audit_event(Some(
                    account_hash.clone(),
                )));
            })?;
        self.emit_audit_event(allowed_audit_event(
            TransportKind::WebSocket,
            AuditRouteKind::ResponsesWebSocket,
            account_hash,
        ));

        Ok(match decision {
            WebSocketFirstFrameDecision::OpenUpstream {
                headers,
                first_frame,
                affinity_owner_context: _,
                ..
            } => WebSocketFirstFrameDecision::OpenUpstream {
                token_generation,
                headers,
                first_frame,
                affinity_owner_context: Some(WebSocketAffinityOwnerContext::new(
                    affinity_secret,
                    selected.account_id().clone(),
                    resolved.credential_generation(),
                )),
            },
        })
    }

    fn load_affinity_secret(&self) -> Result<RouterAffinityHashSecret, WebSocketCloseReason> {
        let provider = self
            .affinity_secret_provider
            .ok_or(WebSocketCloseReason::Selection)?;
        provider
            .load_or_create_affinity_secret()
            .map_err(|_error| WebSocketCloseReason::Selection)
    }
}

impl<'a, S, C> AsyncAuthenticatedWebSocketRouter<'a, S, C>
where
    S: AsyncAccountDecisionSelector,
    C: AsyncProviderCredentialResolver,
{
    /// Creates an async authenticated WebSocket router.
    #[must_use]
    pub const fn new(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
    ) -> Self {
        Self {
            auth_gate,
            selector,
            credential_resolver,
            protocol_router,
            audit_sink: None,
            affinity_secret_provider: None,
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

    fn emit_audit_event(&self, event: AuditEvent) {
        if let Some(audit_sink) = self.audit_sink {
            append_audit_event_with_reporter(audit_sink, &event, &StderrAuditFailureReporter);
        }
    }

    /// Routes one authenticated WebSocket first frame without blocking on
    /// selector or credential resolution.
    pub async fn route_first_frame(
        &self,
        handshake: WebSocketHandshakeRequest,
        first_frame: WebSocketFrame,
    ) -> Result<WebSocketFirstFrameDecision, WebSocketCloseReason> {
        let presented_token = match extract_presented_local_token_from_request(
            handshake.header_value("x-codex-router-token"),
            handshake.header_value("authorization"),
            handshake.header_value("cookie"),
            "",
            &[],
            false,
        ) {
            Ok(presented_token) => presented_token,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::WebSocket,
                    AuditRouteKind::ResponsesWebSocket,
                    reason,
                ));
                return Err(WebSocketCloseReason::LocalAuth { reason });
            }
        };
        let token_generation = match self.auth_gate.authorize(presented_token) {
            Ok(generation) => generation,
            Err(reason) => {
                self.emit_audit_event(local_auth_rejection_audit_event(
                    TransportKind::WebSocket,
                    AuditRouteKind::ResponsesWebSocket,
                    reason,
                ));
                return Err(WebSocketCloseReason::LocalAuth { reason });
            }
        };
        let first_frame_bytes = self
            .protocol_router
            .validate_first_frame(&first_frame)
            .inspect_err(|_reason| {
                self.emit_audit_event(websocket_first_frame_rejection_audit_event(None));
            })?
            .to_vec();
        let selection_request = HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_websocket_upgrade(true)
            .with_body(first_frame_bytes);
        let affinity_secret = self.load_affinity_secret().map_err(|_reason| {
            self.emit_audit_event(websocket_selection_rejection_audit_event());
            WebSocketCloseReason::Selection
        })?;
        let selected = self
            .selector
            .select_upstream_account(&selection_request, token_generation, Some(&affinity_secret))
            .await
            .map_err(|_error| {
                self.emit_audit_event(websocket_selection_rejection_audit_event());
                WebSocketCloseReason::Selection
            })?;
        let account_hash = redacted_account_hash(selected.account_id());
        let resolved = self
            .credential_resolver
            .resolve_provider_credentials(selected.account_id())
            .await
            .map_err(|_reason| {
                self.emit_audit_event(websocket_credential_rejection_audit_event(
                    account_hash.clone(),
                ));
                WebSocketCloseReason::ProviderCredential
            })?;

        let decision = self
            .protocol_router
            .route_first_frame(
                handshake,
                first_frame,
                resolved.access_token().clone(),
                resolved.chatgpt_account_id(),
            )
            .inspect_err(|_reason| {
                self.emit_audit_event(websocket_first_frame_rejection_audit_event(Some(
                    account_hash.clone(),
                )));
            })?;
        self.emit_audit_event(allowed_audit_event(
            TransportKind::WebSocket,
            AuditRouteKind::ResponsesWebSocket,
            account_hash,
        ));

        Ok(match decision {
            WebSocketFirstFrameDecision::OpenUpstream {
                headers,
                first_frame,
                affinity_owner_context: _,
                ..
            } => WebSocketFirstFrameDecision::OpenUpstream {
                token_generation,
                headers,
                first_frame,
                affinity_owner_context: Some(WebSocketAffinityOwnerContext::new(
                    affinity_secret,
                    selected.account_id().clone(),
                    resolved.credential_generation(),
                )),
            },
        })
    }

    fn load_affinity_secret(&self) -> Result<RouterAffinityHashSecret, WebSocketCloseReason> {
        let provider = self
            .affinity_secret_provider
            .ok_or(WebSocketCloseReason::Selection)?;
        provider
            .load_or_create_affinity_secret()
            .map_err(|_error| WebSocketCloseReason::Selection)
    }
}

fn websocket_selection_rejection_audit_event() -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind: AuditRouteKind::ResponsesWebSocket,
        transport_kind: TransportKind::WebSocket,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "selection_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: None,
        error_class: Some("selection"),
    })
}

fn websocket_first_frame_rejection_audit_event(account_hash: Option<String>) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind: AuditRouteKind::ResponsesWebSocket,
        transport_kind: TransportKind::WebSocket,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "first_frame_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash,
        error_class: Some("websocket_first_frame"),
    })
}

fn websocket_credential_rejection_audit_event(account_hash: String) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind: AuditRouteKind::ResponsesWebSocket,
        transport_kind: TransportKind::WebSocket,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "credential_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: Some(account_hash),
        error_class: Some("provider_credential"),
    })
}

/// Tracks active local WebSocket streams by local token generation.
#[derive(Clone, Debug, Default)]
pub struct WebSocketRevocationRegistry {
    connections: Arc<Mutex<HashMap<TokenGeneration, Vec<TcpStream>>>>,
}

impl WebSocketRevocationRegistry {
    /// Creates an empty revocation registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn register(
        &self,
        generation: TokenGeneration,
        stream: &TcpStream,
    ) -> Result<(), WebSocketTunnelError> {
        let stream = stream
            .try_clone()
            .map_err(WebSocketTunnelError::ConnectionTracking)?;
        if let Ok(mut connections) = self.connections.lock() {
            connections.entry(generation).or_default().push(stream);
        }

        Ok(())
    }

    /// Closes connections that authenticated with generations other than the active one.
    pub fn close_all_except(&self, active_generation: TokenGeneration) {
        let Ok(mut connections) = self.connections.lock() else {
            return;
        };
        let stale_generations = connections
            .keys()
            .copied()
            .filter(|generation| *generation != active_generation)
            .collect::<Vec<_>>();
        for stale_generation in stale_generations {
            if let Some(streams) = connections.remove(&stale_generation) {
                for stream in streams {
                    let _result = stream.shutdown(Shutdown::Both);
                }
            }
        }
    }
}

/// Blocking WebSocket tunnel that uses the authenticated first-frame router.
#[derive(Clone)]
pub struct BlockingWebSocketTunnel<'a, S, C>
where
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    router: AuthenticatedWebSocketRouter<'a, S, C>,
    revocations: WebSocketRevocationRegistry,
    affinity_owner_recorder: Option<&'a dyn HttpAffinityOwnerRecorder>,
}

/// Async WebSocket tunnel that uses the authenticated first-frame router.
#[derive(Clone)]
pub struct AsyncWebSocketTunnel<'a, S, C>
where
    S: AsyncAccountDecisionSelector,
    C: AsyncProviderCredentialResolver,
{
    router: AsyncAuthenticatedWebSocketRouter<'a, S, C>,
    affinity_owner_recorder: Option<&'a dyn HttpAffinityOwnerRecorder>,
}

impl<'a, S, C> BlockingWebSocketTunnel<'a, S, C>
where
    S: AccountDecisionSelector,
    C: ProviderCredentialResolver,
{
    /// Creates a blocking WebSocket tunnel.
    #[must_use]
    pub fn new(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
    ) -> Self {
        Self {
            router: AuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            ),
            revocations: WebSocketRevocationRegistry::new(),
            affinity_owner_recorder: None,
        }
    }

    /// Creates a blocking WebSocket tunnel with shared revocation tracking.
    #[must_use]
    pub fn new_with_revocation_registry(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
        revocations: WebSocketRevocationRegistry,
    ) -> Self {
        Self {
            router: AuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            ),
            revocations,
            affinity_owner_recorder: None,
        }
    }

    /// Creates a blocking WebSocket tunnel with revocation tracking and a private audit sink.
    #[must_use]
    pub fn new_with_revocation_registry_and_audit_sink(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
        revocations: WebSocketRevocationRegistry,
        audit_sink: &'a AuditFileSink,
    ) -> Self {
        Self {
            router: AuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            )
            .with_audit_sink(audit_sink),
            revocations,
            affinity_owner_recorder: None,
        }
    }

    /// Adds the router-owned affinity secret provider.
    #[must_use]
    pub fn with_affinity_secret_provider(
        mut self,
        affinity_secret_provider: &'a dyn HttpAffinitySecretProvider,
    ) -> Self {
        self.router = self
            .router
            .with_affinity_secret_provider(affinity_secret_provider);
        self
    }

    /// Adds the previous-response owner recorder.
    #[must_use]
    pub fn with_affinity_owner_recorder(
        mut self,
        affinity_owner_recorder: &'a dyn HttpAffinityOwnerRecorder,
    ) -> Self {
        self.affinity_owner_recorder = Some(affinity_owner_recorder);
        self
    }

    /// Handles one local WebSocket connection and forwards a bounded upstream transcript.
    pub fn handle_connection(
        &self,
        local_stream: TcpStream,
        upstream_url: &str,
        max_upstream_messages: usize,
    ) -> Result<(), WebSocketTunnelError> {
        let captured_handshake = Arc::new(Mutex::new(None));
        let handshake_for_callback = Arc::clone(&captured_handshake);
        let mut local_websocket =
            accept_local_websocket(local_stream, move |request: &Request| {
                let handshake = handshake_from_request(request);
                match handshake_for_callback.lock() {
                    Ok(mut captured) => {
                        *captured = Some(handshake);
                    }
                    Err(_error) => {}
                }
            })?;
        let handshake = take_captured_handshake(&captured_handshake)?;
        local_websocket
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(250)))
            .map_err(|error| WebSocketTunnelError::Transport(tungstenite::Error::Io(error)))?;
        let first_message = match local_websocket.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(error))
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut =>
            {
                local_websocket.close(None)?;
                return Err(WebSocketTunnelError::CloseReason(
                    WebSocketCloseReason::FirstFrameTimeout,
                ));
            }
            Err(error) => return Err(WebSocketTunnelError::Transport(error)),
        };
        let first_frame = frame_from_message(first_message);
        let decision = self
            .router
            .route_first_frame(handshake, first_frame)
            .map_err(WebSocketTunnelError::CloseReason)?;
        let WebSocketFirstFrameDecision::OpenUpstream {
            token_generation,
            headers,
            first_frame,
            affinity_owner_context,
        } = decision;
        self.revocations
            .register(token_generation, local_websocket.get_ref())?;

        let mut upstream_request = upstream_url.into_client_request()?;
        apply_upstream_headers(upstream_request.headers_mut(), &headers)?;
        let (mut upstream_websocket, _response) = connect(upstream_request)?;
        upstream_websocket.send(message_from_frame(first_frame)?)?;
        forward_upstream_response(
            &mut upstream_websocket,
            &mut local_websocket,
            max_upstream_messages,
            self.affinity_owner_recorder,
            affinity_owner_context.as_ref(),
        )?;
        local_websocket
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|error| WebSocketTunnelError::Transport(tungstenite::Error::Io(error)))?;

        loop {
            let local_message = match local_websocket.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(error))
                    if error.kind() == ErrorKind::WouldBlock
                        || error.kind() == ErrorKind::TimedOut =>
                {
                    local_websocket.close(None)?;
                    upstream_websocket.close(None)?;
                    return Ok(());
                }
                Err(error) => return Err(WebSocketTunnelError::Transport(error)),
            };
            let is_close = matches!(local_message, Message::Close(_));
            upstream_websocket.send(local_message)?;
            if is_close {
                return Ok(());
            }
            forward_upstream_response(
                &mut upstream_websocket,
                &mut local_websocket,
                max_upstream_messages,
                self.affinity_owner_recorder,
                affinity_owner_context.as_ref(),
            )?;
        }
    }
}

impl<'a, S, C> AsyncWebSocketTunnel<'a, S, C>
where
    S: AsyncAccountDecisionSelector,
    C: AsyncProviderCredentialResolver,
{
    /// Creates an async WebSocket tunnel.
    #[must_use]
    pub fn new(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
    ) -> Self {
        Self {
            router: AsyncAuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            ),
            affinity_owner_recorder: None,
        }
    }

    /// Creates an async WebSocket tunnel with a private audit sink.
    #[must_use]
    pub fn new_with_audit_sink(
        auth_gate: &'a ProxyLocalAuthGate,
        selector: &'a S,
        credential_resolver: &'a C,
        protocol_router: &'a WebSocketProtocolRouter,
        audit_sink: &'a AuditFileSink,
    ) -> Self {
        Self {
            router: AsyncAuthenticatedWebSocketRouter::new(
                auth_gate,
                selector,
                credential_resolver,
                protocol_router,
            )
            .with_audit_sink(audit_sink),
            affinity_owner_recorder: None,
        }
    }

    /// Adds the router-owned affinity secret provider.
    #[must_use]
    pub fn with_affinity_secret_provider(
        mut self,
        affinity_secret_provider: &'a dyn HttpAffinitySecretProvider,
    ) -> Self {
        self.router = self
            .router
            .with_affinity_secret_provider(affinity_secret_provider);
        self
    }

    /// Adds the previous-response owner recorder.
    #[must_use]
    pub fn with_affinity_owner_recorder(
        mut self,
        affinity_owner_recorder: &'a dyn HttpAffinityOwnerRecorder,
    ) -> Self {
        self.affinity_owner_recorder = Some(affinity_owner_recorder);
        self
    }

    /// Handles one already-upgraded local WebSocket stream.
    pub async fn handle_upgraded_connection<LocalStream>(
        &self,
        mut local_websocket: WebSocketStream<LocalStream>,
        handshake: WebSocketHandshakeRequest,
        upstream_url: &str,
        max_upstream_messages: usize,
    ) -> Result<(), WebSocketTunnelError>
    where
        LocalStream: AsyncRead + AsyncWrite + Unpin,
    {
        let first_message =
            match tokio::time::timeout(Duration::from_millis(250), local_websocket.next()).await {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(error))) => return Err(WebSocketTunnelError::Transport(error)),
                Ok(None) => {
                    local_websocket.close(None).await?;
                    return Err(WebSocketTunnelError::CloseReason(
                        WebSocketCloseReason::FirstFrameTimeout,
                    ));
                }
                Err(_elapsed) => {
                    local_websocket.close(None).await?;
                    return Err(WebSocketTunnelError::CloseReason(
                        WebSocketCloseReason::FirstFrameTimeout,
                    ));
                }
            };
        let first_frame = frame_from_message(first_message);
        let decision = self
            .router
            .route_first_frame(handshake, first_frame)
            .await
            .map_err(WebSocketTunnelError::CloseReason)?;
        let WebSocketFirstFrameDecision::OpenUpstream {
            token_generation: _,
            headers,
            first_frame,
            affinity_owner_context,
        } = decision;

        let mut upstream_request = upstream_url.into_client_request()?;
        apply_upstream_headers(upstream_request.headers_mut(), &headers)?;
        let (mut upstream_websocket, _response) = connect_async(upstream_request).await?;
        upstream_websocket
            .send(message_from_frame(first_frame)?)
            .await?;

        forward_duplex_until_complete(
            &mut local_websocket,
            &mut upstream_websocket,
            max_upstream_messages,
            self.affinity_owner_recorder,
            affinity_owner_context.as_ref(),
        )
        .await
    }
}

async fn forward_duplex_until_complete<LocalStream, UpstreamStream>(
    local_websocket: &mut WebSocketStream<LocalStream>,
    upstream_websocket: &mut WebSocketStream<UpstreamStream>,
    max_upstream_messages: usize,
    affinity_owner_recorder: Option<&dyn HttpAffinityOwnerRecorder>,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) -> Result<(), WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin,
    UpstreamStream: AsyncRead + AsyncWrite + Unpin,
{
    if max_upstream_messages == 0 {
        local_websocket.close(None).await?;
        upstream_websocket.close(None).await?;
        return Ok(());
    }

    let mut upstream_message_count = 0_usize;
    loop {
        tokio::select! {
            local_message = local_websocket.next() => {
                let Some(local_message) = local_message else {
                    upstream_websocket.close(None).await?;
                    return Ok(());
                };
                let local_message = local_message?;
                let is_close = matches!(local_message, Message::Close(_));
                upstream_websocket.send(local_message).await?;
                if is_close {
                    return Ok(());
                }
            }
            upstream_message = upstream_websocket.next() => {
                let Some(upstream_message) = upstream_message else {
                    local_websocket.close(None).await?;
                    return Ok(());
                };
                let upstream_message = upstream_message?;
                let is_close = matches!(upstream_message, Message::Close(_));
                let is_completed = is_response_completed(&upstream_message);
                record_websocket_affinity_owner(
                    &upstream_message,
                    affinity_owner_recorder,
                    affinity_owner_context,
                );
                local_websocket.send(upstream_message).await?;
                upstream_message_count = upstream_message_count.saturating_add(1);
                if is_close || is_completed || upstream_message_count >= max_upstream_messages {
                    return Ok(());
                }
            }
        }
    }
}

fn forward_upstream_response(
    upstream_websocket: &mut WebSocket<impl std::io::Read + std::io::Write>,
    local_websocket: &mut WebSocket<impl std::io::Read + std::io::Write>,
    max_upstream_messages: usize,
    affinity_owner_recorder: Option<&dyn HttpAffinityOwnerRecorder>,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) -> Result<(), WebSocketTunnelError> {
    for _ in 0..max_upstream_messages {
        let upstream_message = upstream_websocket.read()?;
        let is_close = matches!(upstream_message, Message::Close(_));
        let is_completed = is_response_completed(&upstream_message);
        record_websocket_affinity_owner(
            &upstream_message,
            affinity_owner_recorder,
            affinity_owner_context,
        );
        local_websocket.send(upstream_message)?;
        if is_close || is_completed {
            return Ok(());
        }
    }
    local_websocket.close(None)?;
    upstream_websocket.close(None)?;

    Ok(())
}

fn record_websocket_affinity_owner(
    upstream_message: &Message,
    affinity_owner_recorder: Option<&dyn HttpAffinityOwnerRecorder>,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) {
    let (Some(recorder), Some(context)) = (affinity_owner_recorder, affinity_owner_context) else {
        return;
    };
    let Some(previous_response_id) = extract_websocket_response_id(upstream_message) else {
        return;
    };
    let Ok(affinity_key_hash) =
        hash_previous_response_id(&context.affinity_secret, &previous_response_id)
    else {
        return;
    };
    let owner = PreviousResponseAffinityOwnerRecord::new(
        affinity_key_hash,
        context.account_id.clone(),
        context.credential_generation,
        RouteBand::Responses,
        AffinitySourceTransport::WebSocket,
        current_unix_seconds(),
    );
    let _result = recorder.record_affinity_owner(&owner);
}

fn extract_websocket_response_id(message: &Message) -> Option<PreviousResponseId> {
    let Message::Text(text) = message else {
        return None;
    };
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let response_id = value
        .get("response")
        .and_then(serde_json::Value::as_object)
        .and_then(|response| response.get("id"))
        .and_then(serde_json::Value::as_str)?;
    if response_id.is_empty() {
        return None;
    }
    PreviousResponseId::new(response_id.to_owned()).ok()
}

fn is_response_completed(message: &Message) -> bool {
    let Message::Text(text) = message else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .as_deref()
        == Some("response.completed")
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[allow(clippy::result_large_err)]
fn accept_local_websocket<F>(
    local_stream: TcpStream,
    on_request: F,
) -> Result<WebSocket<TcpStream>, WebSocketTunnelError>
where
    F: FnOnce(&Request),
{
    accept_hdr(
        local_stream,
        move |request: &Request, response: Response| {
            on_request(request);
            Ok(response)
        },
    )
    .map_err(|_error| WebSocketTunnelError::Handshake)
}

fn handshake_from_request(request: &Request) -> WebSocketHandshakeRequest {
    request
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| Header::new(name.as_str(), value))
        })
        .fold(WebSocketHandshakeRequest::new(), |handshake, header| {
            handshake.with_header(header)
        })
}

fn take_captured_handshake(
    captured_handshake: &Mutex<Option<WebSocketHandshakeRequest>>,
) -> Result<WebSocketHandshakeRequest, WebSocketTunnelError> {
    let mut captured = captured_handshake
        .lock()
        .map_err(|_| WebSocketTunnelError::HandshakeCapture)?;
    captured
        .take()
        .ok_or(WebSocketTunnelError::HandshakeCapture)
}

fn frame_from_message(message: Message) -> WebSocketFrame {
    match message {
        Message::Text(value) => WebSocketFrame::Text(value.as_str().as_bytes().to_vec()),
        Message::Binary(value) => WebSocketFrame::Binary(value.to_vec()),
        _other => WebSocketFrame::Binary(Vec::new()),
    }
}

fn apply_upstream_headers(
    target: &mut tungstenite::http::HeaderMap,
    headers: &HeaderCollection,
) -> Result<(), WebSocketTunnelError> {
    for header in headers.as_slice() {
        let name = HeaderName::from_str(header.name()).map_err(|_| {
            WebSocketTunnelError::InvalidUpstreamHeader {
                name: header.name().to_owned(),
            }
        })?;
        let value = HeaderValue::from_str(header.value()).map_err(|_| {
            WebSocketTunnelError::InvalidUpstreamHeader {
                name: header.name().to_owned(),
            }
        })?;
        target.insert(name, value);
    }

    Ok(())
}

fn message_from_frame(frame: WebSocketFrame) -> Result<Message, WebSocketTunnelError> {
    match frame {
        WebSocketFrame::Text(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| WebSocketTunnelError::InvalidText)?;
            Ok(Message::text(text))
        }
        WebSocketFrame::Binary(bytes) => Ok(Message::binary(bytes)),
    }
}

/// Blocking WebSocket tunnel failure.
#[derive(Debug, Error)]
pub enum WebSocketTunnelError {
    /// Tungstenite failed.
    #[error("websocket transport failed: {0}")]
    Transport(#[from] tungstenite::Error),
    /// WebSocket handshake failed.
    #[error("websocket handshake failed")]
    Handshake,
    /// First-frame router closed locally before upstream open.
    #[error("websocket closed before upstream open: {0:?}")]
    CloseReason(WebSocketCloseReason),
    /// Handshake capture failed.
    #[error("websocket handshake capture failed")]
    HandshakeCapture,
    /// Active WebSocket connection registration failed.
    #[error("websocket connection tracking failed")]
    ConnectionTracking(#[source] std::io::Error),
    /// Sanitized upstream header was invalid.
    #[error("invalid sanitized upstream header: {name}")]
    InvalidUpstreamHeader {
        /// Header name.
        name: String,
    },
    /// Text frame was no longer valid UTF-8.
    #[error("websocket text frame was invalid utf-8")]
    InvalidText,
}
