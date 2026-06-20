//! HTTP and SSE proxy handling without network binding.

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
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::redaction::SecretString;
use codex_router_quota::snapshot::QuotaSnapshot;
use codex_router_quota::snapshot::SnapshotFreshness;
use codex_router_secret_store::account_tokens::upstream_access_token_key;
use codex_router_secret_store::file_backend::SecretStore;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_selection::eligibility::Eligibility;
use codex_router_selection::eligibility::SelectionCandidate;
use codex_router_selection::weighted_deficit::WeightedDeficitSelector;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::Cursor;
use std::io::Read;
use std::sync::Arc;
use std::sync::Mutex;
use thiserror::Error;

use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::local_auth::ProxyLocalAuthGate;
use crate::routes::Method;
use crate::routes::RouteClass;
use crate::routes::RouteKind;
use crate::routes::classify_route;
use crate::upstream::UpstreamRequestBuilder;

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

/// HTTP/SSE proxy service.
#[derive(Clone, Copy, Debug)]
pub struct HttpProxyService<'a, T>
where
    T: UpstreamHttpTransport,
{
    upstream: &'a T,
}

impl<'a, T> HttpProxyService<'a, T>
where
    T: UpstreamHttpTransport,
{
    /// Creates a proxy service.
    #[must_use]
    pub const fn new(upstream: &'a T) -> Self {
        Self { upstream }
    }

    /// Handles one HTTP/SSE request.
    pub fn handle(
        &self,
        request: HttpProxyRequest,
        upstream_auth_token: SecretString,
    ) -> Result<HttpProxyResponse, HttpProxyError> {
        self.build_upstream_request(request, upstream_auth_token)
            .and_then(|request| self.upstream.send(request))
    }

    fn build_upstream_request(
        &self,
        request: HttpProxyRequest,
        upstream_auth_token: SecretString,
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
            .build(upstream_auth_token);

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
    T: UpstreamHttpTransport + StreamingUpstreamHttpTransport,
{
    /// Handles one HTTP/SSE request without buffering the response body.
    pub fn handle_streaming(
        &self,
        request: HttpProxyRequest,
        upstream_auth_token: SecretString,
    ) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
        self.build_upstream_request(request, upstream_auth_token)
            .and_then(|request| self.upstream.send_streaming(request))
    }
}

/// Selected upstream account material needed by the proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedUpstreamAccount {
    account_id: AccountId,
    upstream_auth_token: SecretString,
    selection_reason: String,
}

impl SelectedUpstreamAccount {
    /// Creates selected account material.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        upstream_auth_token: SecretString,
        selection_reason: impl Into<String>,
    ) -> Self {
        Self {
            account_id,
            upstream_auth_token,
            selection_reason: selection_reason.into(),
        }
    }

    /// Returns selected account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the selected upstream bearer token.
    #[must_use]
    pub fn upstream_auth_token(&self) -> &SecretString {
        &self.upstream_auth_token
    }

    /// Returns a redacted static/audit-safe selection reason.
    #[must_use]
    pub fn selection_reason(&self) -> &str {
        &self.selection_reason
    }
}

/// Selects an upstream account after local auth succeeds.
pub trait UpstreamAccountSelector {
    /// Selects account material for one request.
    fn select_upstream_account(
        &self,
        request: &HttpProxyRequest,
        token_generation: TokenGeneration,
    ) -> Result<SelectedUpstreamAccount, HttpProxyError>;
}

/// Account state consumed by the quota-aware proxy selector adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaAwareAccountState {
    account_id: AccountId,
    upstream_auth_token: SecretString,
    remaining_headroom: u32,
    freshness: SnapshotFreshness,
}

impl QuotaAwareAccountState {
    /// Creates account state for selector input.
    #[must_use]
    pub const fn new(
        account_id: AccountId,
        upstream_auth_token: SecretString,
        remaining_headroom: u32,
        freshness: SnapshotFreshness,
    ) -> Self {
        Self {
            account_id,
            upstream_auth_token,
            remaining_headroom,
            freshness,
        }
    }
}

/// Selection adapter failure.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum QuotaAwareAccountSelectorError {
    /// No account has usable headroom.
    #[error("no eligible accounts")]
    NoEligibleAccounts,
    /// Weighted selector state was unavailable.
    #[error("selector state unavailable")]
    SelectorStateUnavailable,
    /// State repository could not be read.
    #[error("state repository unavailable")]
    StateUnavailable,
    /// Secret store could not be read.
    #[error("secret store unavailable")]
    SecretUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WeightedAccountCandidate {
    account_id: AccountId,
    effective_headroom: u32,
    selection_reason: &'static str,
}

/// Account selector adapter using quota freshness and weighted deficit state.
#[derive(Debug)]
pub struct QuotaAwareAccountSelector {
    accounts: Vec<QuotaAwareAccountState>,
    weighted_selector: Mutex<WeightedDeficitSelector>,
}

impl QuotaAwareAccountSelector {
    /// Creates a quota-aware selector from account snapshots.
    #[must_use]
    pub fn new(accounts: Vec<QuotaAwareAccountState>) -> Self {
        Self {
            accounts,
            weighted_selector: Mutex::new(WeightedDeficitSelector::default()),
        }
    }
}

impl UpstreamAccountSelector for QuotaAwareAccountSelector {
    fn select_upstream_account(
        &self,
        _request: &HttpProxyRequest,
        _token_generation: TokenGeneration,
    ) -> Result<SelectedUpstreamAccount, HttpProxyError> {
        select_from_account_states(&self.accounts, &self.weighted_selector)
    }
}

/// Selector that hydrates account state from repositories at request time.
#[derive(Debug)]
pub struct RepositoryBackedAccountSelector<'a, R, S>
where
    R: AccountStateRepository + QuotaSnapshotRepository,
    S: SecretStore,
{
    state_repository: &'a R,
    secret_store: &'a S,
    now_unix_seconds: u64,
    max_snapshot_age_seconds: u64,
    weighted_selector: Arc<Mutex<WeightedDeficitSelector>>,
}

impl<'a, R, S> RepositoryBackedAccountSelector<'a, R, S>
where
    R: AccountStateRepository + QuotaSnapshotRepository,
    S: SecretStore,
{
    /// Creates a repository-backed selector.
    #[must_use]
    pub fn new(
        state_repository: &'a R,
        secret_store: &'a S,
        now_unix_seconds: u64,
        max_snapshot_age_seconds: u64,
    ) -> Self {
        Self {
            state_repository,
            secret_store,
            now_unix_seconds,
            max_snapshot_age_seconds,
            weighted_selector: Arc::new(Mutex::new(WeightedDeficitSelector::default())),
        }
    }

    /// Creates a repository-backed selector with process-lifetime weighted state.
    #[must_use]
    pub fn new_with_weighted_selector(
        state_repository: &'a R,
        secret_store: &'a S,
        now_unix_seconds: u64,
        max_snapshot_age_seconds: u64,
        weighted_selector: Arc<Mutex<WeightedDeficitSelector>>,
    ) -> Self {
        Self {
            state_repository,
            secret_store,
            now_unix_seconds,
            max_snapshot_age_seconds,
            weighted_selector,
        }
    }
}

impl<R, S> UpstreamAccountSelector for RepositoryBackedAccountSelector<'_, R, S>
where
    R: AccountStateRepository + QuotaSnapshotRepository,
    S: SecretStore,
{
    fn select_upstream_account(
        &self,
        request: &HttpProxyRequest,
        _token_generation: TokenGeneration,
    ) -> Result<SelectedUpstreamAccount, HttpProxyError> {
        let route_band = route_band_for_request(request)?;
        let accounts =
            self.state_repository
                .list_accounts()
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::StateUnavailable,
                })?;
        let mut selector_accounts = Vec::new();
        for account in accounts {
            if account.status() != AccountStatus::Enabled {
                continue;
            }
            let token_key = upstream_access_token_key(account.account_id()).map_err(|_error| {
                HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::SecretUnavailable,
                }
            })?;
            let upstream_auth_token = match self.secret_store.read_secret(&token_key) {
                Ok(token) => token,
                Err(SecretStoreError::Filesystem { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    continue;
                }
                Err(_error) => {
                    return Err(HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::SecretUnavailable,
                    });
                }
            };
            let snapshot = self
                .state_repository
                .load_snapshot_for_route_band(account.account_id(), route_band)
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::StateUnavailable,
                })?;
            selector_accounts.push(account_state_from_repositories(
                account.account_id().clone(),
                upstream_auth_token,
                snapshot.as_ref(),
                route_band,
                self.now_unix_seconds,
                self.max_snapshot_age_seconds,
            ));
        }

        select_from_account_states(&selector_accounts, self.weighted_selector.as_ref())
    }
}

fn select_from_account_states(
    accounts: &[QuotaAwareAccountState],
    weighted_selector: &Mutex<WeightedDeficitSelector>,
) -> Result<SelectedUpstreamAccount, HttpProxyError> {
    let known_fresh_account_exists = accounts.iter().any(|account| {
        account.remaining_headroom > 0
            && matches!(account.freshness, SnapshotFreshness::Fresh { .. })
    });
    let weighted_candidates = accounts
        .iter()
        .filter_map(|account| weighted_candidate_for_account(account, known_fresh_account_exists))
        .collect::<Vec<_>>();
    let selector_input = weighted_candidates
        .iter()
        .map(|candidate| (candidate.account_id.clone(), candidate.effective_headroom))
        .collect::<Vec<_>>();
    let mut weighted_selector =
        weighted_selector
            .lock()
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
            })?;
    let selected_account_id =
        weighted_selector
            .select(&selector_input, 1)
            .ok_or(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
            })?;
    let selected_input = accounts
        .iter()
        .find(|account| account.account_id == selected_account_id)
        .ok_or(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        })?;
    let selected_candidate = weighted_candidates
        .iter()
        .find(|candidate| candidate.account_id == selected_account_id)
        .ok_or(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        })?;

    Ok(SelectedUpstreamAccount::new(
        selected_account_id,
        selected_input.upstream_auth_token.clone(),
        selected_candidate.selection_reason,
    ))
}

fn account_state_from_repositories(
    account_id: AccountId,
    upstream_auth_token: SecretString,
    snapshot: Option<&PersistedQuotaSnapshot>,
    route_band: &str,
    now_unix_seconds: u64,
    max_snapshot_age_seconds: u64,
) -> QuotaAwareAccountState {
    let Some(snapshot) = snapshot else {
        return QuotaAwareAccountState::new(
            account_id,
            upstream_auth_token,
            0,
            SnapshotFreshness::Unknown,
        );
    };
    let remaining_headroom = if snapshot.route_band() == route_band {
        snapshot.remaining_headroom()
    } else {
        0
    };
    let freshness = QuotaSnapshot::freshness_for_observed_at(
        Some(snapshot.observed_unix_seconds()),
        now_unix_seconds,
        max_snapshot_age_seconds,
    );

    QuotaAwareAccountState::new(
        account_id,
        upstream_auth_token,
        remaining_headroom,
        freshness,
    )
}

fn route_band_for_request(request: &HttpProxyRequest) -> Result<&'static str, HttpProxyError> {
    let classification_path = path_without_query(request.path());
    match classify_route(
        request.method,
        classification_path,
        request.websocket_upgrade,
    ) {
        RouteClass::Supported(RouteKind::Responses | RouteKind::ResponsesWebSocket) => {
            Ok("responses")
        }
        RouteClass::Supported(RouteKind::Models) => Ok("models"),
        RouteClass::Supported(RouteKind::MemoriesTraceSummarize) => Ok("memories_trace_summarize"),
        RouteClass::Supported(RouteKind::ResponsesCompact) => Ok("responses_compact"),
        RouteClass::Rejected { reason } => Err(HttpProxyError::Rejected { reason }),
    }
}

fn weighted_candidate_for_account(
    account: &QuotaAwareAccountState,
    known_fresh_account_exists: bool,
) -> Option<WeightedAccountCandidate> {
    let candidate = SelectionCandidate::new(
        account.account_id.clone(),
        account.remaining_headroom,
        account.freshness,
    );
    match candidate.eligibility(known_fresh_account_exists) {
        Eligibility::Eligible { headroom } => Some(WeightedAccountCandidate {
            account_id: account.account_id.clone(),
            effective_headroom: headroom,
            selection_reason: selection_reason_for_freshness(account.freshness),
        }),
        Eligibility::Penalized { headroom, reason } => Some(WeightedAccountCandidate {
            account_id: account.account_id.clone(),
            effective_headroom: headroom,
            selection_reason: reason,
        }),
        Eligibility::Ineligible { .. } => None,
    }
}

const fn selection_reason_for_freshness(freshness: SnapshotFreshness) -> &'static str {
    match freshness {
        SnapshotFreshness::Fresh { .. } => "fresh_quota",
        SnapshotFreshness::StaleWithPenalty { .. } => "stale_quota_fallback",
        SnapshotFreshness::Unknown => "unknown_quota_fallback",
    }
}

/// HTTP/SSE service that composes local auth, account selection, and forwarding.
#[derive(Clone, Copy, Debug)]
pub struct AuthenticatedHttpProxyService<'a, T, S>
where
    T: UpstreamHttpTransport,
    S: UpstreamAccountSelector,
{
    auth_gate: &'a ProxyLocalAuthGate,
    selector: &'a S,
    proxy: HttpProxyService<'a, T>,
    audit_sink: Option<&'a AuditFileSink>,
}

impl<'a, T, S> AuthenticatedHttpProxyService<'a, T, S>
where
    T: UpstreamHttpTransport,
    S: UpstreamAccountSelector,
{
    /// Creates an authenticated HTTP proxy service.
    #[must_use]
    pub const fn new(auth_gate: &'a ProxyLocalAuthGate, selector: &'a S, upstream: &'a T) -> Self {
        Self {
            auth_gate,
            selector,
            proxy: HttpProxyService::new(upstream),
            audit_sink: None,
        }
    }

    /// Adds a private audit sink.
    #[must_use]
    pub const fn with_audit_sink(mut self, audit_sink: &'a AuditFileSink) -> Self {
        self.audit_sink = Some(audit_sink);
        self
    }

    fn emit_audit_event(&self, event: AuditEvent) {
        if let Some(audit_sink) = self.audit_sink {
            let _result = audit_sink.append(&event);
        }
    }
}

impl<T, S> HttpRequestHandler for AuthenticatedHttpProxyService<'_, T, S>
where
    T: UpstreamHttpTransport,
    S: UpstreamAccountSelector,
{
    fn handle_request(
        &self,
        request: HttpProxyRequest,
    ) -> Result<HttpProxyResponse, HttpProxyError> {
        let audit_route_kind = audit_route_kind_for_request(&request);
        let token_generation = match self
            .auth_gate
            .authorize(request.header_value("x-codex-router-token"))
        {
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
        let selected = self
            .selector
            .select_upstream_account(&request, token_generation)?;
        let account_hash = redacted_account_hash(selected.account_id());

        let response = self.proxy.handle(request, selected.upstream_auth_token)?;
        self.emit_audit_event(allowed_audit_event(
            TransportKind::Http,
            audit_route_kind,
            account_hash,
        ));

        Ok(response)
    }
}

impl<T, S> StreamingHttpRequestHandler for AuthenticatedHttpProxyService<'_, T, S>
where
    T: UpstreamHttpTransport + StreamingUpstreamHttpTransport,
    S: UpstreamAccountSelector,
{
    fn handle_streaming_request(
        &self,
        request: HttpProxyRequest,
    ) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
        let audit_route_kind = audit_route_kind_for_request(&request);
        let token_generation = match self
            .auth_gate
            .authorize(request.header_value("x-codex-router-token"))
        {
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
        let selected = self
            .selector
            .select_upstream_account(&request, token_generation)?;
        let account_hash = redacted_account_hash(selected.account_id());

        let response = self
            .proxy
            .handle_streaming(request, selected.upstream_auth_token)?;
        self.emit_audit_event(allowed_audit_event(
            TransportKind::Http,
            audit_route_kind,
            account_hash,
        ));

        Ok(response)
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

fn audit_route_kind_for_request(request: &HttpProxyRequest) -> AuditRouteKind {
    match classify_route(
        request.method,
        path_without_query(request.path()),
        request.websocket_upgrade,
    ) {
        RouteClass::Supported(RouteKind::Responses) => AuditRouteKind::Responses,
        RouteClass::Supported(RouteKind::ResponsesWebSocket) => AuditRouteKind::ResponsesWebSocket,
        RouteClass::Supported(RouteKind::Models) => AuditRouteKind::Models,
        RouteClass::Supported(RouteKind::MemoriesTraceSummarize) => AuditRouteKind::MemoryTrace,
        RouteClass::Supported(RouteKind::ResponsesCompact) => AuditRouteKind::Compact,
        RouteClass::Rejected { .. } => AuditRouteKind::Responses,
    }
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?')
        .map_or(path, |(path_component, _query)| path_component)
}
