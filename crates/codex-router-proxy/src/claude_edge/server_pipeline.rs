//! Claude Messages orchestration over the listener's existing runtime resources.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;

use bytes::Bytes;
use bytes::BytesMut;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_core::attempt_outcome::{AttemptOutcome, PassThroughReason, TransportFailure};
use codex_router_core::audit::{AuditFileSink, RouteKind as AuditRouteKind, TransportKind};
use codex_router_core::ids::AccountId;
use codex_router_core::ids::TokenGeneration;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::safe_account_label;
use codex_router_core::route_profile::CLAUDE_MESSAGES;
use codex_router_core::route_profile::ClaudeFiveHourReservePercent;
use codex_router_core::routes::RouteBand;
use codex_router_selection::selection_outcome::CredentialStoreAvailability;
use codex_router_selection::selection_outcome::UnavailableReason;
use codex_router_selection::selection_outcome::classify_unavailable_reason;
use codex_router_state::selection_projection::project_route_band_selection_inputs_read_only;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use codex_router_state::window_observation::{
    WindowObservation, WindowRejection, WindowRejectionProps,
};
use futures_util::future::BoxFuture;
use http::{Request as HttpRequest, Response as HttpResponse, StatusCode};
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper::body::Body;
use hyper::body::Frame;
use hyper::body::Incoming;
use std::time::Duration;
use tokio_util::task::TaskTracker;

use crate::account_selection::{
    AsyncAccountDecisionSelector, AsyncAccountSelectorRuntimeState,
    AsyncRepositoryBackedAccountSelector, DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
    SelectedAccountDecision, SqliteActiveClientLeaseReporter,
};
use crate::credential_runtime::AsyncProxyCredentialResolver;
use crate::db_write_actor::DbWriteActor;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::http_sse::{
    AsyncClaudeQuotaObservationWriter, AsyncHttpBodyError, AsyncStreamingHttpProxyResponse,
    AsyncStreamingUpstreamHttpTransport, HttpProxyError, HttpProxyRequest,
    StderrAuditFailureReporter, allowed_audit_event, append_audit_event_with_reporter,
    record_passive_claude_quota_observations, redacted_account_hash,
};
use crate::local_auth::{ProxyLocalAuthGate, extract_presented_local_token_from_request};
use crate::server::{
    CapturedClaudeProviderErrorResponse, box_body_from_bytes,
    claude_selection_unavailable_response, empty_response, hold_active_reservation_until_body_drop,
    http_error_response, incoming_body_error, method_from_hyper,
};
use crate::session_account_affinity_cache::{
    SharedSessionAccountAffinityCache, publish_claude_session_account_affinity_on_success,
    release_claude_session_account_affinity,
};
use crate::upstream::HyperHttpUpstreamTransport;

use super::attempt_loop::{ClaudeAttemptPipeline, run_at_most_two};
use super::classifier::classify as classify_claude_outcome;
use super::forward::{
    BufferedClaudeRequestBody, CLAUDE_ERROR_EVIDENCE_LIMIT, ClaudeEdge, ErrorBodyEvidence,
    buffer_request_body,
};

static NEXT_CLAUDE_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Route-local adapter; store roles preserve the listener's read/write ownership.
pub(crate) struct ClaudeServerRuntime {
    pub(crate) auth_gate: Option<ProxyLocalAuthGate>,
    pub(crate) upstream: HyperHttpUpstreamTransport,
    pub(crate) selection_state_store: AsyncSqliteStateStore,
    pub(crate) provider_error_state_store: AsyncSqliteStateStore,
    pub(crate) credential_resolver: AsyncProxyCredentialResolver,
    pub(crate) credential_store_availability: CredentialStoreAvailability,
    pub(crate) selector_runtime_state: AsyncAccountSelectorRuntimeState,
    pub(crate) session_affinity_cache: SharedSessionAccountAffinityCache,
    pub(crate) claude_five_hour_reserve_percent: ClaudeFiveHourReservePercent,
    pub(crate) quota_refresh_interval: Duration,
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
        let request_started_at_unix_seconds = (self.clock)();
        let (parts, body) = request.into_parts();
        let path = parts
            .uri
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str);
        let mut request = HttpProxyRequest::new(method_from_hyper(&parts.method), path);
        for (name, value) in &parts.headers {
            request = request.with_header(Header::from_http(name.as_str(), value));
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
        let pipeline = ClaudeServerAttemptPipeline {
            handler: self,
            request,
            token_generation,
            request_started_at_unix_seconds,
            quota_refresh_interval: self.quota_refresh_interval,
            credential_resolver: self.credential_resolver.clone(),
            request_sequence: NEXT_CLAUDE_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            first_attempt_response_capture: FirstAttemptResponseCapture::default(),
        };
        if !matches!(
            self.credential_store_availability,
            CredentialStoreAvailability::Available
        ) {
            let error = HttpProxyError::Selection {
                reason:
                    crate::account_selection::QuotaAwareAccountSelectorError::NoEligibleAccounts,
            };
            pipeline.log_attempt_failure(1, failure_class_from_proxy_error(&error));
            pipeline.log_attempts_completed(0, "not_attempted");
            return pipeline.response_for_error(error).await;
        }
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
        let first_attempt = match pipeline.select_attempt(&pipeline.request, 1).await {
            Ok(attempt) => attempt,
            Err(error) => {
                pipeline.log_attempt_failure(1, failure_class_from_proxy_error(&error));
                pipeline.log_attempts_completed(0, "not_attempted");
                return pipeline.response_for_error(error).await;
            }
        };
        let result = match run_at_most_two(
            &pipeline,
            first_attempt,
            buffered_body,
            &classify_claude_outcome,
            self.affinity_record_tasks.clone(),
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                pipeline.log_attempts_completed(1, failure_class_from_proxy_error(&error));
                return pipeline.response_for_error(error).await;
            }
        };
        pipeline.log_attempts_completed(
            result.attempts,
            if result.attempts == 2 {
                failure_class_from_outcome(&result.outcome)
            } else {
                "not_attempted"
            },
        );
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
    attempt_number: u8,
}

struct ClaudeServerAttemptPipeline<'a> {
    handler: &'a ClaudeServerRuntime,
    request: HttpProxyRequest,
    token_generation: TokenGeneration,
    request_started_at_unix_seconds: u64,
    quota_refresh_interval: Duration,
    credential_resolver: AsyncProxyCredentialResolver,
    request_sequence: u64,
    first_attempt_response_capture: FirstAttemptResponseCapture,
}

#[derive(Clone, Default)]
struct FirstAttemptResponseCapture {
    response: Arc<Mutex<Option<CapturedClaudeProviderErrorResponse>>>,
}

impl FirstAttemptResponseCapture {
    fn retain(&self, response: CapturedClaudeProviderErrorResponse) {
        if let Ok(mut captured_response) = self.response.lock() {
            *captured_response = Some(response);
        }
    }

    fn take(&self) -> Option<CapturedClaudeProviderErrorResponse> {
        self.response
            .lock()
            .ok()
            .and_then(|mut response| response.take())
    }
}

struct CapturingProviderErrorBody {
    body: BoxBody<Bytes, AsyncHttpBodyError>,
    response_capture: FirstAttemptResponseCapture,
    status: u16,
    headers: HeaderCollection,
    captured_body: Vec<Bytes>,
    evidence_prefix: BytesMut,
    evidence_complete: bool,
    finished: bool,
}

impl CapturingProviderErrorBody {
    fn new(
        body: BoxBody<Bytes, AsyncHttpBodyError>,
        response_capture: FirstAttemptResponseCapture,
        status: u16,
        headers: HeaderCollection,
    ) -> Self {
        Self {
            body,
            response_capture,
            status,
            headers,
            captured_body: Vec::new(),
            evidence_prefix: BytesMut::new(),
            evidence_complete: true,
            finished: false,
        }
    }

    fn record_frame(&mut self, frame: &Frame<Bytes>) {
        let Some(bytes) = frame.data_ref() else {
            return;
        };
        self.captured_body.push(bytes.clone());
        let remaining = CLAUDE_ERROR_EVIDENCE_LIMIT.saturating_sub(self.evidence_prefix.len());
        if bytes.len() > remaining {
            if let Some(prefix) = bytes.get(..remaining) {
                self.evidence_prefix.extend_from_slice(prefix);
            }
            self.evidence_complete = false;
        } else if self.evidence_complete {
            self.evidence_prefix.extend_from_slice(bytes);
        }
    }

    fn finish(&mut self, complete: bool) {
        if self.finished {
            return;
        }
        self.finished = true;
        if complete {
            self.response_capture
                .retain(CapturedClaudeProviderErrorResponse {
                    status: self.status,
                    headers: self.headers.clone(),
                    body: std::mem::take(&mut self.captured_body),
                    evidence_prefix: self.evidence_prefix.split().freeze(),
                    evidence_complete: self.evidence_complete,
                });
        } else {
            self.captured_body.clear();
        }
    }
}

impl Body for CapturingProviderErrorBody {
    type Data = Bytes;
    type Error = AsyncHttpBodyError;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.body).poll_frame(context) {
            Poll::Ready(Some(Ok(frame))) => {
                this.record_frame(&frame);
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.finish(false);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                this.finish(true);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

struct StateBackedClaudeQuotaObservationWriter {
    state: AsyncSqliteStateStore,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl AsyncClaudeQuotaObservationWriter for StateBackedClaudeQuotaObservationWriter {
    fn record_window_observation<'a>(
        &'a self,
        observation: WindowObservation,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        let state = self.state.clone();
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            state
                .record_window_observation(&observation, || clock())
                .await
                .map(|_observation_was_newest| ())
        })
    }
}

fn schedule_passive_claude_quota_observation(
    response: AsyncStreamingHttpProxyResponse,
    writer: Arc<dyn AsyncClaudeQuotaObservationWriter>,
    account_id: AccountId,
    request_started_at_unix_seconds: u64,
    quota_refresh_interval: Duration,
    affinity_record_tasks: TaskTracker,
) -> AsyncStreamingHttpProxyResponse {
    let response_headers = response.headers().clone();
    affinity_record_tasks.spawn(async move {
        if let Err(error) = record_passive_claude_quota_observations(
            &CLAUDE_MESSAGES,
            &account_id,
            &response_headers,
            request_started_at_unix_seconds,
            quota_refresh_interval,
            writer.as_ref(),
        )
        .await
        {
            tracing::warn!(error = %error, "codex_router.claude_passive_quota_observation_failed");
        }
    });
    response
}

impl ClaudeServerAttemptPipeline<'_> {
    async fn select_attempt(
        &self,
        request: &HttpProxyRequest,
        attempt_number: u8,
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
        let attempt = ClaudeServerAttempt {
            selected,
            credential,
            attempt_number,
        };
        self.log_attempt_selected(&attempt);
        Ok(attempt)
    }

    fn log_attempt_selected(&self, attempt: &ClaudeServerAttempt) {
        log_claude_attempt_selection(
            self.request_sequence,
            attempt.attempt_number,
            &attempt.selected,
        );
    }

    fn log_attempt_failure(&self, attempt_number: u8, failure_class: &'static str) {
        tracing::info!(
            request.sequence = self.request_sequence,
            attempt.number = attempt_number,
            attempt.failure_class = failure_class,
            "codex_router.claude_attempt_failed"
        );
    }

    fn log_attempts_completed(&self, attempts: u8, attempt_2_failure_class: &'static str) {
        log_claude_attempts_completed(self.request_sequence, attempts, attempt_2_failure_class);
    }

    async fn response_for_error(
        &self,
        error: HttpProxyError,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        let no_account_selection = matches!(
            &error,
            HttpProxyError::Selection {
                reason: crate::account_selection::QuotaAwareAccountSelectorError::NoEligibleAccounts
                    | crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable
            }
        );
        if !no_account_selection {
            return http_error_response(error);
        }

        let now_unix_seconds = (self.handler.clock)();
        let Ok(projection) = project_route_band_selection_inputs_read_only(
            &self.handler.selection_state_store,
            RouteBand::ClaudeMessages.as_str(),
            now_unix_seconds,
            // R12 uses credential/quota restrictions; active load does not affect its reason.
            0,
        )
        .await
        else {
            return http_error_response(error);
        };
        let reason = classify_unavailable_reason(
            Provider::Claude,
            self.handler.credential_store_availability,
            projection.account_states(),
        );
        if matches!(
            &reason,
            UnavailableReason::HeldByFloors { accounts } if accounts.is_empty()
        ) {
            return http_error_response(error);
        }
        let Ok(accounts) = self.handler.selection_state_store.list_accounts().await else {
            return http_error_response(error);
        };
        let account_labels = accounts
            .iter()
            .filter(|account| account.provider() == Provider::Claude)
            .map(|account| {
                (
                    account.account_id().clone(),
                    safe_account_label(account.label(), account.account_id()).to_string(),
                )
            })
            .collect::<Vec<_>>();
        let provider_usage_limit_response =
            if matches!(&reason, UnavailableReason::AllExhausted { .. }) {
                self.first_attempt_response_capture
                    .take()
                    .filter(|response| {
                        matches!(
                            classify_claude_outcome(
                                response.status,
                                &response.headers,
                                ErrorBodyEvidence {
                                    prefix: &response.evidence_prefix,
                                    complete: response.evidence_complete,
                                },
                            ),
                            AttemptOutcome::SharedWindowExhausted { .. }
                        )
                    })
            } else {
                None
            };
        claude_selection_unavailable_response(
            reason,
            &account_labels,
            now_unix_seconds,
            provider_usage_limit_response,
        )
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

fn log_claude_attempt_selection(
    request_sequence: u64,
    attempt_number: u8,
    selected: &SelectedAccountDecision,
) {
    let selection_mode = if selected.selection_reason() == "prompt_cache_account_affinity" {
        "pin"
    } else {
        "fresh"
    };
    tracing::info!(
        request.sequence = request_sequence,
        attempt.number = attempt_number,
        account.hash = redacted_account_hash(selected.account_id()),
        selection.mode = selection_mode,
        "codex_router.claude_attempt_selected"
    );
}

fn log_claude_attempts_completed(
    request_sequence: u64,
    attempts: u8,
    attempt_2_failure_class: &'static str,
) {
    tracing::info!(
        request.sequence = request_sequence,
        attempts,
        attempt_2.failure_class = attempt_2_failure_class,
        "codex_router.claude_attempts_completed"
    );
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
            let response = self
                .handler
                .upstream
                .send_streaming(request)
                .await
                .map_err(|_error| TransportFailure::Connection)?;
            let observation_writer = Arc::new(StateBackedClaudeQuotaObservationWriter {
                state: self.handler.provider_error_state_store.clone(),
                clock: Arc::clone(&self.handler.clock),
            });
            let response = schedule_passive_claude_quota_observation(
                response,
                observation_writer,
                attempt.selected.account_id().clone(),
                self.request_started_at_unix_seconds,
                self.quota_refresh_interval,
                self.handler.affinity_record_tasks.clone(),
            );
            if attempt.attempt_number == 1 && response.status() == StatusCode::TOO_MANY_REQUESTS {
                let (status, headers, body) = response.into_parts();
                let body = CapturingProviderErrorBody::new(
                    body,
                    self.first_attempt_response_capture.clone(),
                    status,
                    headers.clone(),
                )
                .boxed();
                Ok(AsyncStreamingHttpProxyResponse::new(status, headers, body))
            } else {
                Ok(response)
            }
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
                    let second_attempt = ClaudeServerAttempt {
                        selected: attempt.selected.clone(),
                        credential,
                        attempt_number: 2,
                    };
                    self.log_attempt_selected(&second_attempt);
                    return Ok(second_attempt);
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
            self.select_attempt(&request, 2).await.inspect_err(|error| {
                self.log_attempt_failure(2, failure_class_from_proxy_error(error));
            })
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

fn failure_class_from_outcome(outcome: &AttemptOutcome) -> &'static str {
    match outcome {
        AttemptOutcome::Success => "success",
        AttemptOutcome::SharedWindowExhausted { .. } => "shared_window_exhausted",
        AttemptOutcome::CredentialRejected => "credential_rejected",
        AttemptOutcome::PassThrough(reason) => match reason {
            PassThroughReason::Overloaded => "provider_overloaded",
            PassThroughReason::ServerError => "provider_server_error",
            PassThroughReason::RateThrottled => "request_rate_limited",
            PassThroughReason::ModelOrOverageLimit => "model_or_overage_limited",
            PassThroughReason::RequestRejected => "request_rejected",
            PassThroughReason::UnattributedLimit => "unattributed_limit",
            PassThroughReason::MalformedEvidence => "malformed_evidence",
        },
        AttemptOutcome::NoResponse(reason) => match reason {
            TransportFailure::Connection => "connection_failure",
            TransportFailure::Timeout => "timeout",
        },
    }
}

fn failure_class_from_proxy_error(error: &HttpProxyError) -> &'static str {
    match error {
        HttpProxyError::LocalAuth { .. } => "local_auth_rejected",
        HttpProxyError::Rejected { .. } => "request_rejected",
        HttpProxyError::Upstream { .. } => "upstream_failure",
        HttpProxyError::ProviderCredential { .. } => "credential_unavailable",
        HttpProxyError::Selection { reason } => match reason {
            crate::account_selection::QuotaAwareAccountSelectorError::NoEligibleAccounts => {
                "no_eligible_account"
            }
            crate::account_selection::QuotaAwareAccountSelectorError::ShortQuotaExhausted {
                ..
            } => "short_quota_exhausted",
            crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable => {
                "selection_state_unavailable"
            }
            crate::account_selection::QuotaAwareAccountSelectorError::SelectorStateUnavailable => {
                "selector_state_unavailable"
            }
            crate::account_selection::QuotaAwareAccountSelectorError::SecretUnavailable => {
                "affinity_secret_unavailable"
            }
            crate::account_selection::QuotaAwareAccountSelectorError::MalformedAffinityKey => {
                "malformed_affinity_key"
            }
            crate::account_selection::QuotaAwareAccountSelectorError::AffinityOwnerMissing => {
                "affinity_owner_missing"
            }
            crate::account_selection::QuotaAwareAccountSelectorError::AffinityOwnerUnavailable => {
                "affinity_owner_unavailable"
            }
        },
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

#[cfg(test)]
#[path = "server_pipeline_r12_tests.rs"]
mod r12_tests;
