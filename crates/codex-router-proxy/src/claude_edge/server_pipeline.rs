//! Claude Messages orchestration over the listener's existing runtime resources.

use std::sync::Arc;

use bytes::Bytes;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_core::attempt_outcome::{AttemptOutcome, TransportFailure};
use codex_router_core::audit::{AuditFileSink, RouteKind as AuditRouteKind, TransportKind};
use codex_router_core::ids::TokenGeneration;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::ClaudeFiveHourReservePercent;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::window_observation::{WindowRejection, WindowRejectionProps};
use futures_util::future::BoxFuture;
use http::{Request as HttpRequest, Response as HttpResponse, StatusCode};
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use tokio_util::task::TaskTracker;

use crate::account_selection::{
    AsyncAccountDecisionSelector, AsyncAccountSelectorRuntimeState,
    AsyncRepositoryBackedAccountSelector, DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
    SelectedAccountDecision, SqliteActiveClientLeaseReporter,
};
use crate::credential_runtime::AsyncProxyCredentialResolver;
use crate::db_write_actor::DbWriteActor;
use crate::headers::Header;
use crate::http_sse::{
    AsyncHttpBodyError, AsyncStreamingHttpProxyResponse, AsyncStreamingUpstreamHttpTransport,
    HttpProxyError, HttpProxyRequest, StderrAuditFailureReporter, allowed_audit_event,
    append_audit_event_with_reporter, redacted_account_hash,
};
use crate::local_auth::{ProxyLocalAuthGate, extract_presented_local_token_from_request};
use crate::server::{
    box_body_from_bytes, empty_response, hold_active_reservation_until_body_drop,
    http_error_response, incoming_body_error, method_from_hyper,
};
use crate::session_account_affinity_cache::{
    SharedSessionAccountAffinityCache, publish_claude_session_account_affinity_on_success,
    release_claude_session_account_affinity,
};
use crate::upstream::HyperHttpUpstreamTransport;

use super::attempt_loop::{ClaudeAttemptPipeline, run_at_most_two};
use super::classifier::classify as classify_claude_outcome;
use super::forward::{BufferedClaudeRequestBody, ClaudeEdge, buffer_request_body};

/// Route-local adapter; store roles preserve the listener's read/write ownership.
pub(crate) struct ClaudeServerRuntime {
    pub(crate) auth_gate: Option<ProxyLocalAuthGate>,
    pub(crate) upstream: HyperHttpUpstreamTransport,
    pub(crate) selection_state_store: AsyncSqliteStateStore,
    pub(crate) provider_error_state_store: AsyncSqliteStateStore,
    pub(crate) credential_resolver: AsyncProxyCredentialResolver,
    pub(crate) selector_runtime_state: AsyncAccountSelectorRuntimeState,
    pub(crate) session_affinity_cache: SharedSessionAccountAffinityCache,
    pub(crate) claude_five_hour_reserve_percent: ClaudeFiveHourReservePercent,
    pub(crate) db_write_actor: DbWriteActor,
    pub(crate) clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub(crate) affinity_record_tasks: TaskTracker,
    pub(crate) audit_sink: Option<AuditFileSink>,
}

impl ClaudeServerRuntime {
    pub(crate) async fn handle_request(
        &self,
        request: HttpRequest<Incoming>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        let (parts, body) = request.into_parts();
        let path = parts
            .uri
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str);
        let mut request = HttpProxyRequest::new(method_from_hyper(&parts.method), path);
        for (name, value) in &parts.headers {
            if let Ok(value) = value.to_str() {
                request = request.with_header(Header::new(name.as_str(), value));
            }
        }
        // Recheck the dedicated gate after admission so token rotation remains authoritative.
        let presented_token = match extract_presented_local_token_from_request(
            request.header_value("x-codex-router-token"),
            request.header_value("authorization"),
            request.header_value("cookie"),
            request.path(),
            &[],
            false,
        ) {
            Ok(token) => token,
            Err(reason) => return http_error_response(HttpProxyError::LocalAuth { reason }),
        };
        let token_generation = match self
            .auth_gate
            .as_ref()
            .ok_or(LocalAuthError::Missing)
            .and_then(|gate| gate.authorize(presented_token))
        {
            Ok(generation) => generation,
            Err(reason) => return http_error_response(HttpProxyError::LocalAuth { reason }),
        };
        let buffered_body = match buffer_request_body(body.map_err(incoming_body_error).boxed())
            .await
        {
            Ok(body) => body,
            Err(error) => {
                return empty_response(
                    StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::BAD_REQUEST),
                );
            }
        };
        let pipeline = ClaudeServerAttemptPipeline {
            handler: self,
            request,
            token_generation,
            credential_resolver: self.credential_resolver.clone(),
        };
        let first_attempt = match pipeline.select_attempt(&pipeline.request).await {
            Ok(attempt) => attempt,
            Err(error) => return http_error_response(error),
        };
        let result = match run_at_most_two(
            &pipeline,
            first_attempt,
            buffered_body,
            &classify_claude_outcome,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => return http_error_response(error),
        };
        tracing::debug!(attempts = result.attempts, outcome = ?result.outcome, "codex_router.claude_attempts_completed");
        let Some(response) = result.response else {
            return claude_provider_unreachable_response();
        };
        let (response, _outcome, completion) = response.into_parts();
        if let (Some(session_id), Some(observation), Some(completion)) = (
            pipeline
                .request
                .header_value("x-claude-code-session-id")
                .filter(|session| !session.is_empty()),
            result.attempt.selected.pin_observation(),
            completion,
        ) {
            let cache = Arc::clone(&self.session_affinity_cache);
            let session_id = session_id.to_owned();
            let observation = observation.clone();
            let account_id = result.attempt.selected.account_id().clone();
            let state = self.provider_error_state_store.clone();
            let clock = Arc::clone(&self.clock);
            self.affinity_record_tasks.spawn(async move {
                if let Err(error) = publish_claude_session_account_affinity_on_success(
                    &cache,
                    &session_id,
                    &observation,
                    &account_id,
                    completion,
                    &state,
                    || clock(),
                )
                .await
                {
                    tracing::error!(error = %error, "codex_router.claude_pin_publication_failed");
                }
            });
        }
        if let Some(audit_sink) = &self.audit_sink {
            append_audit_event_with_reporter(
                audit_sink,
                &allowed_audit_event(
                    TransportKind::Http,
                    AuditRouteKind::ClaudeMessages,
                    redacted_account_hash(result.attempt.selected.account_id()),
                ),
                &StderrAuditFailureReporter,
            );
        }
        let (status, headers, body) = response.into_parts();
        let body = hold_active_reservation_until_body_drop(
            body,
            result.attempt.selected.active_reservation_guard().cloned(),
        );
        let mut builder = HttpResponse::builder().status(status);
        for header in headers.as_slice() {
            builder = builder.header(header.name(), header.value());
        }
        builder
            .body(body)
            .unwrap_or_else(|_error| empty_response(StatusCode::BAD_GATEWAY))
    }
}

#[derive(Clone)]
struct ClaudeServerAttempt {
    selected: SelectedAccountDecision,
    credential: ResolvedProviderCredential,
}

struct ClaudeServerAttemptPipeline<'a> {
    handler: &'a ClaudeServerRuntime,
    request: HttpProxyRequest,
    token_generation: TokenGeneration,
    credential_resolver: AsyncProxyCredentialResolver,
}

impl ClaudeServerAttemptPipeline<'_> {
    async fn select_attempt(
        &self,
        request: &HttpProxyRequest,
    ) -> Result<ClaudeServerAttempt, HttpProxyError> {
        let handler = self.handler;
        let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
            &handler.selection_state_store,
            handler.selector_runtime_state.clone(),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            Arc::clone(&handler.clock),
        )
        .with_claude_affinity_writer(&handler.provider_error_state_store)
        .with_claude_five_hour_reserve_percent(handler.claude_five_hour_reserve_percent)
        .with_active_client_lease_reporter(Arc::new(SqliteActiveClientLeaseReporter::new(
            handler.db_write_actor.clone(),
            Arc::clone(&handler.clock),
        )));
        let selected = selector
            .select_upstream_account(request, self.token_generation, None)
            .await?;
        let credential = self
            .credential_resolver
            .resolve_provider_credentials(selected.account_id(), Provider::Claude)
            .await
            .map_err(|reason| HttpProxyError::ProviderCredential { reason })?;
        Ok(ClaudeServerAttempt {
            selected,
            credential,
        })
    }

    async fn record_window_rejections(
        &self,
        attempt: &ClaudeServerAttempt,
        outcome: &AttemptOutcome,
    ) -> Result<(), HttpProxyError> {
        let AttemptOutcome::SharedWindowExhausted { windows, resets } = outcome else {
            return Ok(());
        };
        for (index, window) in windows.iter().enumerate() {
            let mut props = WindowRejectionProps::new(
                attempt.selected.account_id().clone(),
                *window,
                (self.handler.clock)(),
            );
            if let Some(Some(reset)) = resets.get(index) {
                props = props.with_reported_reset(*reset);
            }
            self.handler
                .provider_error_state_store
                .record_window_rejection(&WindowRejection::new(props))
                .await
                .map_err(|_error| claude_selection_state_unavailable())?;
        }
        Ok(())
    }

    async fn release_attempt_pin(
        &self,
        attempt: &ClaudeServerAttempt,
    ) -> Result<(), HttpProxyError> {
        if let (Some(session_id), Some(observation)) = (
            self.request
                .header_value("x-claude-code-session-id")
                .filter(|session| !session.is_empty()),
            attempt.selected.pin_observation(),
        ) {
            release_claude_session_account_affinity(
                &self.handler.session_affinity_cache,
                session_id,
                observation,
                &self.handler.selection_state_store,
                &self.handler.provider_error_state_store,
                (self.handler.clock)(),
            )
            .await
            .map_err(|_error| claude_selection_state_unavailable())?;
        }
        Ok(())
    }
}

impl ClaudeAttemptPipeline for ClaudeServerAttemptPipeline<'_> {
    type Attempt = ClaudeServerAttempt;

    fn send<'a>(
        &'a self,
        attempt: &'a Self::Attempt,
        body: Bytes,
    ) -> BoxFuture<'a, Result<AsyncStreamingHttpProxyResponse, TransportFailure>> {
        Box::pin(async move {
            let body = BufferedClaudeRequestBody::new(body)
                .map_err(|_error| TransportFailure::Connection)?;
            let request = ClaudeEdge::prepare_upstream(&self.request, &body, &attempt.credential)
                .map_err(|_error| TransportFailure::Connection)?;
            self.handler
                .upstream
                .send_streaming(request)
                .await
                .map_err(|_error| TransportFailure::Connection)
        })
    }

    fn recover_first<'a>(
        &'a self,
        attempt: &'a Self::Attempt,
        outcome: &'a AttemptOutcome,
    ) -> BoxFuture<'a, Result<Self::Attempt, HttpProxyError>> {
        Box::pin(async move {
            if matches!(outcome, AttemptOutcome::CredentialRejected) {
                if let Ok((credential, _provider_used)) = self
                    .credential_resolver
                    .recover_unauthorized_credentials(
                        attempt.selected.account_id(),
                        Provider::Claude,
                        attempt.credential.credential_generation(),
                    )
                    .await
                {
                    return Ok(ClaudeServerAttempt {
                        selected: attempt.selected.clone(),
                        credential,
                    });
                }
                // Renewal owns provider refusal/uncertainty and local failure disposition.
            } else {
                self.record_window_rejections(attempt, outcome).await?;
            }
            self.release_attempt_pin(attempt).await?;
            if let Some(reservation) = attempt.selected.active_reservation_guard() {
                reservation.release();
            }
            let request = self
                .request
                .clone()
                .with_excluded_account(attempt.selected.account_id().clone());
            self.select_attempt(&request).await
        })
    }

    fn record_final<'a>(
        &'a self,
        attempt: &'a Self::Attempt,
        outcome: &'a AttemptOutcome,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let result = if matches!(outcome, AttemptOutcome::CredentialRejected) {
                self.credential_resolver
                    .mark_generation_reauth_required(
                        attempt.selected.account_id(),
                        attempt.credential.credential_generation(),
                    )
                    .await
                    .map(|_marked| ())
                    .map_err(|reason| HttpProxyError::ProviderCredential { reason })
            } else {
                self.record_window_rejections(attempt, outcome).await
            };
            if let Err(error) = result {
                tracing::error!(error = %error, "codex_router.claude_final_restriction_failed");
            }
        })
    }
}

fn claude_selection_state_unavailable() -> HttpProxyError {
    HttpProxyError::Selection {
        reason: crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
    }
}

fn claude_provider_unreachable_response() -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    HttpResponse::builder().status(StatusCode::BAD_GATEWAY)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(box_body_from_bytes(br#"{"type":"error","error":{"type":"provider_unreachable","message":"Claude provider could not be reached."}}"#.to_vec()))
        .unwrap_or_else(|_error| empty_response(StatusCode::BAD_GATEWAY))
}

#[cfg(test)]
#[path = "server_pipeline_tests.rs"]
mod tests;
