#[derive(Clone)]
struct LoopbackProtocolConnectionHandler {
    credential_state_store: AsyncSqliteStateStore,
    provider_error_state_store: AsyncSqliteStateStore,
    selection_state_store: AsyncSqliteStateStore,
    credential_store_availability: CredentialStoreAvailability,
    credential_factory: AsyncProxyCredentialResolverFactory,
    affinity_secret_provider: RuntimeAffinitySecretProvider,
    affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    affinity_record_tasks: TaskTracker,
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    claude_edge_auth_gate: Option<crate::local_auth::ProxyLocalAuthGate>,
    claude_edge_runtime_config: Option<ClaudeEdgeRuntimeConfig>,
    local_model_authentication_required: bool,
    upstream: HyperHttpUpstreamTransport,
    upstream_endpoint: UpstreamEndpoint,
    websocket_revocations: WebSocketRevocationRegistry,
    audit_sink: Option<AuditFileSink>,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    active_reservations: RouteBandReservationBooks,
    selection_reservation_lock: SelectionReservationLock,
    session_affinity_cache: SharedSessionAccountAffinityCache,
    claude_five_hour_reserve_percent: ClaudeFiveHourReservePercent,
    runtime_exhaustions: RouteBandRuntimeExhaustions,
    route_band_queue_health: RouteBandQueueHealth,
    db_write_actor: DbWriteActor,
    fixed_now_unix_seconds: Option<u64>,
    session_shutdown: CancellationToken,
}

type UpgradeTaskResult = Result<(), LoopbackRouterRuntimeError>;
type UpgradeTaskHandle = tokio::task::JoinHandle<UpgradeTaskResult>;
type SharedUpgradeTasks = Arc<tokio::sync::Mutex<Vec<UpgradeTaskHandle>>>;

impl LoopbackProtocolConnectionHandler {
    async fn handle_hyper_connection(
        self: Arc<Self>,
        stream: tokio::net::TcpStream,
    ) -> Result<(), LoopbackRouterRuntimeError> {
        let local_peer_addr = stream.peer_addr().ok();
        let io = TokioIo::new(stream);
        let service_context = Arc::clone(&self);
        let upgrade_tasks = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let service_upgrade_tasks = Arc::clone(&upgrade_tasks);
        let service = service_fn(move |request: HttpRequest<Incoming>| {
            let request_context = Arc::clone(&service_context);
            let request_upgrade_tasks = Arc::clone(&service_upgrade_tasks);
            async move {
                Ok::<_, Infallible>(
                    request_context
                        .handle_hyper_request(request, request_upgrade_tasks, local_peer_addr)
                        .await,
                )
            }
        });

        let mut http_builder = http1::Builder::new();
        http_builder.half_close(true);
        let serve_result = http_builder
            .serve_connection(io, service)
            .with_upgrades()
            .await
            .map_err(LoopbackRouterRuntimeError::HyperConnection);
        finish_hyper_connection_after_serve_result(serve_result, upgrade_tasks).await
    }

    async fn handle_hyper_request(
        self: Arc<Self>,
        request: HttpRequest<Incoming>,
        upgrade_tasks: SharedUpgradeTasks,
        local_peer_addr: Option<SocketAddr>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        if let Some(response) = router_compatibility_response(
            request.method(),
            request.uri(),
            self.local_model_authentication_required,
        ) {
            return response;
        }
        if let Some(response) = self.preflight_claude_edge_request(&request) {
            return response;
        }
        match HyperProtocolSwitchpoint::classify(request.method(), request.uri(), request.headers())
        {
            HyperProtocolDispatch::WebSocketUpgrade => {
                self.handle_hyper_websocket_request(request, upgrade_tasks, local_peer_addr)
                    .await
            }
            HyperProtocolDispatch::Http => self.handle_hyper_http_request(request).await,
        }
    }

    fn preflight_claude_edge_local_auth(
        &self,
        request: &HttpRequest<Incoming>,
    ) -> Option<HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>>> {
        let path = request
            .uri()
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str);
        let router_token = header_value(request.headers(), "x-codex-router-token");
        let authorization = header_value(request.headers(), "authorization");
        let cookie = header_value(request.headers(), "cookie");
        let presented_token = extract_presented_local_token_from_request(
            router_token.as_deref(),
            authorization.as_deref(),
            cookie.as_deref(),
            path,
            &[],
            false,
        );
        let authorization_result = match presented_token {
            Err(reason) => Err(reason),
            Ok(presented_token) => match &self.claude_edge_auth_gate {
                Some(auth_gate) => auth_gate.authorize(presented_token),
                None => Err(LocalAuthError::Missing),
            },
        };
        match authorization_result {
            Ok(_generation) => None,
            Err(reason) => {
                self.emit_claude_edge_local_auth_rejection(reason);
                Some(empty_response(StatusCode::UNAUTHORIZED))
            }
        }
    }

    fn preflight_claude_edge_request(
        &self,
        request: &HttpRequest<Incoming>,
    ) -> Option<HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>>> {
        let path = request.uri().path();
        if !is_claude_edge_path(path) {
            return None;
        }

        match classify_route(
            method_from_hyper(request.method()),
            path,
            is_websocket_upgrade(request.headers()),
        ) {
            RouteClass::Supported(crate::routes::RouteKind::ClaudeMessages) => {
                self.preflight_claude_edge_local_auth(request)
            }
            RouteClass::Supported(_) | RouteClass::Rejected { .. } => {
                Some(unsupported_claude_path_response(path))
            }
        }
    }

    fn emit_claude_edge_local_auth_rejection(&self, reason: LocalAuthError) {
        if let Some(audit_sink) = &self.audit_sink {
            let event = local_auth_rejection_audit_event(
                TransportKind::Http,
                AuditRouteKind::ClaudeMessages,
                reason,
            );
            append_audit_event_with_reporter(audit_sink, &event, &StderrAuditFailureReporter);
        }
    }

    async fn handle_hyper_websocket_request(
        self: Arc<Self>,
        mut request: HttpRequest<Incoming>,
        upgrade_tasks: SharedUpgradeTasks,
        local_peer_addr: Option<SocketAddr>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        let path = request
            .uri()
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str)
            .to_owned();
        let handshake = websocket_handshake_from_hyper_headers(request.headers());
        if let Some(response) = self.preflight_hyper_websocket_request(&request, &path) {
            return response;
        }
        let (upgrade_response, websocket) =
            match hyper_tungstenite::upgrade(&mut request, Some(router_websocket_config())) {
                Ok(upgrade) => upgrade,
                Err(_error) => return empty_response(StatusCode::BAD_REQUEST),
            };
        let task_context = Arc::clone(&self);
        let upgrade_task = tokio::spawn(async move {
            match websocket.await {
                Ok(local_websocket) => {
                    task_context
                        .handle_hyper_websocket_upgraded(
                            local_websocket,
                            handshake,
                            path,
                            local_peer_addr,
                        )
                        .await
                }
                Err(error) => Err(LoopbackRouterRuntimeError::WebSocket(
                    crate::websocket::WebSocketTunnelError::Transport(error),
                )),
            }
        });
        upgrade_tasks.lock().await.push(upgrade_task);

        upgrade_response.map(|body| {
            body.map_err(|never: Infallible| -> AsyncHttpBodyError { match never {} })
                .boxed()
        })
    }

    async fn handle_hyper_websocket_upgraded(
        self: Arc<Self>,
        local_websocket: hyper_tungstenite::HyperWebsocketStream,
        handshake: WebSocketHandshakeRequest,
        path: String,
        local_peer_addr: Option<SocketAddr>,
    ) -> Result<(), LoopbackRouterRuntimeError> {
        let selection_runtime_state =
            AsyncAccountSelectorRuntimeState::new_with_selection_lock_and_affinity_cache(
                Arc::clone(&self.weighted_selectors),
                Arc::clone(&self.account_holds),
                Arc::clone(&self.active_reservations),
                Arc::clone(&self.runtime_exhaustions),
                Arc::clone(&self.route_band_queue_health),
                Arc::clone(&self.selection_reservation_lock),
                Arc::clone(&self.session_affinity_cache),
            );
        let account_admission_assessor = RuntimeAccountAdmissionAssessor::new(
            self.selection_state_store.clone(),
            &selection_runtime_state,
            self.runtime_clock(),
        );
        let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
            &self.selection_state_store,
            selection_runtime_state,
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            self.runtime_clock(),
        )
        .with_claude_five_hour_reserve_percent(self.claude_five_hour_reserve_percent)
        .with_active_client_lease_reporter(Arc::new(SqliteActiveClientLeaseReporter::new(
            self.db_write_actor.clone(),
            self.runtime_clock(),
        )))
        .with_session_affinity_writer(self.db_write_actor.clone());
        let credential_resolver = self
            .credential_factory
            .resolver_for_state(self.credential_state_store.clone());
        let protocol_router = WebSocketProtocolRouter::new();
        let tunnel = if let Some(audit_sink) = &self.audit_sink {
            AsyncWebSocketTunnel::new_with_audit_sink(
                &self.auth_gate,
                &selector,
                &credential_resolver,
                &protocol_router,
                audit_sink,
            )
        } else {
            AsyncWebSocketTunnel::new(
                &self.auth_gate,
                &selector,
                &credential_resolver,
                &protocol_router,
            )
        }
        .with_revocation_registry(self.websocket_revocations.clone())
        .with_account_admission_assessor(Arc::new(account_admission_assessor))
        .with_session_shutdown(self.session_shutdown.clone())
        .with_affinity_secret_provider(&self.affinity_secret_provider)
        .with_async_affinity_owner_recorder(Arc::clone(&self.affinity_owner_recorder))
        .with_affinity_owner_task_tracker(self.affinity_record_tasks.clone())
        .with_provider_error_observer(Arc::new(AsyncSqliteProviderErrorObserver::new(
            self.provider_error_state_store.clone(),
            self.selection_state_store.clone(),
            Arc::clone(&self.active_reservations),
            Arc::clone(&self.runtime_exhaustions),
            Arc::clone(&self.route_band_queue_health),
            self.db_write_actor.clone(),
        )))
        .with_local_peer_addr(local_peer_addr);
        let upstream_url = self.upstream_endpoint.websocket_url_for_path(&path);
        {
            crate::telemetry::record_websocket_event(RouteBand::Responses.as_str(), "open");
            let open_span = tracing::info_span!(
                "codex_router.websocket_open",
                route.path = sanitize_route_path_for_log(&path),
                peer.present = local_peer_addr.is_some(),
            );
            let _open_span_guard = open_span.enter();
            tracing::info!(
                route.path = sanitize_route_path_for_log(&path),
                peer.present = local_peer_addr.is_some(),
                "codex_router.websocket_open"
            );
        }
        let result = tunnel
            .handle_upgraded_connection(local_websocket, handshake, upstream_url.as_str())
            .await
            .map_err(LoopbackRouterRuntimeError::WebSocket);
        match &result {
            Ok(()) => {
                crate::telemetry::record_websocket_event(RouteBand::Responses.as_str(), "closed");
                let span = tracing::info_span!(
                    "codex_router.websocket_closed",
                    route.path = sanitize_route_path_for_log(&path),
                );
                let _span_guard = span.enter();
                tracing::info!(
                    route.path = sanitize_route_path_for_log(&path),
                    "codex_router.websocket_closed"
                );
            }
            Err(error) => {
                crate::telemetry::record_websocket_event(RouteBand::Responses.as_str(), "failed");
                let span = tracing::warn_span!(
                    "codex_router.websocket_failed",
                    route.path = sanitize_route_path_for_log(&path),
                    error.kind = websocket_runtime_error_kind(error),
                    error = %sanitize_error_for_log(error),
                );
                let _span_guard = span.enter();
                tracing::warn!(
                    route.path = sanitize_route_path_for_log(&path),
                    error.kind = websocket_runtime_error_kind(error),
                    error = %sanitize_error_for_log(error),
                    "codex_router.websocket_failed"
                );
            }
        }
        result
    }

    async fn handle_hyper_http_request(
        self: Arc<Self>,
        request: HttpRequest<Incoming>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        if is_claude_edge_path(request.uri().path()) {
            return self.handle_claude_messages_request(request).await;
        }
        let (request, body, full_replay_body) =
            match hyper_request_to_streaming_proxy_request(request).await {
                Ok(request) => request,
                Err(_error) => return empty_response(StatusCode::BAD_REQUEST),
            };
        let replayable_request = full_replay_body
            .filter(|body| request_metadata_prefix_is_complete_json(body))
            .map(|body| request.clone().with_body(body));

        let max_account_attempts = if replayable_request.is_some() {
            match self.enabled_account_attempt_limit().await {
                Ok(limit) => limit,
                Err(_error) => return quota_state_unavailable_response(),
            }
        } else {
            1
        };
        let mut first_attempt_body = Some(body);
        for attempt_index in 0..max_account_attempts {
            let (attempt_request, attempt_body) = if attempt_index == 0 {
                let Some(first_attempt_body) = first_attempt_body.take() else {
                    return empty_response(StatusCode::SERVICE_UNAVAILABLE);
                };
                (request.clone(), first_attempt_body)
            } else if let Some(replayable_request) = replayable_request.clone() {
                let retry_body = box_body_from_bytes(replayable_request.body().to_vec());
                (replayable_request, retry_body)
            } else {
                return empty_response(StatusCode::SERVICE_UNAVAILABLE);
            };

            let prepared = match self
                .prepare_async_streaming_http_request_async(attempt_request, attempt_body)
                .await
            {
                Ok(prepared) => prepared,
                Err(error) => return http_error_response(error),
            };
            let (upstream_request, completion) = prepared.into_parts();
            let response = match self.upstream.send_streaming(upstream_request).await {
                Ok(response) => response,
                Err(error) => return http_error_response(error),
            };
            match self
                .observe_precommit_http_quota_response(response, completion)
                .await
            {
                Ok(prepared_response) => {
                    return self.async_streaming_http_response_to_hyper(
                        prepared_response.completion,
                        prepared_response.response,
                    );
                }
                Err(PrecommitHttpQuotaResponse::AccountQuotaExhausted) => {
                    if replayable_request.is_none() {
                        return quota_state_unavailable_response();
                    }
                    tracing::info!(
                        attempt = attempt_index + 1,
                        max_attempts = max_account_attempts,
                        "codex_router.http_precommit_account_exhausted_retry"
                    );
                }
                Err(PrecommitHttpQuotaResponse::ProbeFailed(error)) => {
                    return http_error_response(error);
                }
                Err(PrecommitHttpQuotaResponse::ObservationFailed) => {
                    return empty_response(StatusCode::SERVICE_UNAVAILABLE);
                }
            }
        }

        all_accounts_exhausted_response()
    }

    async fn enabled_account_attempt_limit(&self) -> Result<usize, StateStoreError> {
        enabled_account_attempt_limit_from_accounts(
            self.selection_state_store.list_accounts().await,
        )
    }

    async fn handle_claude_messages_request(
        &self,
        request: HttpRequest<Incoming>,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        let Some(claude_edge_config) = &self.claude_edge_runtime_config else {
            tracing::error!("codex_router.claude_edge_configuration_missing_after_auth_preflight");
            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        };
        ClaudeServerRuntime {
            auth_gate: self.claude_edge_auth_gate.clone(),
            upstream: self.upstream.clone(),
            selection_state_store: self.selection_state_store.clone(),
            provider_error_state_store: self.provider_error_state_store.clone(),
            credential_resolver: self
                .credential_factory
                .resolver_for_state(self.credential_state_store.clone()),
            credential_store_availability: self.credential_store_availability,
            selector_runtime_state:
                AsyncAccountSelectorRuntimeState::new_with_selection_lock_and_affinity_cache(
                    Arc::clone(&self.weighted_selectors),
                    Arc::clone(&self.account_holds),
                    Arc::clone(&self.active_reservations),
                    Arc::clone(&self.runtime_exhaustions),
                    Arc::clone(&self.route_band_queue_health),
                    Arc::clone(&self.selection_reservation_lock),
                    Arc::clone(&self.session_affinity_cache),
                ),
            session_affinity_cache: Arc::clone(&self.session_affinity_cache),
            claude_five_hour_reserve_percent: self.claude_five_hour_reserve_percent,
            quota_refresh_interval: claude_edge_config.quota_refresh_interval,
            db_write_actor: self.db_write_actor.clone(),
            clock: self.runtime_clock(),
            affinity_record_tasks: self.affinity_record_tasks.clone(),
            audit_sink: self.audit_sink.clone(),
        }
        .handle_request(request)
        .await
    }
}

fn router_compatibility_response(
    method: &HttpMethod,
    uri: &Uri,
    local_model_authentication_required: bool,
) -> Option<HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>>> {
    if method != HttpMethod::GET || uri.path() != "/healthz" {
        return None;
    }

    let body = match serde_json::to_vec(&RouterCompatibility::current(
        local_model_authentication_required,
    )) {
        Ok(body) => body,
        Err(_error) => {
            return Some(empty_response(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };
    let response = HttpResponse::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(box_body_from_bytes(body))
        .unwrap_or_else(|_error| HttpResponse::new(empty_body()));
    Some(response)
}

fn enabled_account_attempt_limit_from_accounts(
    accounts: Result<Vec<AccountRecord>, StateStoreError>,
) -> Result<usize, StateStoreError> {
    Ok(accounts?
        .iter()
        .filter(|account| account.status() == AccountStatus::Enabled)
        .count()
        .max(1))
}

impl LoopbackProtocolConnectionHandler {
    async fn prepare_async_streaming_http_request_async(
        &self,
        request: HttpProxyRequest,
        body: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> Result<PreparedAsyncStreamingHttpProxyRequest, HttpProxyError> {
        let credential_resolver = self
            .credential_factory
            .resolver_for_state(self.credential_state_store.clone());
        let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
            &self.selection_state_store,
            AsyncAccountSelectorRuntimeState::new_with_selection_lock_and_affinity_cache(
                Arc::clone(&self.weighted_selectors),
                Arc::clone(&self.account_holds),
                Arc::clone(&self.active_reservations),
                Arc::clone(&self.runtime_exhaustions),
                Arc::clone(&self.route_band_queue_health),
                Arc::clone(&self.selection_reservation_lock),
                Arc::clone(&self.session_affinity_cache),
            ),
            DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            self.runtime_clock(),
        )
        .with_claude_five_hour_reserve_percent(self.claude_five_hour_reserve_percent)
        .with_active_client_lease_reporter(Arc::new(SqliteActiveClientLeaseReporter::new(
            self.db_write_actor.clone(),
            self.runtime_clock(),
        )))
        .with_session_affinity_writer(self.db_write_actor.clone());
        let service = AuthenticatedHttpProxyService::new(
            &self.auth_gate,
            &selector,
            &credential_resolver,
            &self.upstream,
        )
        .with_affinity_secret_provider(&self.affinity_secret_provider)
        .with_provider_error_observer(Arc::new(AsyncSqliteProviderErrorObserver::new(
            self.provider_error_state_store.clone(),
            self.selection_state_store.clone(),
            Arc::clone(&self.active_reservations),
            Arc::clone(&self.runtime_exhaustions),
            Arc::clone(&self.route_band_queue_health),
            self.db_write_actor.clone(),
        )));
        let service = if let Some(audit_sink) = &self.audit_sink {
            service.with_audit_sink(audit_sink)
        } else {
            service
        };
        service
            .prepare_async_streaming_request_async(request, body)
            .await
    }

    fn async_streaming_http_response_to_hyper(
        &self,
        completion: crate::http_sse::StreamingHttpProxyCompletion,
        response: AsyncStreamingHttpProxyResponse,
    ) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
        if let Some(audit_sink) = &self.audit_sink {
            append_audit_event_with_reporter(
                audit_sink,
                completion.allowed_audit_event(),
                &StderrAuditFailureReporter,
            );
        }
        let (status, headers, body) = response.into_parts();
        async_streaming_http_response_to_hyper(
            status,
            headers,
            body,
            completion,
            Arc::clone(&self.affinity_owner_recorder),
            self.affinity_record_tasks.clone(),
        )
    }

    async fn observe_precommit_http_quota_response(
        &self,
        response: AsyncStreamingHttpProxyResponse,
        completion: StreamingHttpProxyCompletion,
    ) -> Result<PreparedHttpResponseForCommit, PrecommitHttpQuotaResponse> {
        let provider_error_observer = completion.provider_error_observer().cloned();
        let account_id = completion.account_id().clone();
        let route_band = completion.route_band();
        match split_precommit_http_quota_response(response).await {
            Ok(PrecommitHttpResponseProbe::Forward(response)) => {
                Ok(PreparedHttpResponseForCommit {
                    response,
                    completion,
                })
            }
            Ok(PrecommitHttpResponseProbe::AccountQuotaExhausted { body }) => {
                drop(body);
                observe_precommit_http_quota_exhaustion_for_retry(
                    provider_error_observer,
                    account_id,
                    route_band,
                    current_unix_seconds().unwrap_or(0),
                )?;
                Err(PrecommitHttpQuotaResponse::AccountQuotaExhausted)
            }
            Err(error) => Err(PrecommitHttpQuotaResponse::ProbeFailed(error)),
        }
    }

    fn preflight_hyper_websocket_request(
        &self,
        request: &HttpRequest<Incoming>,
        path: &str,
    ) -> Option<HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>>> {
        let subprotocol = header_value(request.headers(), "sec-websocket-protocol");
        let router_token = header_value(request.headers(), "x-codex-router-token");
        let authorization = header_value(request.headers(), "authorization");
        let cookie = header_value(request.headers(), "cookie");
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
                path,
                &[],
                false,
            )
        };
        let presented_token = match presented_token {
            Ok(presented_token) => presented_token,
            Err(reason) => {
                self.emit_websocket_local_auth_rejection(reason);
                return Some(empty_response(StatusCode::UNAUTHORIZED));
            }
        };
        if let Err(reason) = self.auth_gate.authorize(presented_token) {
            self.emit_websocket_local_auth_rejection(reason);
            return Some(empty_response(StatusCode::UNAUTHORIZED));
        }
        match classify_route(Method::Post, path_without_query(path), true) {
            RouteClass::Supported(_) => None,
            RouteClass::Rejected { .. } => Some(empty_response(StatusCode::NOT_FOUND)),
        }
    }

    fn emit_websocket_local_auth_rejection(&self, reason: LocalAuthError) {
        if let Some(audit_sink) = &self.audit_sink {
            let event = local_auth_rejection_audit_event(
                TransportKind::WebSocket,
                AuditRouteKind::ResponsesWebSocket,
                reason,
            );
            append_audit_event_with_reporter(audit_sink, &event, &StderrAuditFailureReporter);
        }
    }

    fn runtime_clock(&self) -> Arc<dyn Fn() -> u64 + Send + Sync> {
        let fixed_now_unix_seconds = self.fixed_now_unix_seconds;
        Arc::new(move || {
            fixed_now_unix_seconds.unwrap_or_else(|| match current_unix_seconds() {
                Ok(now_unix_seconds) => now_unix_seconds,
                Err(error) => {
                    tracing::error!(
                        error.class = "system_clock_before_unix_epoch",
                        error.message = %error,
                        "codex_router.runtime_clock_failed"
                    );
                    0
                }
            })
        })
    }
}

fn current_unix_seconds() -> Result<u64, std::time::SystemTimeError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
}
