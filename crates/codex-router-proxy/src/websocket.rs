//! WebSocket first-frame routing protocol.

use std::collections::HashMap;
#[cfg(test)]
use std::io::ErrorKind;
#[cfg(test)]
use std::net::Shutdown;
use std::net::SocketAddr;
#[cfg(test)]
use std::net::TcpStream;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(test)]
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_auth::resolver::ProviderCredentialResolver;
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
use futures_util::stream::SplitSink;
use futures_util::stream::SplitStream;
use thiserror::Error;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::Message;
#[cfg(test)]
use tokio_tungstenite::tungstenite::WebSocket;
#[cfg(test)]
use tokio_tungstenite::tungstenite::accept_hdr;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
#[cfg(test)]
use tokio_tungstenite::tungstenite::connect;
use tokio_tungstenite::tungstenite::error::ProtocolError;
#[cfg(test)]
use tokio_tungstenite::tungstenite::handshake::server::Request;
#[cfg(test)]
use tokio_tungstenite::tungstenite::handshake::server::Response;
use tokio_tungstenite::tungstenite::http::HeaderName;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::account_selection::AccountDecisionSelector;
use crate::account_selection::ActiveReservationGuard;
use crate::account_selection::AsyncAccountDecisionSelector;
use crate::account_selection::QuotaAwareAccountSelectorError;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::headers::sanitize_headers_for_upstream;
use crate::http_sse::AsyncHttpAffinityOwnerRecorder;
use crate::http_sse::AsyncProviderCredentialResolver;
use crate::http_sse::HttpAffinityOwnerRecorder;
use crate::http_sse::HttpAffinitySecretProvider;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
use crate::http_sse::StderrAuditFailureReporter;
use crate::http_sse::allowed_audit_event;
use crate::http_sse::append_audit_event_with_reporter;
use crate::http_sse::local_auth_rejection_audit_event;
use crate::http_sse::redacted_account_hash;
use crate::local_auth::ProxyLocalAuthGate;
use crate::local_auth::extract_presented_local_token_from_request;
use crate::provider_error::AsyncProviderErrorObserver;
use crate::provider_error::ProviderErrorClassification;
use crate::provider_error::classify_provider_error_envelope;

use crate::routes::Method;

const WEBSOCKET_METADATA_SCAN_LIMIT_BYTES: usize = 64 * 1024;
const WEBSOCKET_METADATA_SCAN_MAX_TOP_LEVEL_KEYS: usize = 64;
const CODEX_WEBSOCKET_RECONNECT_SIGNAL: &str = r#"{"type":"error","status":400,"error":{"type":"invalid_request_error","code":"websocket_connection_limit_reached","message":"Responses websocket connection limit reached (60 minutes). Create a new websocket connection to continue."}}"#;
const ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL: &str = r#"{"type":"error","status":429,"error":{"type":"codex_router_quota_exhausted","code":"codex_router_all_accounts_exhausted","message":"All configured codex-router accounts are out of usable quota."}}"#;
const ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL: &str = r#"{"type":"error","status":503,"error":{"type":"codex_router_quota_state_unavailable","code":"codex_router_quota_state_unavailable","message":"codex-router cannot safely rotate accounts because quota state is unavailable."}}"#;

/// WebSocket frame subset needed before upstream connection opens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketFrame {
    /// Text frame bytes.
    Text(Vec<u8>),
    /// Binary frame bytes.
    Binary(Vec<u8>),
}

impl WebSocketFrame {
    fn payload(&self) -> &[u8] {
        match self {
            Self::Text(payload) | Self::Binary(payload) => payload,
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
    active_reservation_guard: Option<ActiveReservationGuard>,
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
            active_reservation_guard: None,
        }
    }

    fn with_active_reservation_guard(
        mut self,
        active_reservation_guard: Option<ActiveReservationGuard>,
    ) -> Self {
        self.active_reservation_guard = active_reservation_guard;
        self
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
    Selection {
        /// Selection failure reason.
        reason: QuotaAwareAccountSelectorError,
    },
    /// Provider credential resolution failed before upstream open.
    ProviderCredential,
    /// First frame failed local auth-safety or routing metadata constraints.
    UnexpectedFirstFrame,
}

/// WebSocket first-frame router.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WebSocketProtocolRouter;

impl WebSocketProtocolRouter {
    /// Creates a WebSocket protocol router.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Checks router-owned first-frame auth invariants before upstream open.
    pub fn ensure_first_frame_allowed(
        &self,
        first_frame: &WebSocketFrame,
    ) -> Result<(), WebSocketCloseReason> {
        if let WebSocketFrame::Text(first_frame_bytes) = first_frame
            && has_forbidden_top_level_websocket_auth_carrier(first_frame_bytes)
        {
            return Err(WebSocketCloseReason::UnexpectedFirstFrame);
        }

        Ok(())
    }

    /// Routes the first frame, returning either sanitized upstream open data or a local close reason.
    pub fn route_first_frame(
        &self,
        handshake: WebSocketHandshakeRequest,
        first_frame: WebSocketFrame,
        provider_bearer_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> Result<WebSocketFirstFrameDecision, WebSocketCloseReason> {
        self.ensure_first_frame_allowed(&first_frame)?;

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
        self.protocol_router
            .ensure_first_frame_allowed(&first_frame)
            .inspect_err(|_reason| {
                self.emit_audit_event(websocket_first_frame_rejection_audit_event(None));
            })?;
        let selection_request = HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_websocket_upgrade(true)
            .with_body(first_frame.payload().to_vec());
        let affinity_secret = self.load_affinity_secret().map_err(|_reason| {
            self.emit_audit_event(websocket_selection_rejection_audit_event());
            WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            }
        })?;
        let selected = self
            .selector
            .select_upstream_account(&selection_request, token_generation, Some(&affinity_secret))
            .map_err(|error| {
                self.emit_audit_event(websocket_selection_rejection_audit_event());
                selection_close_reason_from_http_error(error)
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
                affinity_owner_context: Some(
                    WebSocketAffinityOwnerContext::new(
                        affinity_secret,
                        selected.account_id().clone(),
                        resolved.credential_generation(),
                    )
                    .with_active_reservation_guard(selected.active_reservation_guard().cloned()),
                ),
            },
        })
    }

    fn load_affinity_secret(&self) -> Result<RouterAffinityHashSecret, WebSocketCloseReason> {
        let provider = self
            .affinity_secret_provider
            .ok_or(WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            })?;
        provider.load_or_create_affinity_secret().map_err(|_error| {
            WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            }
        })
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
        self.protocol_router
            .ensure_first_frame_allowed(&first_frame)
            .inspect_err(|_reason| {
                self.emit_audit_event(websocket_first_frame_rejection_audit_event(None));
            })?;
        let selection_request = HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_websocket_upgrade(true)
            .with_body(first_frame.payload().to_vec());
        let affinity_secret = self.load_affinity_secret().map_err(|_reason| {
            self.emit_audit_event(websocket_selection_rejection_audit_event());
            WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            }
        })?;
        let selected = self
            .selector
            .select_upstream_account(&selection_request, token_generation, Some(&affinity_secret))
            .await
            .map_err(|error| {
                self.emit_audit_event(websocket_selection_rejection_audit_event());
                selection_close_reason_from_http_error(error)
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
                affinity_owner_context: Some(
                    WebSocketAffinityOwnerContext::new(
                        affinity_secret,
                        selected.account_id().clone(),
                        resolved.credential_generation(),
                    )
                    .with_active_reservation_guard(selected.active_reservation_guard().cloned()),
                ),
            },
        })
    }

    fn load_affinity_secret(&self) -> Result<RouterAffinityHashSecret, WebSocketCloseReason> {
        let provider = self
            .affinity_secret_provider
            .ok_or(WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            })?;
        provider.load_or_create_affinity_secret().map_err(|_error| {
            WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            }
        })
    }
}

fn selection_close_reason_from_http_error(error: HttpProxyError) -> WebSocketCloseReason {
    match error {
        HttpProxyError::Selection { reason } => WebSocketCloseReason::Selection { reason },
        _ => WebSocketCloseReason::Selection {
            reason: QuotaAwareAccountSelectorError::StateUnavailable,
        },
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
const MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS: usize = 1024;

/// Tracks active local WebSocket streams by local token generation.
#[derive(Clone, Debug, Default)]
pub struct WebSocketRevocationRegistry {
    #[cfg(test)]
    connections: Arc<Mutex<HashMap<TokenGeneration, Vec<TcpStream>>>>,
    cancellations: Arc<Mutex<HashMap<TokenGeneration, Vec<WebSocketCancellationEntry>>>>,
    stats: Arc<Mutex<WebSocketRegistryStats>>,
    registered_session_ids: Arc<Mutex<Vec<u64>>>,
    completed_session_ids: Arc<Mutex<Vec<u64>>>,
    closed_session_ids: Arc<Mutex<Vec<u64>>>,
    session_peer_addrs: Arc<Mutex<Vec<WebSocketSessionPeerAddr>>>,
    forwarded_upstream_messages_by_session: Arc<Mutex<HashMap<u64, usize>>>,
    completed_session_forwarded_upstream_message_counts: Arc<Mutex<Vec<usize>>>,
    final_session_forwarded_upstream_message_counts: Arc<Mutex<Vec<usize>>>,
}

#[derive(Clone, Debug)]
struct WebSocketCancellationEntry {
    session_id: u64,
    token: CancellationToken,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct WebSocketRegistryStats {
    next_session_id: u64,
    active_sessions: usize,
    high_water_sessions: usize,
    registered_sessions: usize,
    closed_sessions: usize,
    completed_response_sessions: usize,
    forwarded_upstream_messages: usize,
}

/// Redacted local peer socket observed for a WebSocket session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketSessionPeerAddr {
    /// Redacted numeric router-local session id.
    pub session_id: u64,
    /// Loopback client socket address as observed by the router.
    pub peer_addr: String,
}

/// Redacted snapshot of WebSocket session registry counters.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WebSocketRegistrySnapshot {
    /// Currently active async WebSocket sessions.
    pub active_sessions: usize,
    /// Highest active async WebSocket session count observed.
    pub high_water_sessions: usize,
    /// Total async WebSocket sessions registered since router start.
    pub registered_sessions: usize,
    /// Total async WebSocket sessions that have dropped their registry handle.
    pub closed_sessions: usize,
    /// Total response.completed events forwarded by async WebSocket sessions.
    pub completed_response_sessions: usize,
    /// Total upstream-to-local WebSocket messages written to local clients.
    pub forwarded_upstream_messages: usize,
    /// Redacted numeric session ids opened by the router registry.
    pub registered_session_ids: Vec<u64>,
    /// Redacted numeric session ids that forwarded response.completed.
    pub completed_session_ids: Vec<u64>,
    /// Redacted numeric session ids closed by the router registry.
    pub closed_session_ids: Vec<u64>,
    /// Redacted local peer socket addresses associated with opened sessions.
    pub session_peer_addrs: Vec<WebSocketSessionPeerAddr>,
    /// Upstream-to-local write counts captured when each response.completed event is observed.
    pub completed_session_forwarded_upstream_message_counts: Vec<usize>,
    /// Final upstream-to-local write counts captured once per closed async WebSocket session.
    pub final_session_forwarded_upstream_message_counts: Vec<usize>,
}

#[derive(Debug)]
struct WebSocketSessionRegistration {
    registry: WebSocketRevocationRegistry,
    generation: TokenGeneration,
    session_id: u64,
    cancellation: CancellationToken,
}

impl WebSocketRevocationRegistry {
    /// Creates an empty revocation registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
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

    #[cfg(test)]
    fn register_cancellation(&self, generation: TokenGeneration) -> WebSocketSessionRegistration {
        self.register_cancellation_with_peer_addr(generation, None)
    }

    fn register_cancellation_with_peer_addr(
        &self,
        generation: TokenGeneration,
        peer_addr: Option<SocketAddr>,
    ) -> WebSocketSessionRegistration {
        let cancellation = CancellationToken::new();
        let session_id = self.note_session_opened();
        if let Some(peer_addr) = peer_addr
            && let Ok(mut session_peer_addrs) = self.session_peer_addrs.lock()
        {
            push_bounded_peer_addr(
                &mut session_peer_addrs,
                WebSocketSessionPeerAddr {
                    session_id,
                    peer_addr: peer_addr.to_string(),
                },
            );
        }
        if let Ok(mut cancellations) = self.cancellations.lock() {
            cancellations
                .entry(generation)
                .or_default()
                .push(WebSocketCancellationEntry {
                    session_id,
                    token: cancellation.clone(),
                });
        }

        WebSocketSessionRegistration {
            registry: self.clone(),
            generation,
            session_id,
            cancellation,
        }
    }

    /// Returns redacted active-session registry counters.
    #[must_use]
    pub fn snapshot(&self) -> WebSocketRegistrySnapshot {
        self.stats.lock().map_or_else(
            |_| WebSocketRegistrySnapshot::default(),
            |stats| WebSocketRegistrySnapshot {
                active_sessions: stats.active_sessions,
                high_water_sessions: stats.high_water_sessions,
                registered_sessions: stats.registered_sessions,
                closed_sessions: stats.closed_sessions,
                completed_response_sessions: stats.completed_response_sessions,
                forwarded_upstream_messages: stats.forwarded_upstream_messages,
                registered_session_ids: self
                    .registered_session_ids
                    .lock()
                    .map_or_else(|_| Vec::new(), |ids| ids.clone()),
                completed_session_ids: self
                    .completed_session_ids
                    .lock()
                    .map_or_else(|_| Vec::new(), |ids| ids.clone()),
                closed_session_ids: self
                    .closed_session_ids
                    .lock()
                    .map_or_else(|_| Vec::new(), |ids| ids.clone()),
                session_peer_addrs: self
                    .session_peer_addrs
                    .lock()
                    .map_or_else(|_| Vec::new(), |peers| peers.clone()),
                completed_session_forwarded_upstream_message_counts: self
                    .completed_session_forwarded_upstream_message_counts
                    .lock()
                    .map_or_else(|_| Vec::new(), |counts| counts.clone()),
                final_session_forwarded_upstream_message_counts: self
                    .final_session_forwarded_upstream_message_counts
                    .lock()
                    .map_or_else(|_| Vec::new(), |counts| counts.clone()),
            },
        )
    }

    fn note_session_opened(&self) -> u64 {
        let Ok(mut stats) = self.stats.lock() else {
            return 0;
        };
        stats.next_session_id = stats.next_session_id.saturating_add(1);
        stats.active_sessions = stats.active_sessions.saturating_add(1);
        stats.registered_sessions = stats.registered_sessions.saturating_add(1);
        stats.high_water_sessions = stats.high_water_sessions.max(stats.active_sessions);
        if let Ok(mut registered_session_ids) = self.registered_session_ids.lock() {
            push_bounded_u64(&mut registered_session_ids, stats.next_session_id);
        }
        stats.next_session_id
    }

    fn note_session_closed(&self, generation: TokenGeneration, session_id: u64) {
        if let Ok(mut cancellations) = self.cancellations.lock()
            && let Some(entries) = cancellations.get_mut(&generation)
        {
            entries.retain(|entry| entry.session_id != session_id);
            if entries.is_empty() {
                cancellations.remove(&generation);
            }
        }
        if let Ok(mut stats) = self.stats.lock() {
            stats.active_sessions = stats.active_sessions.saturating_sub(1);
            stats.closed_sessions = stats.closed_sessions.saturating_add(1);
        }
        if let Ok(mut closed_session_ids) = self.closed_session_ids.lock() {
            push_bounded_u64(&mut closed_session_ids, session_id);
        }
        let forwarded_count = self
            .forwarded_upstream_messages_by_session
            .lock()
            .ok()
            .and_then(|forwarded_by_session| forwarded_by_session.get(&session_id).copied())
            .unwrap_or_default();
        if let Ok(mut counts) = self.final_session_forwarded_upstream_message_counts.lock() {
            push_bounded_count(&mut counts, forwarded_count);
        }
        if let Ok(mut forwarded_by_session) = self.forwarded_upstream_messages_by_session.lock() {
            forwarded_by_session.remove(&session_id);
        }
    }

    fn note_upstream_message_forwarded(&self, session_id: u64) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.forwarded_upstream_messages = stats.forwarded_upstream_messages.saturating_add(1);
        }
        if let Ok(mut forwarded_by_session) = self.forwarded_upstream_messages_by_session.lock() {
            let count = forwarded_by_session.entry(session_id).or_default();
            *count = count.saturating_add(1);
        }
    }

    fn note_response_completed(&self, session_id: u64) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.completed_response_sessions = stats.completed_response_sessions.saturating_add(1);
        }
        if let Ok(mut completed_session_ids) = self.completed_session_ids.lock() {
            push_bounded_u64(&mut completed_session_ids, session_id);
        }
        let forwarded_count = self
            .forwarded_upstream_messages_by_session
            .lock()
            .ok()
            .and_then(|forwarded_by_session| forwarded_by_session.get(&session_id).copied())
            .unwrap_or_default();
        if let Ok(mut counts) = self
            .completed_session_forwarded_upstream_message_counts
            .lock()
        {
            push_bounded_count(&mut counts, forwarded_count);
        }
    }

    /// Closes connections that authenticated with generations other than the active one.
    pub fn close_all_except(&self, active_generation: TokenGeneration) {
        #[cfg(test)]
        {
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
        let Ok(mut cancellations) = self.cancellations.lock() else {
            return;
        };
        let stale_generations = cancellations
            .keys()
            .copied()
            .filter(|generation| *generation != active_generation)
            .collect::<Vec<_>>();
        for stale_generation in stale_generations {
            if let Some(entries) = cancellations.remove(&stale_generation) {
                for entry in entries {
                    entry.token.cancel();
                }
            }
        }
    }
}

fn push_bounded_count(counts: &mut Vec<usize>, count: usize) {
    counts.push(count);
    if counts.len() > MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS {
        let excess = counts.len() - MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS;
        counts.drain(0..excess);
    }
}

fn push_bounded_u64(values: &mut Vec<u64>, value: u64) {
    values.push(value);
    if values.len() > MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS {
        let excess = values.len() - MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS;
        values.drain(0..excess);
    }
}

fn push_bounded_peer_addr(
    values: &mut Vec<WebSocketSessionPeerAddr>,
    value: WebSocketSessionPeerAddr,
) {
    values.push(value);
    if values.len() > MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS {
        let excess = values.len() - MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS;
        values.drain(0..excess);
    }
}

fn is_reset_without_closing_handshake(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)
    )
}

impl WebSocketSessionRegistration {
    fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    #[cfg(test)]
    fn note_response_completed(&self) {
        self.registry.note_response_completed(self.session_id);
    }
}

impl Drop for WebSocketSessionRegistration {
    fn drop(&mut self) {
        self.registry
            .note_session_closed(self.generation, self.session_id);
    }
}

#[cfg(test)]
mod registry_tests {
    use super::TokenGeneration;
    use super::WebSocketRevocationRegistry;

    #[test]
    fn registry_snapshot_tracks_active_high_water_and_cleanup() {
        let registry = WebSocketRevocationRegistry::new();

        let first_session = registry.register_cancellation(TokenGeneration::new(1));
        let second_session = registry.register_cancellation(TokenGeneration::new(1));
        assert_eq!(registry.snapshot().active_sessions, 2);
        assert_eq!(registry.snapshot().high_water_sessions, 2);
        assert_eq!(registry.snapshot().registered_sessions, 2);
        assert_eq!(registry.snapshot().closed_sessions, 0);
        assert_eq!(registry.snapshot().completed_response_sessions, 0);

        second_session.note_response_completed();
        assert_eq!(registry.snapshot().completed_response_sessions, 1);

        drop(first_session);
        assert_eq!(registry.snapshot().active_sessions, 1);
        assert_eq!(registry.snapshot().high_water_sessions, 2);
        assert_eq!(registry.snapshot().closed_sessions, 1);

        drop(second_session);
        assert_eq!(registry.snapshot().active_sessions, 0);
        assert_eq!(registry.snapshot().high_water_sessions, 2);
        assert_eq!(registry.snapshot().registered_sessions, 2);
        assert_eq!(registry.snapshot().closed_sessions, 2);
    }

    #[test]
    fn registry_revokes_only_stale_generation_cancellations() {
        let registry = WebSocketRevocationRegistry::new();
        let stale_session = registry.register_cancellation(TokenGeneration::new(1));
        let active_session = registry.register_cancellation(TokenGeneration::new(2));

        registry.close_all_except(TokenGeneration::new(2));

        assert!(stale_session.cancellation().is_cancelled());
        assert!(!active_session.cancellation().is_cancelled());
    }
}

#[cfg(test)]
mod async_forwarding_tests {
    use super::ActiveTurnReservationState;
    use super::AsyncWebSocketTunnel;
    use super::TokenGeneration;
    use super::WebSocketAffinityOwnerContext;
    use super::WebSocketForwardingContext;
    use super::WebSocketHandshakeRequest;
    use super::WebSocketProtocolRouter;
    use super::WebSocketRevocationRegistry;
    use super::forward_duplex_until_complete;
    use super::is_response_completed;
    use super::is_response_create;
    use super::record_forwarded_websocket_metadata;
    use super::websocket_affinity_owner_record;
    use bytes::Bytes;
    use codex_router_auth::resolver::CredentialResolverError;
    use codex_router_auth::resolver::ResolvedProviderCredential;
    use codex_router_core::affinity::RouterAffinityHashSecret;
    use codex_router_core::ids::AccountId;
    use codex_router_core::ids::TokenGeneration as LocalTokenGeneration;
    use codex_router_core::redaction::SecretString;
    use futures_util::SinkExt;
    use futures_util::StreamExt;
    use futures_util::future::BoxFuture;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::io::duplex;
    use tokio::net::TcpListener;
    use tokio::sync::Notify;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::protocol::Role;
    use tokio_util::sync::CancellationToken;
    use tokio_util::task::TaskTracker;

    use crate::account_selection::ActiveReservationGuard;
    use crate::account_selection::AsyncAccountDecisionSelector;
    use crate::account_selection::QuotaAwareAccountSelectorError;
    use crate::account_selection::RouteBandReservationBooks;
    use crate::account_selection::SelectedAccountDecision;
    use crate::http_sse::AsyncHttpAffinityOwnerRecorder;
    use crate::http_sse::AsyncProviderCredentialResolver;
    use crate::http_sse::HttpAffinitySecretProvider;
    use crate::http_sse::HttpProxyError;
    use crate::http_sse::HttpProxyRequest;
    use crate::local_auth::ProxyLocalAuthGate;
    use crate::provider_error::AsyncProviderErrorObserver;
    use crate::provider_error::ProviderErrorObservationError;
    use codex_router_selection::reservation::ReservationBook;
    use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedProviderError {
        account_id: AccountId,
        route_band: codex_router_core::routes::RouteBand,
        body: Vec<u8>,
    }

    #[derive(Clone, Debug, Default)]
    struct RecordingAsyncProviderErrorObserver {
        records: Arc<Mutex<Vec<RecordedProviderError>>>,
        observed: Arc<Notify>,
    }

    impl RecordingAsyncProviderErrorObserver {
        fn records(&self) -> Vec<RecordedProviderError> {
            self.records.lock().map_or_else(
                |error| panic!("records lock should be available: {error}"),
                |records| records.clone(),
            )
        }
    }

    impl AsyncProviderErrorObserver for RecordingAsyncProviderErrorObserver {
        fn observe_provider_error<'a>(
            &'a self,
            account_id: AccountId,
            route_band: codex_router_core::routes::RouteBand,
            body: Vec<u8>,
            _observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
            Box::pin(async move {
                self.records
                    .lock()
                    .unwrap_or_else(|error| panic!("records lock should be available: {error}"))
                    .push(RecordedProviderError {
                        account_id,
                        route_band,
                        body,
                    });
                self.observed.notify_one();
                Ok(())
            })
        }
    }

    #[derive(Clone, Debug, Default)]
    struct FailingAsyncProviderErrorObserver;

    impl AsyncProviderErrorObserver for FailingAsyncProviderErrorObserver {
        fn observe_provider_error<'a>(
            &'a self,
            _account_id: AccountId,
            _route_band: codex_router_core::routes::RouteBand,
            _body: Vec<u8>,
            _observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
            Box::pin(async {
                Err(ProviderErrorObservationError::State(
                    codex_router_state::sqlite::StateStoreError::UnsupportedSchemaVersion {
                        version: 0,
                    },
                ))
            })
        }
    }

    #[derive(Debug)]
    struct BlockingAsyncAffinityOwnerRecorder {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl BlockingAsyncAffinityOwnerRecorder {
        fn new(entered: Arc<Notify>, release: Arc<Notify>) -> Self {
            Self { entered, release }
        }
    }

    impl AsyncHttpAffinityOwnerRecorder for BlockingAsyncAffinityOwnerRecorder {
        fn record_affinity_owner<'a>(
            &'a self,
            _owner: PreviousResponseAffinityOwnerRecord,
        ) -> BoxFuture<'a, Result<(), HttpProxyError>> {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(())
            })
        }
    }

    #[test]
    fn response_create_detection_is_bounded_top_level_metadata_only() {
        assert!(is_response_create(&Message::text(
            r#"{"type":"response.create","turn":1}"#,
        )));
        assert!(!is_response_create(&Message::text(
            r#"{"input":[{"content":"{\"type\":\"response.create\"}"}]}"#,
        )));

        let mut late_type = br#"{"input":""#.to_vec();
        late_type.extend(std::iter::repeat_n(b'x', 70 * 1024));
        late_type.extend(br#"","type":"response.create"}"#);
        let late_type = String::from_utf8(late_type)
            .unwrap_or_else(|error| panic!("test text should be utf-8: {error}"));
        assert!(!is_response_create(&Message::text(late_type)));
    }

    #[tokio::test]
    async fn completion_releases_before_blocked_affinity_recording() {
        let selected_account = AccountId::new("acct_selected")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
        let active_reservations = RouteBandReservationBooks::default();
        let reservation_handle = {
            let mut reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .entry("responses".to_owned())
                .or_insert_with(ReservationBook::default)
                .reserve_next_at(selected_account.clone(), 8, 1)
        };
        let active_reservation_guard = ActiveReservationGuard::new(
            active_reservations.clone(),
            "responses".to_owned(),
            reservation_handle,
        );
        let active_turn_reservation =
            ActiveTurnReservationState::new(Some(active_reservation_guard));
        let active_pressure = || {
            let reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .get("responses")
                .map_or(0, |book| book.active_load_pressure(&selected_account))
        };
        assert_eq!(active_pressure(), 8);

        let affinity_secret = RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
        let affinity_owner_context = WebSocketAffinityOwnerContext {
            affinity_secret,
            account_id: selected_account.clone(),
            credential_generation: 1,
            active_reservation_guard: None,
        };
        let recorder_entered = Arc::new(Notify::new());
        let recorder_release = Arc::new(Notify::new());
        let recorder = Arc::new(BlockingAsyncAffinityOwnerRecorder::new(
            Arc::clone(&recorder_entered),
            Arc::clone(&recorder_release),
        ));
        let registry = WebSocketRevocationRegistry::new();
        let session = registry.register_cancellation(TokenGeneration::new(1));
        let session_id = session.session_id;
        let active_turn_reservation_for_task = active_turn_reservation.clone();

        let record_task = tokio::spawn(async move {
            record_forwarded_websocket_metadata(
                tokio_tungstenite::tungstenite::Utf8Bytes::from_static(
                    r#"{"type":"response.completed","response":{"id":"resp_1"}}"#,
                ),
                registry,
                session_id,
                Some(&affinity_owner_context),
                active_turn_reservation_for_task,
                Some(recorder),
                None,
            )
            .await;
        });

        tokio::time::timeout(Duration::from_secs(1), recorder_entered.notified())
            .await
            .unwrap_or_else(|_elapsed| panic!("recorder should be entered"));
        assert_eq!(
            active_pressure(),
            0,
            "completion must release active pressure before affinity persistence can block"
        );

        active_turn_reservation.reserve_if_idle(2);
        assert_eq!(
            active_pressure(),
            8,
            "a new same-socket turn must be able to reserve while affinity persistence is blocked"
        );
        recorder_release.notify_one();
        record_task
            .await
            .unwrap_or_else(|error| panic!("record task should finish: {error}"));
        assert_eq!(active_pressure(), 8);
    }

    #[derive(Clone, Debug)]
    struct PendingAsyncSelector {
        started: Arc<Notify>,
    }

    impl AsyncAccountDecisionSelector for PendingAsyncSelector {
        fn select_upstream_account<'a>(
            &'a self,
            _request: &'a HttpProxyRequest,
            _token_generation: LocalTokenGeneration,
            _affinity_secret: Option<&'a RouterAffinityHashSecret>,
        ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
            Box::pin(async move {
                self.started.notify_one();
                std::future::pending().await
            })
        }
    }

    #[derive(Clone, Debug)]
    struct FixedAsyncSelector {
        account_id: AccountId,
    }

    impl AsyncAccountDecisionSelector for FixedAsyncSelector {
        fn select_upstream_account<'a>(
            &'a self,
            _request: &'a HttpProxyRequest,
            _token_generation: LocalTokenGeneration,
            _affinity_secret: Option<&'a RouterAffinityHashSecret>,
        ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
            Box::pin(async move {
                Ok(SelectedAccountDecision::new(
                    self.account_id.clone(),
                    "test-fixed",
                ))
            })
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct RejectingAsyncSelector {
        reason: QuotaAwareAccountSelectorError,
    }

    impl AsyncAccountDecisionSelector for RejectingAsyncSelector {
        fn select_upstream_account<'a>(
            &'a self,
            _request: &'a HttpProxyRequest,
            _token_generation: LocalTokenGeneration,
            _affinity_secret: Option<&'a RouterAffinityHashSecret>,
        ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
            Box::pin(async move {
                Err(HttpProxyError::Selection {
                    reason: self.reason,
                })
            })
        }
    }

    #[derive(Clone, Debug)]
    struct FixedAsyncCredentialResolver {
        account_id: AccountId,
    }

    impl AsyncProviderCredentialResolver for FixedAsyncCredentialResolver {
        fn resolve_provider_credentials<'a>(
            &'a self,
            _account_id: &'a AccountId,
        ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
            Box::pin(async move {
                Ok(ResolvedProviderCredential::new(
                    self.account_id.clone(),
                    SecretString::new("test-access-token"),
                    1,
                ))
            })
        }
    }

    #[derive(Clone, Debug)]
    struct FixedAffinitySecretProvider {
        secret: RouterAffinityHashSecret,
    }

    impl FixedAffinitySecretProvider {
        fn new() -> Self {
            Self {
                secret: RouterAffinityHashSecret::new(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                )
                .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}")),
            }
        }
    }

    impl HttpAffinitySecretProvider for FixedAffinitySecretProvider {
        fn load_or_create_affinity_secret(
            &self,
        ) -> Result<RouterAffinityHashSecret, HttpProxyError> {
            Ok(self.secret.clone())
        }
    }

    #[tokio::test]
    async fn runtime_shutdown_cancels_pending_first_frame_routing() {
        let account_id = AccountId::new("acct_ws_test")
            .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
        let selector_started = Arc::new(Notify::new());
        let selector = PendingAsyncSelector {
            started: Arc::clone(&selector_started),
        };
        let credential_resolver = FixedAsyncCredentialResolver { account_id };
        let auth_gate = ProxyLocalAuthGate::disabled();
        let affinity_secret_provider = FixedAffinitySecretProvider::new();
        let protocol_router = WebSocketProtocolRouter::new();
        let registry = WebSocketRevocationRegistry::new();
        let session_shutdown = CancellationToken::new();
        let tunnel = AsyncWebSocketTunnel::new(
            &auth_gate,
            &selector,
            &credential_resolver,
            &protocol_router,
        )
        .with_revocation_registry(registry.clone())
        .with_session_shutdown(session_shutdown.clone())
        .with_affinity_secret_provider(&affinity_secret_provider);
        let (router_local_stream, client_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let router_future = tunnel.handle_upgraded_connection(
            router_local_websocket,
            WebSocketHandshakeRequest::new(),
            "ws://127.0.0.1:1/v1/responses",
        );
        let peer_future = async {
            client_websocket
                .send(Message::text(r#"{"type":"response.create"}"#))
                .await
                .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
            tokio::time::timeout(Duration::from_secs(1), selector_started.notified())
                .await
                .unwrap_or_else(|_elapsed| panic!("selector should start before shutdown"));
            session_shutdown.cancel();
        };
        let (router_result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(router_future, peer_future)
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("router should exit while routing is pending"));

        assert!(
            router_result.is_ok(),
            "shutdown should close pending first-frame routing cleanly, got {router_result:?}"
        );
        assert_eq!(registry.snapshot().active_sessions, 0);
    }

    #[tokio::test]
    async fn all_accounts_exhausted_sends_scrubbed_router_error_before_upstream_connect() {
        let account_id = AccountId::new("acct_ws_test")
            .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
        let selector = RejectingAsyncSelector {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        };
        let credential_resolver = FixedAsyncCredentialResolver { account_id };
        let auth_gate = ProxyLocalAuthGate::disabled();
        let affinity_secret_provider = FixedAffinitySecretProvider::new();
        let protocol_router = WebSocketProtocolRouter::new();
        let tunnel = AsyncWebSocketTunnel::new(
            &auth_gate,
            &selector,
            &credential_resolver,
            &protocol_router,
        )
        .with_affinity_secret_provider(&affinity_secret_provider);
        let (router_local_stream, client_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let router_future = tunnel.handle_upgraded_connection(
            router_local_websocket,
            WebSocketHandshakeRequest::new(),
            "ws://127.0.0.1:1/v1/responses",
        );
        let peer_future = async {
            client_websocket
                .send(Message::text(r#"{"type":"response.create"}"#))
                .await
                .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
            let message = match client_websocket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => panic!("client should receive router exhausted error: {error}"),
                None => panic!("client should receive router exhausted error"),
            };
            let rendered = message.to_string();
            assert_eq!(rendered, super::ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL);
            assert!(!rendered.contains("acct_"));
            assert!(!rendered.contains("usage_limit_reached"));
        };

        let (router_result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(router_future, peer_future)
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("router should return exhausted error promptly"));

        assert!(
            router_result.is_ok(),
            "all-accounts exhausted should be a clean router-level WebSocket error, got {router_result:?}"
        );
    }

    #[test]
    fn websocket_metadata_scan_ignores_oversized_late_affinity_and_completion_fields() {
        let affinity_secret = RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
        let account_id = AccountId::new("acct_selected")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
        let context = WebSocketAffinityOwnerContext {
            affinity_secret,
            account_id,
            credential_generation: 7,
            active_reservation_guard: None,
        };
        let oversized_padding = "a".repeat(65 * 1024);
        let message = Message::text(format!(
            r#"{{"padding":"{oversized_padding}","type":"response.completed","response":{{"id":"resp_late"}}}}"#
        ));

        assert!(
            websocket_affinity_owner_record(&message, Some(&context)).is_none(),
            "late oversized affinity metadata should be forwarded but not classified"
        );
        assert!(
            !is_response_completed(&message),
            "late oversized completion metadata should be forwarded but not classified"
        );
    }

    #[tokio::test]
    async fn runtime_shutdown_cancels_pending_upstream_connect() {
        let account_id = AccountId::new("acct_ws_test")
            .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
        let selector = FixedAsyncSelector {
            account_id: account_id.clone(),
        };
        let credential_resolver = FixedAsyncCredentialResolver { account_id };
        let auth_gate = ProxyLocalAuthGate::disabled();
        let affinity_secret_provider = FixedAffinitySecretProvider::new();
        let protocol_router = WebSocketProtocolRouter::new();
        let registry = WebSocketRevocationRegistry::new();
        let session_shutdown = CancellationToken::new();
        let tunnel = AsyncWebSocketTunnel::new(
            &auth_gate,
            &selector,
            &credential_resolver,
            &protocol_router,
        )
        .with_revocation_registry(registry.clone())
        .with_session_shutdown(session_shutdown.clone())
        .with_affinity_secret_provider(&affinity_secret_provider);
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap_or_else(|error| panic!("blackhole listener should bind: {error}"));
        let upstream_url = format!(
            "ws://{}/v1/responses",
            listener
                .local_addr()
                .unwrap_or_else(|error| panic!("local addr should read: {error}"))
        );
        let accepted = Arc::new(Notify::new());
        let accepted_for_task = Arc::clone(&accepted);
        let accept_task = tokio::spawn(async move {
            let (_stream, _addr) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("blackhole should accept: {error}"));
            accepted_for_task.notify_one();
            std::future::pending::<()>().await;
        });
        let (router_local_stream, client_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let router_future = tunnel.handle_upgraded_connection(
            router_local_websocket,
            WebSocketHandshakeRequest::new(),
            &upstream_url,
        );
        let peer_future = async {
            client_websocket
                .send(Message::text(r#"{"type":"response.create"}"#))
                .await
                .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
            tokio::time::timeout(Duration::from_secs(1), accepted.notified())
                .await
                .unwrap_or_else(|_elapsed| panic!("upstream connection should be accepted"));
            assert_eq!(registry.snapshot().active_sessions, 1);
            session_shutdown.cancel();
        };
        let (router_result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(router_future, peer_future)
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("router should exit while connect is pending"));
        accept_task.abort();

        assert!(
            router_result.is_ok(),
            "shutdown should close pending upstream connect cleanly, got {router_result:?}"
        );
        assert_eq!(registry.snapshot().active_sessions, 0);
    }

    #[tokio::test]
    async fn reset_during_new_turn_after_prior_completion_is_clean_transport_close() {
        let (router_local_stream, client_stream) = duplex(4096);
        let (router_upstream_stream, upstream_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let router_upstream_websocket =
            WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let mut upstream_websocket =
            WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
        let registry = WebSocketRevocationRegistry::new();
        let session = registry.register_cancellation(TokenGeneration::new(1));
        let revocation = session.cancellation().clone();
        let session_shutdown = CancellationToken::new();

        let router_task = async {
            forward_duplex_until_complete(
                router_local_websocket,
                router_upstream_websocket,
                WebSocketForwardingContext {
                    session_registration: session,
                    affinity_owner_recorder: None,
                    async_affinity_owner_recorder: None,
                    affinity_record_tasks: TaskTracker::new(),
                    affinity_owner_context: None,
                    provider_error_observer: None,
                    revocation: &revocation,
                    session_shutdown: &session_shutdown,
                },
            )
            .await
        };
        let peer_task = async {
            client_websocket
                .send(Message::text(r#"{"type":"response.create","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
            let first_upstream_frame = match upstream_websocket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => panic!("first upstream frame should be valid: {error}"),
                None => panic!("upstream should receive first frame"),
            };
            assert_eq!(
                first_upstream_frame.to_string(),
                r#"{"type":"response.create","turn":1}"#
            );
            upstream_websocket
                .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("first completion should send: {error}"));
            let first_client_response = match client_websocket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => panic!("first client response should be valid: {error}"),
                None => panic!("client should receive first completion"),
            };
            assert_eq!(
                first_client_response.to_string(),
                r#"{"type":"response.completed","turn":1}"#
            );

            client_websocket
                .send(Message::text(
                    r#"{"type":"conversation.item.create","turn":2}"#,
                ))
                .await
                .unwrap_or_else(|error| panic!("second local frame should send: {error}"));
            let second_upstream_frame = match upstream_websocket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => panic!("second upstream frame should be valid: {error}"),
                None => panic!("upstream should receive second frame"),
            };
            assert_eq!(
                second_upstream_frame.to_string(),
                r#"{"type":"conversation.item.create","turn":2}"#
            );
            drop(upstream_websocket);
        };

        let (router_result, ()) = tokio::join!(router_task, peer_task);
        assert!(
            router_result.is_ok(),
            "router should not invent per-turn reset semantics, got {router_result:?}"
        );
    }

    #[tokio::test]
    async fn reset_after_idle_control_frame_remains_clean_after_completion() {
        let (router_local_stream, client_stream) = duplex(4096);
        let (router_upstream_stream, upstream_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let router_upstream_websocket =
            WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let mut upstream_websocket =
            WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
        let registry = WebSocketRevocationRegistry::new();
        let session = registry.register_cancellation(TokenGeneration::new(1));
        let revocation = session.cancellation().clone();
        let session_shutdown = CancellationToken::new();

        let router_task = async {
            forward_duplex_until_complete(
                router_local_websocket,
                router_upstream_websocket,
                WebSocketForwardingContext {
                    session_registration: session,
                    affinity_owner_recorder: None,
                    async_affinity_owner_recorder: None,
                    affinity_record_tasks: TaskTracker::new(),
                    affinity_owner_context: None,
                    provider_error_observer: None,
                    revocation: &revocation,
                    session_shutdown: &session_shutdown,
                },
            )
            .await
        };
        let peer_task = async {
            client_websocket
                .send(Message::text(r#"{"type":"response.create","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("local frame should send: {error}"));
            match upstream_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
                None => panic!("upstream should receive first frame"),
            }
            upstream_websocket
                .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("completion should send: {error}"));
            match client_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("client should receive completion: {error}"),
                None => panic!("client should receive completion"),
            }

            client_websocket
                .send(Message::Ping(Bytes::from_static(b"idle")))
                .await
                .unwrap_or_else(|error| panic!("idle ping should send: {error}"));
            match upstream_websocket.next().await {
                Some(Ok(Message::Ping(_))) => {}
                Some(Ok(message)) => panic!("upstream should receive ping, got {message:?}"),
                Some(Err(error)) => panic!("upstream ping should be valid: {error}"),
                None => panic!("upstream should receive idle ping"),
            }
            drop(upstream_websocket);
        };

        let (router_result, ()) = tokio::join!(router_task, peer_task);
        assert!(
            router_result.is_ok(),
            "idle control frame after completion must not make reset look like a failed turn, got {router_result:?}"
        );
    }

    #[tokio::test]
    async fn response_completed_releases_active_reservation_before_socket_closes() {
        let (router_local_stream, client_stream) = duplex(4096);
        let (router_upstream_stream, upstream_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let router_upstream_websocket =
            WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let mut upstream_websocket =
            WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
        let registry = WebSocketRevocationRegistry::new();
        let session = registry.register_cancellation(TokenGeneration::new(1));
        let revocation = session.cancellation().clone();
        let session_shutdown = CancellationToken::new();
        let selected_account = AccountId::new("acct_selected")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
        let active_reservations = RouteBandReservationBooks::default();
        let reservation_handle = {
            let mut reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .entry("responses".to_owned())
                .or_insert_with(ReservationBook::default)
                .reserve_next_at(selected_account.clone(), 8, 1)
        };
        let active_reservation_guard = ActiveReservationGuard::new_with_active_client_leases(
            active_reservations.clone(),
            "responses".to_owned(),
            reservation_handle,
            None,
        );
        let affinity_secret = RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
        let affinity_owner_context = WebSocketAffinityOwnerContext {
            affinity_secret,
            account_id: selected_account.clone(),
            credential_generation: 1,
            active_reservation_guard: Some(active_reservation_guard),
        };

        let active_pressure = || {
            let reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .get("responses")
                .map_or(0, |book| book.active_load_pressure(&selected_account))
        };
        assert_eq!(active_pressure(), 8);

        let router_task = async {
            forward_duplex_until_complete(
                router_local_websocket,
                router_upstream_websocket,
                WebSocketForwardingContext {
                    session_registration: session,
                    affinity_owner_recorder: None,
                    async_affinity_owner_recorder: None,
                    affinity_record_tasks: TaskTracker::new(),
                    affinity_owner_context: Some(&affinity_owner_context),
                    provider_error_observer: None,
                    revocation: &revocation,
                    session_shutdown: &session_shutdown,
                },
            )
            .await
        };
        let peer_task = async {
            client_websocket
                .send(Message::text(r#"{"type":"response.create","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("local frame should send: {error}"));
            match upstream_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
                None => panic!("upstream should receive first frame"),
            }
            upstream_websocket
                .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("completion should send: {error}"));
            match client_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("client should receive completion: {error}"),
                None => panic!("client should receive completion"),
            }

            tokio::time::timeout(Duration::from_secs(1), async {
                while active_pressure() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_elapsed| panic!("response.completed should release active load"));

            client_websocket
                .send(Message::Ping(Bytes::from_static(b"still-open")))
                .await
                .unwrap_or_else(|error| {
                    panic!("socket should remain open after completion: {error}")
                });
            match upstream_websocket.next().await {
                Some(Ok(Message::Ping(_))) => {}
                Some(Ok(message)) => {
                    panic!("upstream should receive post-completion ping, got {message:?}")
                }
                Some(Err(error)) => panic!("post-completion ping should be valid: {error}"),
                None => panic!("upstream should receive post-completion ping"),
            }
            drop(upstream_websocket);
        };

        let (router_result, ()) = tokio::join!(router_task, peer_task);
        assert!(
            router_result.is_ok(),
            "completion release should not close the websocket by itself, got {router_result:?}"
        );
        assert_eq!(active_pressure(), 0);
    }

    #[tokio::test]
    async fn same_socket_request_after_completion_reserves_pinned_account_again() {
        let (router_local_stream, client_stream) = duplex(4096);
        let (router_upstream_stream, upstream_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let router_upstream_websocket =
            WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let mut upstream_websocket =
            WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
        let registry = WebSocketRevocationRegistry::new();
        let session = registry.register_cancellation(TokenGeneration::new(1));
        let revocation = session.cancellation().clone();
        let session_shutdown = CancellationToken::new();
        let selected_account = AccountId::new("acct_selected")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
        let active_reservations = RouteBandReservationBooks::default();
        let reservation_handle = {
            let mut reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .entry("responses".to_owned())
                .or_insert_with(ReservationBook::default)
                .reserve_next_at(selected_account.clone(), 8, 1)
        };
        let active_reservation_guard = ActiveReservationGuard::new(
            active_reservations.clone(),
            "responses".to_owned(),
            reservation_handle,
        );
        let affinity_secret = RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
        let affinity_owner_context = WebSocketAffinityOwnerContext {
            affinity_secret,
            account_id: selected_account.clone(),
            credential_generation: 1,
            active_reservation_guard: Some(active_reservation_guard),
        };
        let active_pressure = || {
            let reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .get("responses")
                .map_or(0, |book| book.active_load_pressure(&selected_account))
        };

        let router_task = async {
            forward_duplex_until_complete(
                router_local_websocket,
                router_upstream_websocket,
                WebSocketForwardingContext {
                    session_registration: session,
                    affinity_owner_recorder: None,
                    async_affinity_owner_recorder: None,
                    affinity_record_tasks: TaskTracker::new(),
                    affinity_owner_context: Some(&affinity_owner_context),
                    provider_error_observer: None,
                    revocation: &revocation,
                    session_shutdown: &session_shutdown,
                },
            )
            .await
        };
        let peer_task = async {
            assert_eq!(active_pressure(), 8);
            client_websocket
                .send(Message::text(r#"{"type":"response.create","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("first request should send: {error}"));
            match upstream_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("upstream should receive first request: {error}"),
                None => panic!("upstream should receive first request"),
            }
            upstream_websocket
                .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("first completion should send: {error}"));
            match client_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("client should receive first completion: {error}"),
                None => panic!("client should receive first completion"),
            }
            tokio::time::timeout(Duration::from_secs(1), async {
                while active_pressure() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_elapsed| panic!("first completion should release active load"));

            client_websocket
                .send(Message::text(r#"{"type":"response.create","turn":2}"#))
                .await
                .unwrap_or_else(|error| panic!("second request should send: {error}"));
            match upstream_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("upstream should receive second request: {error}"),
                None => panic!("upstream should receive second request"),
            }
            tokio::time::timeout(Duration::from_secs(1), async {
                while active_pressure() != 8 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_elapsed| panic!("second request should re-reserve active load"));

            upstream_websocket
                .send(Message::text(r#"{"type":"response.completed","turn":2}"#))
                .await
                .unwrap_or_else(|error| panic!("second completion should send: {error}"));
            match client_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("client should receive second completion: {error}"),
                None => panic!("client should receive second completion"),
            }
            tokio::time::timeout(Duration::from_secs(1), async {
                while active_pressure() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_elapsed| panic!("second completion should release active load"));
            drop(upstream_websocket);
        };

        let (router_result, ()) = tokio::join!(router_task, peer_task);
        assert!(
            router_result.is_ok(),
            "same-socket re-reservation flow should complete: {router_result:?}"
        );
    }

    #[tokio::test]
    async fn upstream_usage_limit_frame_is_hidden_behind_codex_reconnect_signal() {
        let (router_local_stream, client_stream) = duplex(4096);
        let (router_upstream_stream, upstream_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let router_upstream_websocket =
            WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let mut upstream_websocket =
            WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
        let registry = WebSocketRevocationRegistry::new();
        let session = registry.register_cancellation(TokenGeneration::new(1));
        let revocation = session.cancellation().clone();
        let session_shutdown = CancellationToken::new();
        let provider_error_observer = Arc::new(RecordingAsyncProviderErrorObserver::default());
        let affinity_secret = RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
        let selected_account = AccountId::new("acct_selected")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
        let affinity_owner_context = WebSocketAffinityOwnerContext {
            affinity_secret,
            account_id: selected_account.clone(),
            credential_generation: 1,
            active_reservation_guard: None,
        };
        let usage_limit_frame = r#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#;

        let router_task = async {
            forward_duplex_until_complete(
                router_local_websocket,
                router_upstream_websocket,
                WebSocketForwardingContext {
                    session_registration: session,
                    affinity_owner_recorder: None,
                    async_affinity_owner_recorder: None,
                    affinity_record_tasks: TaskTracker::new(),
                    affinity_owner_context: Some(&affinity_owner_context),
                    provider_error_observer: Some(provider_error_observer.clone()),
                    revocation: &revocation,
                    session_shutdown: &session_shutdown,
                },
            )
            .await
        };
        let peer_task = async {
            client_websocket
                .send(Message::text(r#"{"type":"response.create","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("local frame should send: {error}"));
            match upstream_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
                None => panic!("upstream should receive first frame"),
            }
            upstream_websocket
                .send(Message::text(usage_limit_frame))
                .await
                .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
            let client_message = match client_websocket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => panic!("client should receive reconnect signal: {error}"),
                None => panic!("client should receive reconnect signal"),
            };
            assert_eq!(
                client_message.to_string(),
                super::CODEX_WEBSOCKET_RECONNECT_SIGNAL
            );
            assert!(
                !client_message.to_string().contains("usage_limit_reached"),
                "single-account quota exhaustion must not leak to Codex while router can rotate"
            );
            drop(upstream_websocket);
        };

        let (router_result, ()) = tokio::join!(router_task, peer_task);
        assert!(
            router_result.is_ok(),
            "router should hide provider quota exhaustion behind reconnect signal, got {router_result:?}"
        );
        tokio::time::timeout(
            Duration::from_secs(1),
            provider_error_observer.observed.notified(),
        )
        .await
        .unwrap_or_else(|_elapsed| panic!("provider error should be observed"));
        assert_eq!(
            provider_error_observer.records(),
            vec![RecordedProviderError {
                account_id: selected_account,
                route_band: codex_router_core::routes::RouteBand::Responses,
                body: usage_limit_frame.as_bytes().to_vec(),
            }]
        );
    }

    #[tokio::test]
    async fn upstream_usage_limit_frame_does_not_emit_reconnect_when_exhaustion_mark_fails() {
        let (router_local_stream, client_stream) = duplex(4096);
        let (router_upstream_stream, upstream_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let router_upstream_websocket =
            WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let mut upstream_websocket =
            WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
        let registry = WebSocketRevocationRegistry::new();
        let session = registry.register_cancellation(TokenGeneration::new(1));
        let revocation = session.cancellation().clone();
        let session_shutdown = CancellationToken::new();
        let affinity_secret = RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
        let selected_account = AccountId::new("acct_selected")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
        let affinity_owner_context = WebSocketAffinityOwnerContext {
            affinity_secret,
            account_id: selected_account,
            credential_generation: 1,
            active_reservation_guard: None,
        };
        let usage_limit_frame = r#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#;

        let router_task = async {
            forward_duplex_until_complete(
                router_local_websocket,
                router_upstream_websocket,
                WebSocketForwardingContext {
                    session_registration: session,
                    affinity_owner_recorder: None,
                    async_affinity_owner_recorder: None,
                    affinity_record_tasks: TaskTracker::new(),
                    affinity_owner_context: Some(&affinity_owner_context),
                    provider_error_observer: Some(Arc::new(FailingAsyncProviderErrorObserver)),
                    revocation: &revocation,
                    session_shutdown: &session_shutdown,
                },
            )
            .await
        };
        let peer_task = async {
            client_websocket
                .send(Message::text(r#"{"type":"response.create","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("local frame should send: {error}"));
            match upstream_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
                None => panic!("upstream should receive first frame"),
            }
            upstream_websocket
                .send(Message::text(usage_limit_frame))
                .await
                .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
            let client_message = match client_websocket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => panic!("client should receive router state failure: {error}"),
                None => panic!("client should receive router state failure"),
            };
            assert_eq!(
                client_message.to_string(),
                super::ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL
            );
            assert!(!client_message.to_string().contains("usage_limit_reached"));
            assert_ne!(
                client_message.to_string(),
                super::CODEX_WEBSOCKET_RECONNECT_SIGNAL,
                "router must not ask Codex to reconnect when it failed to mark the account exhausted"
            );
            drop(upstream_websocket);
        };

        let (router_result, ()) = tokio::join!(router_task, peer_task);
        assert!(
            router_result.is_ok(),
            "router should emit scrubbed state-unavailable signal, got {router_result:?}"
        );
    }

    #[tokio::test]
    async fn upstream_websocket_connection_limit_frame_is_forwarded_unchanged_and_observed() {
        let (router_local_stream, client_stream) = duplex(4096);
        let (router_upstream_stream, upstream_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let router_upstream_websocket =
            WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let mut upstream_websocket =
            WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
        let registry = WebSocketRevocationRegistry::new();
        let session = registry.register_cancellation(TokenGeneration::new(1));
        let revocation = session.cancellation().clone();
        let session_shutdown = CancellationToken::new();
        let provider_error_observer = Arc::new(RecordingAsyncProviderErrorObserver::default());
        let affinity_secret = RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
        let selected_account = AccountId::new("acct_selected")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
        let affinity_owner_context = WebSocketAffinityOwnerContext {
            affinity_secret,
            account_id: selected_account.clone(),
            credential_generation: 1,
            active_reservation_guard: None,
        };
        let connection_limit_frame = r#"{"type":"error","status":400,"error":{"type":"invalid_request_error","code":"websocket_connection_limit_reached","message":"Responses websocket connection limit reached"}}"#;

        let router_task = async {
            forward_duplex_until_complete(
                router_local_websocket,
                router_upstream_websocket,
                WebSocketForwardingContext {
                    session_registration: session,
                    affinity_owner_recorder: None,
                    async_affinity_owner_recorder: None,
                    affinity_record_tasks: TaskTracker::new(),
                    affinity_owner_context: Some(&affinity_owner_context),
                    provider_error_observer: Some(provider_error_observer.clone()),
                    revocation: &revocation,
                    session_shutdown: &session_shutdown,
                },
            )
            .await
        };
        let peer_task = async {
            client_websocket
                .send(Message::text(r#"{"type":"response.create","turn":1}"#))
                .await
                .unwrap_or_else(|error| panic!("local frame should send: {error}"));
            match upstream_websocket.next().await {
                Some(Ok(_message)) => {}
                Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
                None => panic!("upstream should receive first frame"),
            }
            upstream_websocket
                .send(Message::text(connection_limit_frame))
                .await
                .unwrap_or_else(|error| panic!("connection-limit should send: {error}"));
            let client_message = match client_websocket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => panic!("client should receive connection-limit frame: {error}"),
                None => panic!("client should receive connection-limit frame"),
            };
            assert_eq!(client_message.to_string(), connection_limit_frame);
            drop(upstream_websocket);
        };

        let (router_result, ()) = tokio::join!(router_task, peer_task);
        assert!(
            router_result.is_ok(),
            "router should keep connection-limit as pass-through frame, got {router_result:?}"
        );
        tokio::time::timeout(
            Duration::from_secs(1),
            provider_error_observer.observed.notified(),
        )
        .await
        .unwrap_or_else(|_elapsed| panic!("connection-limit should be observed"));
        assert_eq!(
            provider_error_observer.records(),
            vec![RecordedProviderError {
                account_id: selected_account,
                route_band: codex_router_core::routes::RouteBand::Responses,
                body: connection_limit_frame.as_bytes().to_vec(),
            }]
        );
    }

    #[tokio::test]
    async fn runtime_shutdown_cancels_active_duplex_pumps() {
        let (router_local_stream, client_stream) = duplex(4096);
        let (router_upstream_stream, upstream_stream) = duplex(4096);
        let router_local_websocket =
            WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
        let router_upstream_websocket =
            WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
        let mut client_websocket =
            WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
        let mut upstream_websocket =
            WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
        let registry = WebSocketRevocationRegistry::new();
        let session = registry.register_cancellation(TokenGeneration::new(1));
        let revocation = session.cancellation().clone();
        let session_shutdown = CancellationToken::new();
        let session_shutdown_for_task = session_shutdown.clone();

        let router_task = tokio::spawn(async move {
            forward_duplex_until_complete(
                router_local_websocket,
                router_upstream_websocket,
                WebSocketForwardingContext {
                    session_registration: session,
                    affinity_owner_recorder: None,
                    async_affinity_owner_recorder: None,
                    affinity_record_tasks: TaskTracker::new(),
                    affinity_owner_context: None,
                    provider_error_observer: None,
                    revocation: &revocation,
                    session_shutdown: &session_shutdown_for_task,
                },
            )
            .await
        });
        client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .unwrap_or_else(|error| panic!("local frame should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive frame: {error}"),
            None => panic!("upstream should receive frame"),
        }
        assert_eq!(registry.snapshot().active_sessions, 1);

        session_shutdown.cancel();
        let router_result = tokio::time::timeout(Duration::from_secs(1), router_task)
            .await
            .unwrap_or_else(|_elapsed| panic!("router pump should exit promptly on shutdown"))
            .unwrap_or_else(|error| panic!("router pump task should join: {error}"));
        assert!(
            router_result.is_ok(),
            "shutdown should close active pump cleanly, got {router_result:?}"
        );
        assert_eq!(registry.snapshot().active_sessions, 0);
    }
}

/// Blocking WebSocket tunnel that uses the authenticated first-frame router.
#[cfg(test)]
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
    revocations: WebSocketRevocationRegistry,
    affinity_owner_recorder: Option<Arc<dyn HttpAffinityOwnerRecorder>>,
    async_affinity_owner_recorder: Option<Arc<dyn AsyncHttpAffinityOwnerRecorder>>,
    affinity_record_tasks: TaskTracker,
    provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
    session_shutdown: CancellationToken,
    local_peer_addr: Option<SocketAddr>,
}

#[cfg(test)]
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
        let first_message = match local_websocket.read() {
            Ok(message) => message,
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
            revocations: WebSocketRevocationRegistry::new(),
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            provider_error_observer: None,
            session_shutdown: CancellationToken::new(),
            local_peer_addr: None,
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
            revocations: WebSocketRevocationRegistry::new(),
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            provider_error_observer: None,
            session_shutdown: CancellationToken::new(),
            local_peer_addr: None,
        }
    }

    /// Adds shared revocation tracking for token rotation.
    #[must_use]
    pub fn with_revocation_registry(mut self, revocations: WebSocketRevocationRegistry) -> Self {
        self.revocations = revocations;
        self
    }

    /// Adds process/runtime shutdown cancellation for active upgraded sessions.
    #[must_use]
    pub fn with_session_shutdown(mut self, session_shutdown: CancellationToken) -> Self {
        self.session_shutdown = session_shutdown;
        self
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
        affinity_owner_recorder: Arc<dyn HttpAffinityOwnerRecorder>,
    ) -> Self {
        self.affinity_owner_recorder = Some(affinity_owner_recorder);
        self
    }

    /// Adds the async previous-response owner recorder for production Tokio runtime callers.
    #[must_use]
    pub fn with_async_affinity_owner_recorder(
        mut self,
        affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    ) -> Self {
        self.async_affinity_owner_recorder = Some(affinity_owner_recorder);
        self
    }

    /// Adds the task tracker used to drain non-blocking affinity-owner writes on shutdown.
    #[must_use]
    pub fn with_affinity_owner_task_tracker(mut self, affinity_record_tasks: TaskTracker) -> Self {
        self.affinity_record_tasks = affinity_record_tasks;
        self
    }

    /// Adds async provider error observer for quota accounting.
    #[must_use]
    pub fn with_provider_error_observer(
        mut self,
        provider_error_observer: Arc<dyn AsyncProviderErrorObserver>,
    ) -> Self {
        self.provider_error_observer = Some(provider_error_observer);
        self
    }

    /// Adds the local peer socket address observed by the Hyper accept path.
    #[must_use]
    pub fn with_local_peer_addr(mut self, local_peer_addr: Option<SocketAddr>) -> Self {
        self.local_peer_addr = local_peer_addr;
        self
    }

    /// Handles one already-upgraded local WebSocket stream.
    pub async fn handle_upgraded_connection<LocalStream>(
        &self,
        mut local_websocket: WebSocketStream<LocalStream>,
        handshake: WebSocketHandshakeRequest,
        upstream_url: &str,
    ) -> Result<(), WebSocketTunnelError>
    where
        LocalStream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let Some(first_message) =
            next_data_message_before_upstream(&mut local_websocket, &self.session_shutdown).await?
        else {
            return Ok(());
        };
        let first_frame = frame_from_message(first_message);
        let decision = tokio::select! {
            () = self.session_shutdown.cancelled() => {
                local_websocket.close(None).await?;
                return Ok(());
            }
            decision = self.router.route_first_frame(handshake, first_frame) => {
                match decision {
                    Ok(decision) => decision,
                    Err(close_reason) => {
                        if handle_pre_upstream_close_reason(&mut local_websocket, &close_reason).await? {
                            return Ok(());
                        }
                        return Err(WebSocketTunnelError::CloseReason(close_reason));
                    }
                }
            }
        };
        let WebSocketFirstFrameDecision::OpenUpstream {
            token_generation,
            headers,
            first_frame,
            affinity_owner_context,
        } = decision;
        let session_registration = self
            .revocations
            .register_cancellation_with_peer_addr(token_generation, self.local_peer_addr);
        let revocation = session_registration.cancellation().clone();

        let mut upstream_request = upstream_url.into_client_request()?;
        apply_upstream_headers(upstream_request.headers_mut(), &headers)?;
        let (mut upstream_websocket, _response) = tokio::select! {
            () = self.session_shutdown.cancelled() => {
                local_websocket.close(None).await?;
                return Ok(());
            }
            () = revocation.cancelled() => {
                local_websocket.close(None).await?;
                return Ok(());
            }
            connection = connect_async(upstream_request) => connection?,
        };
        let upstream_first_message = message_from_frame(first_frame)?;
        tokio::select! {
            () = self.session_shutdown.cancelled() => {
                local_websocket.close(None).await?;
                upstream_websocket.close(None).await?;
                return Ok(());
            }
            () = revocation.cancelled() => {
                local_websocket.close(None).await?;
                upstream_websocket.close(None).await?;
                return Ok(());
            }
            result = upstream_websocket.send(upstream_first_message) => {
                result?;
            }
        }

        forward_duplex_until_complete(
            local_websocket,
            upstream_websocket,
            WebSocketForwardingContext {
                session_registration,
                affinity_owner_recorder: self.affinity_owner_recorder.clone(),
                async_affinity_owner_recorder: self.async_affinity_owner_recorder.clone(),
                affinity_record_tasks: self.affinity_record_tasks.clone(),
                affinity_owner_context: affinity_owner_context.as_ref(),
                provider_error_observer: self.provider_error_observer.clone(),
                revocation: &revocation,
                session_shutdown: &self.session_shutdown,
            },
        )
        .await
    }
}

async fn handle_pre_upstream_close_reason<LocalStream>(
    local_websocket: &mut WebSocketStream<LocalStream>,
    close_reason: &WebSocketCloseReason,
) -> Result<bool, WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin,
{
    if !matches!(
        close_reason,
        WebSocketCloseReason::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
        }
    ) {
        return Ok(false);
    }

    local_websocket
        .send(Message::text(ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL))
        .await?;
    local_websocket.close(None).await?;
    Ok(true)
}

async fn next_data_message_before_upstream<LocalStream>(
    local_websocket: &mut WebSocketStream<LocalStream>,
    session_shutdown: &CancellationToken,
) -> Result<Option<Message>, WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let message = tokio::select! {
            () = session_shutdown.cancelled() => {
                local_websocket.close(None).await?;
                return Ok(None);
            }
            message = local_websocket.next() => message,
        };

        match message {
            Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                return Ok(Some(message));
            }
            Some(Ok(Message::Ping(payload))) => {
                local_websocket.send(Message::Pong(payload)).await?;
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_close_frame))) => return Ok(None),
            Some(Ok(Message::Frame(_))) => {}
            Some(Err(error)) if is_reset_without_closing_handshake(&error) => return Ok(None),
            Some(Err(error)) => return Err(WebSocketTunnelError::Transport(error)),
            None => return Ok(None),
        }
    }
}

struct WebSocketForwardingContext<'a> {
    session_registration: WebSocketSessionRegistration,
    affinity_owner_recorder: Option<Arc<dyn HttpAffinityOwnerRecorder>>,
    async_affinity_owner_recorder: Option<Arc<dyn AsyncHttpAffinityOwnerRecorder>>,
    affinity_record_tasks: TaskTracker,
    affinity_owner_context: Option<&'a WebSocketAffinityOwnerContext>,
    provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
    revocation: &'a CancellationToken,
    session_shutdown: &'a CancellationToken,
}

async fn forward_duplex_until_complete<LocalStream, UpstreamStream>(
    local_websocket: WebSocketStream<LocalStream>,
    upstream_websocket: WebSocketStream<UpstreamStream>,
    context: WebSocketForwardingContext<'_>,
) -> Result<(), WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    UpstreamStream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (local_write, local_read) = local_websocket.split();
    let (upstream_write, upstream_read) = upstream_websocket.split();
    let local_to_upstream_revocation = context.revocation.clone();
    let upstream_to_local_revocation = context.revocation.clone();
    let local_to_upstream_shutdown = context.session_shutdown.clone();
    let upstream_to_local_shutdown = context.session_shutdown.clone();
    let session_registry = context.session_registration.registry.clone();
    let session_id = context.session_registration.session_id;
    let affinity_owner_context = context.affinity_owner_context.cloned();
    let active_turn_reservation = ActiveTurnReservationState::new(
        affinity_owner_context
            .as_ref()
            .and_then(|context| context.active_reservation_guard.clone()),
    );
    let local_active_turn_reservation = active_turn_reservation.clone();
    let mut local_to_upstream = tokio::spawn(async move {
        pump_local_to_upstream(
            local_read,
            upstream_write,
            local_to_upstream_revocation,
            local_to_upstream_shutdown,
            local_active_turn_reservation,
        )
        .await
    });
    let mut upstream_to_local = tokio::spawn(async move {
        pump_upstream_to_local(
            upstream_read,
            local_write,
            UpstreamToLocalPumpContext {
                revocation: upstream_to_local_revocation,
                session_shutdown: upstream_to_local_shutdown,
                session_registry,
                session_id,
                affinity_owner_recorder: context.affinity_owner_recorder,
                async_affinity_owner_recorder: context.async_affinity_owner_recorder,
                affinity_record_tasks: context.affinity_record_tasks,
                affinity_owner_context,
                active_turn_reservation,
                provider_error_observer: context.provider_error_observer,
            },
        )
        .await
    });

    let result = tokio::select! {
        () = context.revocation.cancelled() => {
            abort_websocket_pump(&mut local_to_upstream).await;
            abort_websocket_pump(&mut upstream_to_local).await;
            Ok(())
        }
        () = context.session_shutdown.cancelled() => {
            abort_websocket_pump(&mut local_to_upstream).await;
            abort_websocket_pump(&mut upstream_to_local).await;
            Ok(())
        }
        result = &mut local_to_upstream => {
            abort_websocket_pump(&mut upstream_to_local).await;
            flatten_websocket_pump_join(result)
        }
        result = &mut upstream_to_local => {
            abort_websocket_pump(&mut local_to_upstream).await;
            flatten_websocket_pump_join(result)
        }
    };

    drop(context.session_registration);
    result
}

async fn pump_local_to_upstream<LocalStream, UpstreamStream>(
    mut local_read: SplitStream<WebSocketStream<LocalStream>>,
    mut upstream_write: SplitSink<WebSocketStream<UpstreamStream>, Message>,
    revocation: CancellationToken,
    session_shutdown: CancellationToken,
    active_turn_reservation: ActiveTurnReservationState,
) -> Result<(), WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin,
    UpstreamStream: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            () = revocation.cancelled() => {
                upstream_write.close().await?;
                return Ok(());
            }
            () = session_shutdown.cancelled() => {
                upstream_write.close().await?;
                return Ok(());
            }
            local_message = local_read.next() => {
                let Some(local_message) = local_message else {
                    upstream_write.close().await?;
                    return Ok(());
                };
                let local_message = match local_message {
                    Ok(message) => message,
                    Err(error) if is_reset_without_closing_handshake(&error) => {
                        upstream_write.close().await?;
                        return Ok(());
                    }
                    Err(error) => return Err(WebSocketTunnelError::Transport(error)),
                };
                let is_close = matches!(local_message, Message::Close(_));
                if is_response_create(&local_message) {
                    active_turn_reservation.reserve_if_idle(current_unix_seconds());
                }
                upstream_write.send(local_message).await?;
                if is_close {
                    return Ok(());
                }
            }
        }
    }
}

struct UpstreamToLocalPumpContext {
    revocation: CancellationToken,
    session_shutdown: CancellationToken,
    session_registry: WebSocketRevocationRegistry,
    session_id: u64,
    affinity_owner_recorder: Option<Arc<dyn HttpAffinityOwnerRecorder>>,
    async_affinity_owner_recorder: Option<Arc<dyn AsyncHttpAffinityOwnerRecorder>>,
    affinity_record_tasks: TaskTracker,
    affinity_owner_context: Option<WebSocketAffinityOwnerContext>,
    active_turn_reservation: ActiveTurnReservationState,
    provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
}

async fn pump_upstream_to_local<LocalStream, UpstreamStream>(
    mut upstream_read: SplitStream<WebSocketStream<UpstreamStream>>,
    mut local_write: SplitSink<WebSocketStream<LocalStream>, Message>,
    context: UpstreamToLocalPumpContext,
) -> Result<(), WebSocketTunnelError>
where
    LocalStream: AsyncRead + AsyncWrite + Unpin,
    UpstreamStream: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            () = context.revocation.cancelled() => {
                local_write.close().await?;
                return Ok(());
            }
            () = context.session_shutdown.cancelled() => {
                local_write.close().await?;
                return Ok(());
            }
            upstream_message = upstream_read.next() => {
                let Some(upstream_message) = upstream_message else {
                    local_write.close().await?;
                    return Ok(());
                };
                let upstream_message = match upstream_message {
                    Ok(message) => message,
                    Err(error) if is_reset_without_closing_handshake(&error) => {
                        local_write.close().await?;
                        return Ok(());
                    }
                    Err(error) => return Err(WebSocketTunnelError::Transport(error)),
                };
                let is_close = matches!(upstream_message, Message::Close(_));
                let metadata_text = websocket_metadata_text_handle(&upstream_message);
                let provider_error_classification = provider_error_classification_from_message(&upstream_message);
                let provider_error_body = provider_error_body_from_message(&upstream_message);
                let upstream_message =
                    maybe_replace_account_quota_exhaustion_with_reconnect_signal(
                        upstream_message,
                        provider_error_classification,
                        provider_error_body.as_deref(),
                        &context,
                    )
                    .await;
                local_write.send(upstream_message).await?;
                context.session_registry.note_upstream_message_forwarded(context.session_id);
                if let Some(metadata_text) = metadata_text {
                    let session_registry = context.session_registry.clone();
                    let session_id = context.session_id;
                    let affinity_owner_context = context.affinity_owner_context.clone();
                    let active_turn_reservation = context.active_turn_reservation.clone();
                    let async_affinity_owner_recorder =
                        context.async_affinity_owner_recorder.clone();
                    let affinity_owner_recorder = context.affinity_owner_recorder.clone();
                    context.affinity_record_tasks.spawn(async move {
                        record_forwarded_websocket_metadata(
                            metadata_text,
                            session_registry,
                            session_id,
                            affinity_owner_context.as_ref(),
                            active_turn_reservation,
                            async_affinity_owner_recorder,
                            affinity_owner_recorder,
                        )
                        .await;
                    });
                }
                if provider_error_classification != ProviderErrorClassification::AccountQuotaExhausted
                    && let Some(provider_error_body) = provider_error_body
                    && let Some(provider_error_observer) = context.provider_error_observer.clone()
                    && let Some(affinity_owner_context) = context.affinity_owner_context.as_ref()
                {
                    let account_id = affinity_owner_context.account_id.clone();
                    context.affinity_record_tasks.spawn(async move {
                        let _observation_result = provider_error_observer
                            .observe_provider_error(
                                account_id,
                                RouteBand::Responses,
                                provider_error_body,
                                current_unix_seconds(),
                            )
                            .await;
                    });
                }
                if is_close {
                    return Ok(());
                }
            }
        }
    }
}

async fn maybe_replace_account_quota_exhaustion_with_reconnect_signal(
    upstream_message: Message,
    classification: ProviderErrorClassification,
    provider_error_body: Option<&[u8]>,
    context: &UpstreamToLocalPumpContext,
) -> Message {
    if classification != ProviderErrorClassification::AccountQuotaExhausted {
        return upstream_message;
    }
    let Some(provider_error_body) = provider_error_body else {
        return upstream_message;
    };
    let Some(provider_error_observer) = context.provider_error_observer.as_ref() else {
        return upstream_message;
    };
    let Some(affinity_owner_context) = context.affinity_owner_context.as_ref() else {
        return upstream_message;
    };

    match provider_error_observer
        .observe_provider_error(
            affinity_owner_context.account_id.clone(),
            RouteBand::Responses,
            provider_error_body.to_vec(),
            current_unix_seconds(),
        )
        .await
    {
        Ok(()) => {
            crate::telemetry::record_websocket_event(
                RouteBand::Responses.as_str(),
                "quota_reconnect",
            );
            Message::text(CODEX_WEBSOCKET_RECONNECT_SIGNAL)
        }
        Err(_error) => {
            crate::telemetry::record_websocket_event(
                RouteBand::Responses.as_str(),
                "quota_state_unavailable",
            );
            Message::text(ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL)
        }
    }
}

async fn record_forwarded_websocket_metadata(
    metadata_text: tungstenite::Utf8Bytes,
    session_registry: WebSocketRevocationRegistry,
    session_id: u64,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
    active_turn_reservation: ActiveTurnReservationState,
    async_affinity_owner_recorder: Option<Arc<dyn AsyncHttpAffinityOwnerRecorder>>,
    affinity_owner_recorder: Option<Arc<dyn HttpAffinityOwnerRecorder>>,
) {
    let is_completed = is_response_completed_text(&metadata_text);
    if is_completed {
        active_turn_reservation.release_current();
        session_registry.note_response_completed(session_id);
    }
    let affinity_owner =
        websocket_affinity_owner_record_from_text(&metadata_text, affinity_owner_context);
    if let Some(owner) = affinity_owner {
        if let Some(recorder) = async_affinity_owner_recorder {
            let _result = recorder.record_affinity_owner(owner).await;
        } else if let Some(recorder) = affinity_owner_recorder {
            let _join_result = tokio::task::spawn_blocking(move || {
                let _result = recorder.record_affinity_owner(&owner);
            })
            .await;
        }
    }
}

#[derive(Clone, Debug)]
struct ActiveTurnReservationState {
    reservation_template: Option<ActiveReservationGuard>,
    current_reservation: Arc<Mutex<Option<ActiveReservationGuard>>>,
}

impl ActiveTurnReservationState {
    fn new(initial_reservation: Option<ActiveReservationGuard>) -> Self {
        Self {
            reservation_template: initial_reservation.clone(),
            current_reservation: Arc::new(Mutex::new(initial_reservation)),
        }
    }

    fn release_current(&self) {
        let Ok(mut current_reservation) = self.current_reservation.lock() else {
            return;
        };
        if let Some(reservation) = current_reservation.take() {
            reservation.release();
        }
    }

    fn reserve_if_idle(&self, reserved_unix_seconds: u64) {
        let Some(template) = self.reservation_template.as_ref() else {
            return;
        };
        let Ok(mut current_reservation) = self.current_reservation.lock() else {
            return;
        };
        if current_reservation.is_some() {
            return;
        }
        *current_reservation = template.reserve_again_at(reserved_unix_seconds);
    }
}

async fn abort_websocket_pump(handle: &mut JoinHandle<Result<(), WebSocketTunnelError>>) {
    handle.abort();
    let _join_result = handle.await;
}

fn flatten_websocket_pump_join(
    result: Result<Result<(), WebSocketTunnelError>, tokio::task::JoinError>,
) -> Result<(), WebSocketTunnelError> {
    match result {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Ok(()),
        Err(error) => Err(WebSocketTunnelError::TaskJoin(error.to_string())),
    }
}

#[cfg(test)]
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

#[cfg(test)]
fn record_websocket_affinity_owner(
    upstream_message: &Message,
    affinity_owner_recorder: Option<&dyn HttpAffinityOwnerRecorder>,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) {
    let Some(recorder) = affinity_owner_recorder else {
        return;
    };
    let Some(owner) = websocket_affinity_owner_record(upstream_message, affinity_owner_context)
    else {
        return;
    };
    let _result = recorder.record_affinity_owner(&owner);
}

#[cfg(test)]
fn websocket_affinity_owner_record(
    upstream_message: &Message,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) -> Option<PreviousResponseAffinityOwnerRecord> {
    let text = websocket_metadata_text_handle(upstream_message)?;
    websocket_affinity_owner_record_from_text(&text, affinity_owner_context)
}

fn websocket_affinity_owner_record_from_text(
    text: &str,
    affinity_owner_context: Option<&WebSocketAffinityOwnerContext>,
) -> Option<PreviousResponseAffinityOwnerRecord> {
    let context = affinity_owner_context?;
    let previous_response_id = extract_websocket_response_id_from_text(text)?;
    let Ok(affinity_key_hash) =
        hash_previous_response_id(&context.affinity_secret, &previous_response_id)
    else {
        return None;
    };
    Some(PreviousResponseAffinityOwnerRecord::new(
        affinity_key_hash,
        context.account_id.clone(),
        context.credential_generation,
        RouteBand::Responses,
        AffinitySourceTransport::WebSocket,
        current_unix_seconds(),
    ))
}

fn extract_websocket_response_id_from_text(text: &str) -> Option<PreviousResponseId> {
    if text.len() > WEBSOCKET_METADATA_SCAN_LIMIT_BYTES {
        return None;
    }
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

#[cfg(test)]
fn is_response_completed(message: &Message) -> bool {
    let Some(text) = websocket_metadata_text_handle(message) else {
        return false;
    };
    is_response_completed_text(&text)
}

fn is_response_create(message: &Message) -> bool {
    let Some(text) = websocket_metadata_text_handle(message) else {
        return false;
    };
    bounded_top_level_json_string_field_equals(
        text.as_str().as_bytes(),
        b"type",
        b"response.create",
    )
}

fn is_response_completed_text(text: &str) -> bool {
    bounded_top_level_json_string_field_equals(text.as_bytes(), b"type", b"response.completed")
}

fn has_forbidden_top_level_websocket_auth_carrier(body: &[u8]) -> bool {
    bounded_top_level_json_key_matches(body, |key| {
        let canonical = canonical_websocket_auth_field_name(key);
        matches!(
            canonical.as_deref(),
            Some("authorization" | "api-key" | "openai-api-key" | "x-codex-router-token")
        )
    })
}

fn bounded_top_level_json_string_field_equals(
    body: &[u8],
    field_name: &[u8],
    expected: &[u8],
) -> bool {
    let Some(value) = bounded_top_level_json_string_field(body, field_name) else {
        return false;
    };
    value.as_bytes() == expected
}

fn bounded_top_level_json_string_field(body: &[u8], field_name: &[u8]) -> Option<String> {
    let mut cursor = skip_json_whitespace(body, 0);
    if body.get(cursor) != Some(&b'{') {
        return None;
    }
    cursor += 1;
    let mut depth = 1_u32;
    let scan_end = body.len().min(WEBSOCKET_METADATA_SCAN_LIMIT_BYTES);
    let mut top_level_keys = 0_usize;
    while cursor < scan_end {
        cursor = skip_json_whitespace(body, cursor);
        let byte = body.get(cursor).copied()?;
        match byte {
            b'"' => {
                let (string_slice, after_string) = json_string_slice(body, cursor)?;
                if after_string > scan_end {
                    return None;
                }
                let after_key = skip_json_whitespace(body, after_string);
                if depth == 1 && body.get(after_key) == Some(&b':') {
                    top_level_keys += 1;
                    if top_level_keys > WEBSOCKET_METADATA_SCAN_MAX_TOP_LEVEL_KEYS {
                        return None;
                    }
                    let key = serde_json::from_slice::<String>(string_slice).ok()?;
                    if key.as_bytes() == field_name {
                        let value_start = skip_json_whitespace(body, after_key + 1);
                        let (value_slice, _after_value) = json_string_slice(body, value_start)?;
                        return serde_json::from_slice::<String>(value_slice).ok();
                    }
                    cursor = after_key + 1;
                } else {
                    cursor = after_string;
                }
            }
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                cursor += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                if depth == 0 {
                    return None;
                }
            }
            _ => {
                cursor += 1;
            }
        }
    }
    None
}

fn bounded_top_level_json_key_matches(
    body: &[u8],
    mut matches_key: impl FnMut(&str) -> bool,
) -> bool {
    let mut cursor = skip_json_whitespace(body, 0);
    if body.get(cursor) != Some(&b'{') {
        return false;
    }
    cursor += 1;
    let scan_end = body.len().min(WEBSOCKET_METADATA_SCAN_LIMIT_BYTES);
    let mut depth = 1_u32;
    let mut top_level_keys = 0_usize;
    while cursor < scan_end {
        cursor = skip_json_whitespace(body, cursor);
        let Some(byte) = body.get(cursor).copied() else {
            return false;
        };
        match byte {
            b'"' => {
                let Some((string_slice, after_string)) = json_string_slice(body, cursor) else {
                    return false;
                };
                if after_string > scan_end {
                    return false;
                }
                let after_key = skip_json_whitespace(body, after_string);
                if depth == 1 && body.get(after_key) == Some(&b':') {
                    top_level_keys += 1;
                    if top_level_keys > WEBSOCKET_METADATA_SCAN_MAX_TOP_LEVEL_KEYS {
                        return false;
                    }
                    if let Ok(key) = serde_json::from_slice::<String>(string_slice)
                        && matches_key(&key)
                    {
                        return true;
                    }
                    cursor = after_key + 1;
                } else {
                    cursor = after_string;
                }
            }
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                cursor += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                if depth == 0 {
                    return false;
                }
            }
            _ => cursor += 1,
        }
    }
    false
}

fn canonical_websocket_auth_field_name(value: &str) -> Option<String> {
    let decoded = percent_decode_ascii(value);
    let canonical: String = decoded
        .chars()
        .filter(|character| !matches!(character, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect();
    match canonical.as_str() {
        "authorization" => Some("authorization".to_owned()),
        "apikey" => Some("api-key".to_owned()),
        "openaiapikey" => Some("openai-api-key".to_owned()),
        "xcodexroutertoken" => Some("x-codex-router-token".to_owned()),
        _ => None,
    }
}

fn percent_decode_ascii(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn json_string_slice(body: &[u8], start: usize) -> Option<(&[u8], usize)> {
    if body.get(start) != Some(&b'"') {
        return None;
    }
    let mut cursor = start + 1;
    while cursor < body.len() {
        match body[cursor] {
            b'\\' => cursor = cursor.saturating_add(2),
            b'"' => {
                let end = cursor + 1;
                return Some((&body[start..end], end));
            }
            _ => cursor += 1,
        }
    }
    None
}

fn skip_json_whitespace(body: &[u8], mut cursor: usize) -> usize {
    while body
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        cursor += 1;
    }
    cursor
}

fn websocket_metadata_text_handle(message: &Message) -> Option<tungstenite::Utf8Bytes> {
    let Message::Text(text) = message else {
        return None;
    };
    Some(text.clone())
}

fn provider_error_body_from_message(message: &Message) -> Option<Vec<u8>> {
    let Message::Text(text) = message else {
        return None;
    };
    let body = text.as_str().as_bytes();
    if provider_error_classification_from_message(message) == ProviderErrorClassification::Unknown {
        return None;
    }

    Some(body.to_vec())
}

fn provider_error_classification_from_message(message: &Message) -> ProviderErrorClassification {
    let Message::Text(text) = message else {
        return ProviderErrorClassification::Unknown;
    };
    classify_provider_error_envelope(text.as_str().as_bytes())
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[allow(clippy::result_large_err)]
#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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
    /// Async WebSocket pump task failed unexpectedly.
    #[error("websocket pump task failed: {0}")]
    TaskJoin(String),
}
