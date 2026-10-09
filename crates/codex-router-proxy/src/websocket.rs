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
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
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
use codex_router_core::route_profile::RESPONSES_WEBSOCKET;
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
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::Message;
#[cfg(test)]
use tokio_tungstenite::tungstenite::WebSocket;
#[cfg(test)]
use tokio_tungstenite::tungstenite::accept_hdr_with_config;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
#[cfg(test)]
use tokio_tungstenite::tungstenite::client::connect_with_config;
use tokio_tungstenite::tungstenite::error::ProtocolError;
#[cfg(test)]
use tokio_tungstenite::tungstenite::handshake::server::Request;
#[cfg(test)]
use tokio_tungstenite::tungstenite::handshake::server::Response;
use tokio_tungstenite::tungstenite::http::HeaderName;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::account_selection::AccountDecisionSelector;
use crate::account_selection::ActiveReservationGuard;
use crate::account_selection::AsyncAccountDecisionSelector;
use crate::account_selection::LiveAccountAdmissionAssessor;
use crate::account_selection::PostExhaustionRouteBandOutcome;
use crate::account_selection::QuotaAwareAccountSelectorError;
use crate::capacity_retry::CapacityRetryOutcome;
use crate::capacity_retry::CapacityRetryTracker;

#[path = "websocket/account_turn_admission.rs"]
mod account_turn_admission;
#[path = "websocket/credential_resolution_diagnostic.rs"]
mod credential_resolution_diagnostic;
use crate::capacity_retry::MAX_THREAD_ID_BYTES;
use crate::db_write_actor::DbWriteEnqueueResult;
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
use crate::provider_error::ProviderErrorObservationError;
use crate::provider_error::classify_responses_websocket_error_envelope;
use crate::session_account_affinity_cache::SessionAffinityActivityHandle;
use account_turn_admission::AccountTurnAdmission;
use account_turn_admission::FloorSwitchIntent;

use crate::routes::Method;

mod async_tunnel;
mod authenticated_routing;
mod duplex_forwarding;
mod provider_signals;
mod response_metadata;
mod session_registry;
mod transport_cleanup;

use provider_signals::POST_EXHAUSTION_ALTERNATIVE_SELECTION_TIMEOUT;
pub(crate) use provider_signals::{
    ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL, ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL,
};
use response_metadata::has_forbidden_top_level_websocket_auth_carrier;
pub use session_registry::{
    WebSocketQuotaFloorNotifier, WebSocketRegistrySnapshot, WebSocketRevocationRegistry,
    WebSocketSessionPeerAddr,
};
pub(crate) use transport_cleanup::{is_normal_websocket_cleanup_close, router_websocket_config};

#[cfg(test)]
mod blocking_tunnel;
#[cfg(test)]
#[path = "websocket/forwarding_tests.rs"]
mod forwarding_tests;
#[cfg(test)]
#[path = "websocket/session_registry_tests.rs"]
mod registry_tests;

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
    credit_backed_at_selection: bool,
    active_reservation_guard: Option<ActiveReservationGuard>,
    session_affinity_activity_handle: Option<SessionAffinityActivityHandle>,
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
            credit_backed_at_selection: false,
            active_reservation_guard: None,
            session_affinity_activity_handle: None,
        }
    }

    fn with_active_reservation_guard(
        mut self,
        active_reservation_guard: Option<ActiveReservationGuard>,
    ) -> Self {
        self.active_reservation_guard = active_reservation_guard;
        self
    }

    fn with_credit_backed_at_selection(mut self, credit_backed_at_selection: bool) -> Self {
        self.credit_backed_at_selection = credit_backed_at_selection;
        self
    }

    fn with_session_affinity_activity_handle(
        mut self,
        session_affinity_activity_handle: Option<SessionAffinityActivityHandle>,
    ) -> Self {
        self.session_affinity_activity_handle = session_affinity_activity_handle;
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
    account_admission_assessor: Option<Arc<dyn LiveAccountAdmissionAssessor>>,
    session_shutdown: CancellationToken,
    local_peer_addr: Option<SocketAddr>,
}
struct UpstreamToLocalPumpContext {
    revocation: CancellationToken,
    session_shutdown: CancellationToken,
    tunnel_shutdown: CancellationToken,
    session_registry: WebSocketRevocationRegistry,
    session_id: u64,
    affinity_owner_recorder: Option<Arc<dyn HttpAffinityOwnerRecorder>>,
    async_affinity_owner_recorder: Option<Arc<dyn AsyncHttpAffinityOwnerRecorder>>,
    affinity_record_tasks: TaskTracker,
    affinity_owner_context: Option<WebSocketAffinityOwnerContext>,
    active_turn_reservation: ActiveTurnReservationState,
    provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
    quota_floor_reconnect: CancellationToken,
    early_floor_reconnect: CancellationToken,
    graceful_floor_switch: watch::Receiver<FloorSwitchIntent>,
    account_turn_admission: AccountTurnAdmission,
}
#[derive(Clone, Debug)]
struct ActiveTurnReservationState {
    reservation_template: Option<ActiveReservationGuard>,
    current_reservation: Arc<Mutex<Option<ActiveReservationGuard>>>,
    retired: Arc<AtomicBool>,
}
fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn current_unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
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
