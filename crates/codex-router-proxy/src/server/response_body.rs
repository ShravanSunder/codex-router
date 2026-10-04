pub(crate) fn method_from_hyper(method: &HttpMethod) -> Method {
    match *method {
        HttpMethod::GET => Method::Get,
        HttpMethod::POST => Method::Post,
        _ => Method::Other,
    }
}

fn async_streaming_http_response_to_hyper(
    status: u16,
    headers: HeaderCollection,
    body: BoxBody<Bytes, AsyncHttpBodyError>,
    completion: StreamingHttpProxyCompletion,
    affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    affinity_record_tasks: TaskTracker,
) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    let body = record_affinity_owner_from_async_body(
        body,
        completion,
        affinity_owner_recorder,
        None,
        affinity_record_tasks,
    );
    let mut builder = HttpResponse::builder().status(status);
    for header in headers.as_slice() {
        builder = builder.header(header.name(), header.value());
    }
    builder
        .body(body)
        .unwrap_or_else(|_error| empty_response(StatusCode::BAD_GATEWAY))
}

fn record_affinity_owner_from_async_body(
    body: BoxBody<Bytes, AsyncHttpBodyError>,
    completion: StreamingHttpProxyCompletion,
    affinity_owner_recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
    affinity_record_tasks: TaskTracker,
) -> BoxBody<Bytes, AsyncHttpBodyError> {
    let active_reservation_guard = completion.active_reservation_guard().cloned();
    let affinity_secret = completion.affinity_secret().cloned();
    let provider_error_observer =
        provider_error_observer.or_else(|| completion.provider_error_observer().cloned());
    if affinity_secret.is_none() && provider_error_observer.is_none() {
        return hold_active_reservation_until_body_drop(body, active_reservation_guard);
    }
    let account_id = completion.account_id().clone();
    let route_band = completion.route_band();
    let credential_generation = completion.credential_generation();
    let mut buffered = Vec::new();
    let mut affinity_recorded = false;
    let mut provider_error_recorded = false;
    let mut scanned_bytes = 0_usize;
    let mut scanned_events = 0_usize;

    body.map_frame(move |frame| {
        let _active_reservation_guard = &active_reservation_guard;
        let should_scan_affinity = affinity_secret.is_some() && !affinity_recorded;
        let should_scan_provider_error =
            provider_error_observer.is_some() && !provider_error_recorded;
        if (should_scan_affinity || should_scan_provider_error)
            && scanned_bytes < HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES
            && scanned_events < HTTP_RESPONSE_AFFINITY_SCAN_MAX_EVENTS
            && let Some(data) = frame.data_ref()
        {
            scanned_events += 1;
            let remaining_bytes = HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES - scanned_bytes;
            let bytes_to_scan = data.len().min(remaining_bytes);
            if let Some(scanned_data) = data.get(..bytes_to_scan) {
                buffered.extend_from_slice(scanned_data);
            }
            scanned_bytes += bytes_to_scan;
            if should_scan_affinity
                && let Some(secret) = affinity_secret.as_ref()
                && let Ok(Some(response_id)) = extract_response_id_from_body(&buffered)
            {
                affinity_recorded = true;
                spawn_async_affinity_owner_record(
                    Arc::clone(&affinity_owner_recorder),
                    secret.clone(),
                    account_id.clone(),
                    credential_generation,
                    response_id,
                    affinity_record_tasks.clone(),
                );
            }
            if should_scan_provider_error
                && let Some(provider_error_body) = provider_error_body_from_http_buffer(&buffered)
            {
                provider_error_recorded = true;
                let provider_error_classification =
                    classify_provider_error_envelope(&provider_error_body);
                if let Some(observer) = provider_error_observer.as_ref() {
                    spawn_async_provider_error_observation(
                        Arc::clone(observer),
                        account_id.clone(),
                        route_band,
                        provider_error_classification,
                        affinity_record_tasks.clone(),
                    );
                }
            }
        }

        frame
    })
    .boxed()
}

fn provider_error_body_from_http_buffer(buffered: &[u8]) -> Option<Vec<u8>> {
    if classify_provider_error_envelope(buffered) != ProviderErrorClassification::Unknown {
        return Some(buffered.to_vec());
    }

    for line in buffered.split(|byte| *byte == b'\n') {
        let line = trim_ascii_bytes(line);
        let Some(data) = line.strip_prefix(b"data:") else {
            continue;
        };
        let data = trim_ascii_bytes(data);
        if data == b"[DONE]" || data.is_empty() {
            continue;
        }
        if classify_provider_error_envelope(data) != ProviderErrorClassification::Unknown {
            return Some(data.to_vec());
        }
    }

    None
}

fn trim_ascii_bytes(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    match bytes.get(start..end) {
        Some(trimmed) => trimmed,
        None => &[],
    }
}

pub(crate) fn hold_active_reservation_until_body_drop(
    body: BoxBody<Bytes, AsyncHttpBodyError>,
    active_reservation_guard: Option<crate::account_selection::ActiveReservationGuard>,
) -> BoxBody<Bytes, AsyncHttpBodyError> {
    if active_reservation_guard.is_none() {
        return body;
    }

    body.map_frame(move |frame| {
        let _active_reservation_guard = &active_reservation_guard;
        frame
    })
    .boxed()
}

fn spawn_async_affinity_owner_record(
    recorder: Arc<dyn AsyncHttpAffinityOwnerRecorder>,
    affinity_secret: codex_router_core::affinity::RouterAffinityHashSecret,
    account_id: codex_router_core::ids::AccountId,
    credential_generation: u64,
    response_id: codex_router_core::affinity::PreviousResponseId,
    affinity_record_tasks: TaskTracker,
) {
    affinity_record_tasks.spawn(async move {
        let Ok(affinity_key_hash) = hash_previous_response_id(&affinity_secret, &response_id)
        else {
            return;
        };
        let owner = PreviousResponseAffinityOwnerRecord::new(
            affinity_key_hash,
            account_id,
            credential_generation,
            RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            current_unix_seconds().unwrap_or(0),
        );
        let _record_result = recorder.record_affinity_owner(owner).await;
    });
}

fn spawn_async_provider_error_observation(
    observer: Arc<dyn AsyncProviderErrorObserver>,
    account_id: codex_router_core::ids::AccountId,
    route_band: RouteBand,
    classification: ProviderErrorClassification,
    affinity_record_tasks: TaskTracker,
) {
    let observed_unix_seconds = current_unix_seconds().unwrap_or(0);
    if classification == ProviderErrorClassification::AccountQuotaExhausted {
        let _runtime_mark_result = observer.mark_runtime_account_quota_exhausted(
            account_id.clone(),
            route_band,
            observed_unix_seconds,
        );
        let _enqueue_result = observer.enqueue_provider_quota_exhaustion(
            account_id,
            route_band,
            classification,
            observed_unix_seconds,
        );
        return;
    }

    affinity_record_tasks.spawn(async move {
        let _observation_result = observer
            .observe_provider_error(
                account_id,
                route_band,
                classification,
                observed_unix_seconds,
            )
            .await;
    });
}

pub(crate) fn incoming_body_error(error: hyper::Error) -> AsyncHttpBodyError {
    Box::new(error)
}

fn sanitize_route_path_for_log(path: &str) -> &'static str {
    if path.ends_with("/responses") {
        "/v1/responses"
    } else if path.ends_with("/models") {
        "/v1/models"
    } else {
        "other"
    }
}

fn websocket_runtime_error_kind(error: &LoopbackRouterRuntimeError) -> &'static str {
    match error {
        LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::Transport(_),
        ) => "websocket_transport",
        LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::Handshake,
        ) => "websocket_handshake",
        LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::CloseReason(_),
        ) => "websocket_close_before_upstream",
        LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::ConnectionTracking(_),
        ) => "websocket_connection_tracking",
        LoopbackRouterRuntimeError::WebSocket(_) => "websocket_other",
        LoopbackRouterRuntimeError::HyperConnection(_) => "hyper_connection",
        LoopbackRouterRuntimeError::HyperBody(_) => "hyper_body",
        _ => "router_runtime",
    }
}

fn sanitize_error_for_log(error: &LoopbackRouterRuntimeError) -> String {
    let rendered_error = error.to_string();
    if rendered_error.contains("BadRecordMac") {
        "websocket transport failed: BadRecordMac".to_owned()
    } else if rendered_error.contains("FirstFrameTimeout") {
        "websocket closed before upstream open: FirstFrameTimeout".to_owned()
    } else {
        websocket_runtime_error_kind(error).to_owned()
    }
}

pub(crate) fn http_error_response(
    error: HttpProxyError,
) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    match error {
        HttpProxyError::LocalAuth { .. } => empty_response(StatusCode::UNAUTHORIZED),
        HttpProxyError::Selection {
            reason: crate::account_selection::QuotaAwareAccountSelectorError::NoEligibleAccounts,
        } => all_accounts_exhausted_response(),
        HttpProxyError::Selection { .. } => quota_state_unavailable_response(),
        HttpProxyError::ProviderCredential { .. } | HttpProxyError::Upstream { .. } => {
            empty_response(StatusCode::BAD_GATEWAY)
        }
        HttpProxyError::Rejected { .. } => empty_response(StatusCode::NOT_FOUND),
    }
}

/// A complete, bounded provider rejection retained only when Claude cannot select attempt two.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CapturedClaudeProviderErrorResponse {
    pub(crate) status: u16,
    pub(crate) headers: HeaderCollection,
    pub(crate) body: Vec<Bytes>,
    pub(crate) evidence_prefix: Bytes,
    pub(crate) evidence_complete: bool,
}

/// Renders Claude's ordered no-account result without changing the shared Codex mapping.
pub(crate) fn claude_selection_unavailable_response(
    reason: UnavailableReason,
    account_labels: &[(AccountId, String)],
    now_unix_seconds: u64,
    provider_usage_limit_response: Option<CapturedClaudeProviderErrorResponse>,
) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    if let (UnavailableReason::AllExhausted { .. }, Some(response)) =
        (&reason, provider_usage_limit_response)
    {
        return captured_claude_provider_response(response);
    }

    match reason {
        UnavailableReason::NoneConfiguredOrEnabled => claude_api_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "No Claude account is configured or enabled in Router.".to_owned(),
            None,
            None,
        ),
        UnavailableReason::KeyUnreadable => claude_api_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "Router cannot unlock its Claude credentials because the Keychain key is unreadable."
                .to_owned(),
            None,
            None,
        ),
        UnavailableReason::MigrationIncomplete => claude_api_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "Router cannot unlock its Claude credentials because pooled-credential migration is incomplete."
                .to_owned(),
            None,
            None,
        ),
        UnavailableReason::AllNeedLogin { accounts } => {
            let account_names = accounts
                .iter()
                .map(|account_id| claude_account_name(account_id, account_labels))
                .collect::<Vec<_>>();
            claude_api_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                format!(
                    "These Claude accounts need account login before Router can use them: {}.",
                    account_names.join(", ")
                ),
                None,
                None,
            )
        }
        UnavailableReason::AllExhausted { earliest_headroom } => {
            let retry_after_seconds = earliest_headroom
                .map(HeadroomTimestamp::unix_seconds)
                .map(|headroom| headroom.saturating_sub(now_unix_seconds));
            let message = match retry_after_seconds {
                Some(seconds) => format!(
                    "Claude usage limit reached. Router expects quota headroom in {seconds} seconds."
                ),
                None => "Claude usage limit reached.".to_owned(),
            };
            claude_api_error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                message,
                Some("usage_limit_reached"),
                retry_after_seconds,
            )
        }
        UnavailableReason::HeldByFloors { accounts } => {
            let held_accounts = accounts
                .iter()
                .map(|account| {
                    let reason = match account.reason() {
                        SelectionHoldReason::HardFloor => "at the configured hard quota floor",
                        SelectionHoldReason::WaitingForFreshWeeklyObservation => {
                            "waiting for a fresh weekly quota observation under its configured floor"
                        }
                    };
                    format!(
                        "{} ({reason})",
                        claude_account_name(account.account_id(), account_labels)
                    )
                })
                .collect::<Vec<_>>();
            claude_api_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                format!(
                    "These Claude accounts are held by quota floors: {}.",
                    held_accounts.join(", ")
                ),
                None,
                None,
            )
        }
    }
}

fn claude_account_name(account_id: &AccountId, account_labels: &[(AccountId, String)]) -> String {
    account_labels
        .iter()
        .find(|(candidate_id, _label)| candidate_id == account_id)
        .map(|(_account_id, label)| safe_account_label(label, account_id).to_string())
        .unwrap_or_else(|| safe_account_label("account", account_id).to_string())
}

fn claude_api_error_response(
    status: StatusCode,
    error_type: &'static str,
    message: String,
    error_code: Option<&'static str>,
    retry_after_seconds: Option<u64>,
) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    let error = match error_code {
        Some(error_code) => serde_json::json!({
            "type": error_type,
            "message": message,
            "code": error_code,
        }),
        None => serde_json::json!({
            "type": error_type,
            "message": message,
        }),
    };
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "error",
        "error": error,
    }))
    .unwrap_or_else(|_error| {
        br#"{"type":"error","error":{"type":"api_error","message":"Claude request unavailable."}}"#
            .to_vec()
    });
    let mut builder = HttpResponse::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json");
    if let Some(retry_after_seconds) = retry_after_seconds {
        builder = builder.header(http::header::RETRY_AFTER, retry_after_seconds.to_string());
    }
    builder
        .body(box_body_from_bytes(body))
        .unwrap_or_else(|_error| empty_response(StatusCode::BAD_GATEWAY))
}

fn captured_claude_provider_response(
    response: CapturedClaudeProviderErrorResponse,
) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    let mut builder = HttpResponse::builder()
        .status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::BAD_GATEWAY));
    for header in response.headers.as_slice() {
        builder = builder.header(header.name(), header.value());
    }
    let body = StreamBody::new(stream::iter(
        response
            .body
            .into_iter()
            .map(|chunk| Ok::<_, Infallible>(Frame::data(chunk))),
    ))
    .map_err(|never: Infallible| -> AsyncHttpBodyError { match never {} })
    .boxed();
    builder
        .body(body)
        .unwrap_or_else(|_error| empty_response(StatusCode::BAD_GATEWAY))
}

pub(crate) fn empty_response(
    status: StatusCode,
) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    HttpResponse::builder()
        .status(status)
        .body(empty_body())
        .unwrap_or_else(|_error| HttpResponse::new(empty_body()))
}

fn unsupported_claude_path_response(
    path: &str,
) -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    let body = format!("Path {path} is unsupported by Router");
    HttpResponse::builder()
        .status(StatusCode::NOT_FOUND)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(box_body_from_bytes(body.into_bytes()))
        .unwrap_or_else(|_error| HttpResponse::new(empty_body()))
}

fn all_accounts_exhausted_response() -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    HttpResponse::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .body(box_body_from_bytes(
            crate::websocket::ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL
                .as_bytes()
                .to_vec(),
        ))
        .unwrap_or_else(|_error| HttpResponse::new(empty_body()))
}

fn quota_state_unavailable_response() -> HttpResponse<BoxBody<Bytes, AsyncHttpBodyError>> {
    HttpResponse::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .body(box_body_from_bytes(
            crate::websocket::ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL
                .as_bytes()
                .to_vec(),
        ))
        .unwrap_or_else(|_error| HttpResponse::new(empty_body()))
}

fn empty_body() -> BoxBody<Bytes, AsyncHttpBodyError> {
    Empty::<Bytes>::new()
        .map_err(|never: Infallible| -> AsyncHttpBodyError { match never {} })
        .boxed()
}
