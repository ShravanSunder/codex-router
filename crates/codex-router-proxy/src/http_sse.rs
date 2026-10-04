//! HTTP and SSE proxy handling without network binding.

use bytes::Bytes;
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
use codex_router_core::audit::AuditSinkError;
use codex_router_core::audit::LocalAuthAuditResult;
use codex_router_core::audit::ResponseCommitState;
use codex_router_core::audit::RouteKind as AuditRouteKind;
use codex_router_core::audit::TransportKind;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::RequestId;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_core::route_profile::CLAUDE_MESSAGES;
use codex_router_core::route_profile::RESPONSES_HTTP;
use codex_router_core::route_profile::RESPONSES_WEBSOCKET;
use codex_router_core::route_profile::RouteProfile;
use codex_router_core::route_profile::WindowKind;
use codex_router_core::routes::RouteBand;
use codex_router_state::affinity_owner::AffinitySourceTransport;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::sqlite::StateStoreError;
use codex_router_state::window_observation::WindowObservation;
use codex_router_state::window_observation::WindowObservationFreshnessError;
use codex_router_state::window_observation::WindowObservationProps;
use codex_router_state::window_observation::calculate_window_observation_fresh_until_unix_seconds;
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
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use thiserror::Error;

use crate::account_selection::AccountDecisionSelector;
use crate::account_selection::ActiveReservationGuard;
use crate::account_selection::AsyncAccountDecisionSelector;
use crate::account_selection::QuotaAwareAccountSelectorError;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::local_auth::ProxyLocalAuthGate;
use crate::local_auth::extract_presented_local_token_from_request;
use crate::provider_error::AsyncProviderErrorObserver;
use crate::routes::Method;
use crate::routes::RouteClass;
use crate::routes::RouteKind;
use crate::routes::classify_route;
use crate::upstream::UpstreamRequestBuilder;

const REQUEST_LOCAL_CREDENTIAL_ATTEMPT_LIMIT: usize = 16;

mod http_affinity;
mod http_types;
#[cfg(test)]
#[path = "http_sse/tests.rs"]
mod tests;
use http_affinity::*;
pub(crate) use http_affinity::{
    allowed_audit_event, extract_response_id_from_body, local_auth_rejection_audit_event,
    redacted_account_hash,
};
pub use http_types::*;

fn credential_failure_or_selection_error(
    error: HttpProxyError,
    attempted_accounts: &[AccountId],
) -> HttpProxyError {
    if !attempted_accounts.is_empty()
        && matches!(
            error,
            HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
            }
        )
    {
        return HttpProxyError::ProviderCredential {
            reason: CredentialResolverError::RefreshUnavailable,
        };
    }

    error
}

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
        tracing::error!(
            event.name = "codex_router.proxy.audit_append_failure",
            error.kind = "audit_append_failure",
            "audit append failed"
        );
        eprintln!("{diagnostic}");
    }
}

/// Resolves provider credentials for async Tokio runtime callers.
pub trait AsyncProviderCredentialResolver {
    /// Resolves credentials immediately before provider egress.
    fn resolve_provider_credentials<'a>(
        &'a self,
        account_id: &'a AccountId,
        expected_provider: Provider,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>>;
}

/// Writes one passive Claude quota window through the state-owned observation API.
pub(crate) trait AsyncClaudeQuotaObservationWriter: Send + Sync {
    /// Persists one observation and returns the state write result.
    fn record_window_observation<'a>(
        &'a self,
        observation: WindowObservation,
    ) -> BoxFuture<'a, Result<(), StateStoreError>>;
}

/// Failure while deriving or persisting one passive Claude quota observation.
#[derive(Debug, Error)]
pub(crate) enum PassiveClaudeQuotaObservationError {
    /// The configured interval cannot be represented by the state helper's second precision.
    #[error("quota refresh interval must use whole-second precision")]
    SubsecondRefreshInterval,
    /// The shared state freshness helper rejected the deadline arithmetic.
    #[error(transparent)]
    Freshness(#[from] WindowObservationFreshnessError),
    /// The state layer rejected the observation or could not persist it.
    #[error(transparent)]
    State(#[from] StateStoreError),
}

/// Builds and writes account-wide Claude observations from one response's unified headers.
///
/// The caller supplies the route profile, request-start timestamp, and configured refresh
/// interval. OpenAI profiles and absent or malformed window header pairs produce no write.
/// Returns the number of complete per-window observations submitted to the writer.
pub(crate) async fn record_passive_claude_quota_observations(
    route_profile: &RouteProfile,
    account_id: &AccountId,
    response_headers: &HeaderCollection,
    observation_started_at_unix_seconds: u64,
    refresh_interval: Duration,
    writer: &dyn AsyncClaudeQuotaObservationWriter,
) -> Result<usize, PassiveClaudeQuotaObservationError> {
    let observations = passive_claude_quota_observations(
        route_profile,
        account_id,
        response_headers,
        observation_started_at_unix_seconds,
        refresh_interval,
    )?;
    let observation_count = observations.len();
    for observation in observations {
        writer.record_window_observation(observation).await?;
    }
    Ok(observation_count)
}

fn passive_claude_quota_observations(
    route_profile: &RouteProfile,
    account_id: &AccountId,
    response_headers: &HeaderCollection,
    observation_started_at_unix_seconds: u64,
    refresh_interval: Duration,
) -> Result<Vec<WindowObservation>, PassiveClaudeQuotaObservationError> {
    if route_profile.provider != Provider::Claude {
        return Ok(Vec::new());
    }
    if refresh_interval.subsec_nanos() != 0 {
        return Err(PassiveClaudeQuotaObservationError::SubsecondRefreshInterval);
    }

    let fresh_until_unix_seconds = calculate_window_observation_fresh_until_unix_seconds(
        observation_started_at_unix_seconds,
        refresh_interval.as_secs(),
    )?;
    let mut observations = Vec::with_capacity(2);
    for (window_kind, header_name) in [(WindowKind::FiveHour, "5h"), (WindowKind::Weekly, "7d")] {
        let utilization_header_name =
            format!("anthropic-ratelimit-unified-{header_name}-utilization");
        let reset_header_name = format!("anthropic-ratelimit-unified-{header_name}-reset");
        let (Some(utilization_header), Some(reset_header)) = (
            response_headers.value(&utilization_header_name),
            response_headers.value(&reset_header_name),
        ) else {
            continue;
        };
        let Ok(utilization) = utilization_header.trim().parse::<f64>() else {
            continue;
        };
        let Ok(reset_unix_seconds) = reset_header.trim().parse::<u64>() else {
            continue;
        };
        if !utilization.is_finite() || utilization < 0.0 {
            continue;
        }

        let remaining_basis_points = ((1.0 - utilization).max(0.0) * 10_000.0).round() as u32;
        let observation = WindowObservation::new(
            WindowObservationProps::new(
                account_id.clone(),
                window_kind,
                remaining_basis_points,
                observation_started_at_unix_seconds,
            )
            .with_reset_unix_seconds(reset_unix_seconds)
            .with_fresh_until_unix_seconds(fresh_until_unix_seconds),
        )?;
        observations.push(observation);
    }
    Ok(observations)
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

        Ok(UpstreamHttpRequest::new(
            request.method,
            original_path,
            route_kind,
            upstream_request.headers().clone(),
            upstream_request.body().to_vec(),
        ))
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
    provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
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
            provider_error_observer: None,
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

    /// Adds the provider-error observer used for quota-exhaustion accounting.
    #[must_use]
    pub fn with_provider_error_observer(
        mut self,
        provider_error_observer: Arc<dyn AsyncProviderErrorObserver>,
    ) -> Self {
        self.provider_error_observer = Some(provider_error_observer);
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
        let route_kind = request_route_kind(&request)?;
        let route_profile = http_route_profile_for_kind(route_kind);
        let route_band = route_kind.route_band();
        let affinity_secret = self.load_affinity_secret_for_request(&request)?;
        let mut selection_request = request;
        let mut attempted_accounts = Vec::new();
        loop {
            let selected = match self.selector.select_upstream_account(
                &selection_request,
                token_generation,
                affinity_secret.as_ref(),
            ) {
                Ok(selected) => selected,
                Err(error) => {
                    self.emit_audit_event(http_selection_rejection_audit_event(audit_route_kind));
                    return Err(credential_failure_or_selection_error(
                        error,
                        &attempted_accounts,
                    ));
                }
            };
            if attempted_accounts
                .iter()
                .any(|account_id| account_id == selected.account_id())
            {
                return Err(HttpProxyError::ProviderCredential {
                    reason: CredentialResolverError::RefreshUnavailable,
                });
            }
            let account_hash = redacted_account_hash(selected.account_id());
            let resolved = match self
                .credential_resolver
                .resolve_provider_credentials(selected.account_id(), route_profile.provider)
            {
                Ok(resolved) => resolved,
                Err(reason) => {
                    self.emit_audit_event(http_credential_rejection_audit_event(
                        audit_route_kind,
                        account_hash.clone(),
                    ));
                    if selected.selection_reason() == "previous_response_affinity"
                        || attempted_accounts.len() + 1 >= REQUEST_LOCAL_CREDENTIAL_ATTEMPT_LIMIT
                    {
                        return Err(HttpProxyError::ProviderCredential { reason });
                    }
                    tracing::warn!(
                        account.hash = account_hash.as_str(),
                        route.kind = ?audit_route_kind,
                        "codex_router.http_streaming_credential_attempt_failed_retrying_next_account"
                    );
                    attempted_accounts.push(selected.account_id().clone());
                    selection_request =
                        selection_request.with_excluded_account(selected.account_id().clone());
                    continue;
                }
            };
            let upstream_request = self.proxy.build_upstream_request(
                selection_request,
                resolved.access_token().clone(),
                resolved.chatgpt_account_id(),
            )?;
            let completion = StreamingHttpProxyCompletion {
                affinity_secret,
                account_id: selected.account_id().clone(),
                route_band,
                credential_generation: resolved.credential_generation(),
                allowed_audit_event: allowed_audit_event(
                    TransportKind::Http,
                    audit_route_kind,
                    account_hash,
                ),
                active_reservation_guard: selected.active_reservation_guard().cloned(),
                provider_error_observer: self.provider_error_observer.clone(),
            };

            return Ok(PreparedStreamingHttpProxyRequest {
                upstream_request,
                completion,
            });
        }
    }
}

impl<T, S, C> AuthenticatedHttpProxyService<'_, T, S, C>
where
    S: AsyncAccountDecisionSelector,
    C: AsyncProviderCredentialResolver,
{
    /// Prepares one sanitized upstream HTTP/SSE request without blocking on
    /// request-time account selection or credential resolution.
    pub async fn prepare_streaming_request_async(
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
        let route_kind = request_route_kind(&request)?;
        let route_profile = http_route_profile_for_kind(route_kind);
        let route_band = route_kind.route_band();
        let affinity_secret = self.load_affinity_secret_for_request(&request)?;
        let mut selection_request = request;
        let mut attempted_accounts = Vec::new();
        loop {
            let selected = match self
                .selector
                .select_upstream_account(
                    &selection_request,
                    token_generation,
                    affinity_secret.as_ref(),
                )
                .await
            {
                Ok(selected) => selected,
                Err(error) => {
                    self.emit_audit_event(http_selection_rejection_audit_event(audit_route_kind));
                    return Err(credential_failure_or_selection_error(
                        error,
                        &attempted_accounts,
                    ));
                }
            };
            if attempted_accounts
                .iter()
                .any(|account_id| account_id == selected.account_id())
            {
                return Err(HttpProxyError::ProviderCredential {
                    reason: CredentialResolverError::RefreshUnavailable,
                });
            }
            let account_hash = redacted_account_hash(selected.account_id());
            let resolved = match self
                .credential_resolver
                .resolve_provider_credentials(selected.account_id(), route_profile.provider)
                .await
            {
                Ok(resolved) => resolved,
                Err(reason) => {
                    self.emit_audit_event(http_credential_rejection_audit_event(
                        audit_route_kind,
                        account_hash.clone(),
                    ));
                    if selected.selection_reason() == "previous_response_affinity"
                        || attempted_accounts.len() + 1 >= REQUEST_LOCAL_CREDENTIAL_ATTEMPT_LIMIT
                    {
                        return Err(HttpProxyError::ProviderCredential { reason });
                    }
                    tracing::warn!(
                        account.hash = account_hash.as_str(),
                        route.kind = ?audit_route_kind,
                        "codex_router.async_http_streaming_credential_attempt_failed_retrying_next_account"
                    );
                    attempted_accounts.push(selected.account_id().clone());
                    selection_request =
                        selection_request.with_excluded_account(selected.account_id().clone());
                    continue;
                }
            };
            let upstream_request = self.proxy.build_upstream_request(
                selection_request,
                resolved.access_token().clone(),
                resolved.chatgpt_account_id(),
            )?;
            let completion = StreamingHttpProxyCompletion {
                affinity_secret,
                account_id: selected.account_id().clone(),
                route_band,
                credential_generation: resolved.credential_generation(),
                allowed_audit_event: allowed_audit_event(
                    TransportKind::Http,
                    audit_route_kind,
                    account_hash,
                ),
                active_reservation_guard: selected.active_reservation_guard().cloned(),
                provider_error_observer: self.provider_error_observer.clone(),
            };

            return Ok(PreparedStreamingHttpProxyRequest {
                upstream_request,
                completion,
            });
        }
    }

    /// Prepares one sanitized upstream HTTP/SSE request with a Hyper-owned body
    /// stream. The request DTO carries only bounded routing/affinity metadata.
    pub async fn prepare_async_streaming_request_async(
        &self,
        request: HttpProxyRequest,
        body: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> Result<PreparedAsyncStreamingHttpProxyRequest, HttpProxyError> {
        let prepared = self.prepare_streaming_request_async(request).await?;
        let (upstream_request, completion) = prepared.into_parts();

        Ok(PreparedAsyncStreamingHttpProxyRequest {
            upstream_request: upstream_request.into_streaming_body(body),
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
        let route_profile = http_route_profile_for_kind(request_route_kind(&request)?);
        let affinity_secret = self.load_affinity_secret_for_request(&request)?;
        let mut selection_request = request;
        let mut attempted_accounts = Vec::new();
        loop {
            let selected = match self.selector.select_upstream_account(
                &selection_request,
                token_generation,
                affinity_secret.as_ref(),
            ) {
                Ok(selected) => selected,
                Err(error) => {
                    self.emit_audit_event(http_selection_rejection_audit_event(audit_route_kind));
                    return Err(credential_failure_or_selection_error(
                        error,
                        &attempted_accounts,
                    ));
                }
            };
            if attempted_accounts
                .iter()
                .any(|account_id| account_id == selected.account_id())
            {
                return Err(HttpProxyError::ProviderCredential {
                    reason: CredentialResolverError::RefreshUnavailable,
                });
            }
            let account_hash = redacted_account_hash(selected.account_id());
            let resolved = match self
                .credential_resolver
                .resolve_provider_credentials(selected.account_id(), route_profile.provider)
            {
                Ok(resolved) => resolved,
                Err(reason) => {
                    self.emit_audit_event(http_credential_rejection_audit_event(
                        audit_route_kind,
                        account_hash.clone(),
                    ));
                    if selected.selection_reason() == "previous_response_affinity"
                        || attempted_accounts.len() + 1 >= REQUEST_LOCAL_CREDENTIAL_ATTEMPT_LIMIT
                    {
                        return Err(HttpProxyError::ProviderCredential { reason });
                    }
                    tracing::warn!(
                        account.hash = account_hash.as_str(),
                        route.kind = ?audit_route_kind,
                        "codex_router.http_credential_attempt_failed_retrying_next_account"
                    );
                    attempted_accounts.push(selected.account_id().clone());
                    selection_request =
                        selection_request.with_excluded_account(selected.account_id().clone());
                    continue;
                }
            };

            let response = self.proxy.handle(
                selection_request,
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

            return Ok(response);
        }
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
