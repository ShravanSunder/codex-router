use super::*;

pub(super) async fn hyper_request_to_streaming_proxy_request(
    request: HttpRequest<Incoming>,
) -> Result<
    (
        HttpProxyRequest,
        BoxBody<Bytes, AsyncHttpBodyError>,
        Option<Vec<u8>>,
    ),
    LoopbackRouterRuntimeError,
> {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str)
        .to_owned();
    let buffered_body = bounded_request_metadata_body(body)
        .await
        .map_err(LoopbackRouterRuntimeError::HyperBody)?;
    let mut proxy_request = HttpProxyRequest::new(method_from_hyper(&parts.method), path);
    for (name, value) in &parts.headers {
        if let Ok(value) = value.to_str() {
            proxy_request = proxy_request.with_header(Header::new(name.as_str(), value));
        }
    }
    let streaming_body = buffered_body
        .streaming_body
        .map_err(incoming_body_error)
        .boxed();

    Ok((
        proxy_request.with_body(buffered_body.routing_metadata_prefix),
        streaming_body,
        buffered_body.full_replay_body,
    ))
}

pub(super) struct BufferedRequestBody {
    pub(super) routing_metadata_prefix: Vec<u8>,
    pub(super) full_replay_body: Option<Vec<u8>>,
    pub(super) streaming_body: PrefixFramesThenIncomingBody,
}

pub(super) async fn bounded_request_metadata_body(
    mut body: Incoming,
) -> Result<BufferedRequestBody, hyper::Error> {
    let mut routing_metadata_prefix = Vec::new();
    let mut full_replay_body = Some(Vec::new());
    let mut replay_frames = VecDeque::new();
    loop {
        let Some(frame) = body.frame().await.transpose()? else {
            break;
        };
        if let Some(data) = frame.data_ref() {
            if routing_metadata_prefix.len() < HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES {
                let remaining_bytes =
                    HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES - routing_metadata_prefix.len();
                let bytes_to_scan = data.len().min(remaining_bytes);
                if let Some(scanned_data) = data.get(..bytes_to_scan) {
                    routing_metadata_prefix.extend_from_slice(scanned_data);
                }
            }
            if let Some(replay_body) = full_replay_body.as_mut() {
                if replay_body.len().saturating_add(data.len()) <= HTTP_REQUEST_REPLAY_MAX_BYTES {
                    replay_body.extend_from_slice(data);
                } else {
                    full_replay_body = None;
                }
            }
        } else {
            full_replay_body = None;
        }
        let replay_body_is_complete = full_replay_body
            .as_deref()
            .is_some_and(request_metadata_prefix_is_complete_json);
        replay_frames.push_back(frame);
        if replay_body_is_complete || full_replay_body.is_none() {
            break;
        }
    }

    Ok(BufferedRequestBody {
        routing_metadata_prefix,
        full_replay_body,
        streaming_body: PrefixFramesThenIncomingBody::new(replay_frames, body),
    })
}

pub(super) async fn finish_hyper_connection_after_serve_result(
    serve_result: Result<(), LoopbackRouterRuntimeError>,
    upgrade_tasks: SharedUpgradeTasks,
) -> Result<(), LoopbackRouterRuntimeError> {
    let mut first_connection_error = serve_result.err();
    let mut upgrade_task_guard = upgrade_tasks.lock().await;
    let drained_upgrade_tasks = std::mem::take(&mut *upgrade_task_guard);
    drop(upgrade_task_guard);

    for upgrade_task in drained_upgrade_tasks {
        let _stored_error =
            store_connection_join_error(&mut first_connection_error, upgrade_task.await);
    }

    match first_connection_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub(super) fn request_metadata_prefix_is_complete_json(metadata_prefix: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(metadata_prefix).is_ok()
}

pub(super) struct PreparedHttpResponseForCommit {
    pub(super) response: AsyncStreamingHttpProxyResponse,
    pub(super) completion: StreamingHttpProxyCompletion,
}

#[derive(Debug)]
pub(super) enum PrecommitHttpQuotaResponse {
    AccountQuotaExhausted,
    ObservationFailed,
    ProbeFailed(HttpProxyError),
}

pub(super) enum PrecommitHttpResponseProbe {
    Forward(AsyncStreamingHttpProxyResponse),
    AccountQuotaExhausted { body: Vec<u8> },
}

pub(super) fn observe_precommit_http_quota_exhaustion_for_retry(
    provider_error_observer: Option<Arc<dyn AsyncProviderErrorObserver>>,
    account_id: codex_router_core::ids::AccountId,
    route_band: RouteBand,
    observed_unix_seconds: u64,
) -> Result<(), PrecommitHttpQuotaResponse> {
    let Some(observer) = provider_error_observer else {
        return Ok(());
    };
    observer
        .mark_runtime_account_quota_exhausted(account_id.clone(), route_band, observed_unix_seconds)
        .map_err(|_error| PrecommitHttpQuotaResponse::ObservationFailed)?;
    match observer.enqueue_provider_quota_exhaustion(
        account_id,
        route_band,
        ProviderErrorClassification::AccountQuotaExhausted,
        observed_unix_seconds,
    ) {
        DbWriteEnqueueResult::Enqueued => Ok(()),
        DbWriteEnqueueResult::FullDegraded | DbWriteEnqueueResult::ClosedDegraded => {
            Err(PrecommitHttpQuotaResponse::ObservationFailed)
        }
    }
}

pub(super) async fn split_precommit_http_quota_response(
    response: AsyncStreamingHttpProxyResponse,
) -> Result<PrecommitHttpResponseProbe, HttpProxyError> {
    let (status, headers, mut body) = response.into_parts();
    let mut replay_frames = VecDeque::new();
    let mut buffered = Vec::new();
    let mut scanned_bytes = 0_usize;
    let mut scanned_events = 0_usize;

    while scanned_bytes < HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES
        && scanned_events < HTTP_RESPONSE_AFFINITY_SCAN_MAX_EVENTS
    {
        let Some(frame) =
            body.frame()
                .await
                .transpose()
                .map_err(|error| HttpProxyError::Upstream {
                    message: error.to_string(),
                })?
        else {
            break;
        };
        scanned_events += 1;
        if let Some(data) = frame.data_ref() {
            let remaining_bytes = HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES - scanned_bytes;
            let bytes_to_scan = data.len().min(remaining_bytes);
            if let Some(scanned_data) = data.get(..bytes_to_scan) {
                buffered.extend_from_slice(scanned_data);
            }
            scanned_bytes += bytes_to_scan;
            if let Some(provider_error_body) = provider_error_body_from_http_buffer(&buffered)
                && classify_provider_error_envelope(&provider_error_body)
                    == ProviderErrorClassification::AccountQuotaExhausted
            {
                return Ok(PrecommitHttpResponseProbe::AccountQuotaExhausted {
                    body: provider_error_body,
                });
            }
        }
        replay_frames.push_back(frame);
        if status < 400 && !precommit_probe_should_continue_for_success_status(&buffered) {
            break;
        }
    }

    let replay_body = PrefixFramesThenBoxBody::new(replay_frames, body).boxed();
    Ok(PrecommitHttpResponseProbe::Forward(
        AsyncStreamingHttpProxyResponse::new(status, headers, replay_body),
    ))
}

pub(super) fn precommit_probe_should_continue_for_success_status(buffered: &[u8]) -> bool {
    let trimmed = trim_ascii_bytes(buffered);
    trimmed.starts_with(b"event: error") && !trimmed.windows(2).any(|window| window == b"\n\n")
}

pub(crate) fn box_body_from_bytes(bytes: Vec<u8>) -> BoxBody<Bytes, AsyncHttpBodyError> {
    Full::new(Bytes::from(bytes))
        .map_err(|never: Infallible| -> AsyncHttpBodyError { match never {} })
        .boxed()
}

pub(super) struct PrefixFramesThenBoxBody {
    pub(super) prefix_frames: VecDeque<Frame<Bytes>>,
    pub(super) inner: BoxBody<Bytes, AsyncHttpBodyError>,
}

impl PrefixFramesThenBoxBody {
    pub(super) fn new(
        prefix_frames: VecDeque<Frame<Bytes>>,
        inner: BoxBody<Bytes, AsyncHttpBodyError>,
    ) -> Self {
        Self {
            prefix_frames,
            inner,
        }
    }
}

impl HyperBody for PrefixFramesThenBoxBody {
    type Data = Bytes;
    type Error = AsyncHttpBodyError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if let Some(frame) = self.prefix_frames.pop_front() {
            return Poll::Ready(Some(Ok(frame)));
        }

        Pin::new(&mut self.inner).poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.prefix_frames.is_empty() && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        prefix_frames_size_hint(&self.prefix_frames, self.inner.size_hint())
    }
}

pub(super) struct PrefixFramesThenIncomingBody {
    pub(super) prefix_frames: VecDeque<Frame<Bytes>>,
    pub(super) inner: Incoming,
}

impl PrefixFramesThenIncomingBody {
    pub(super) fn new(prefix_frames: VecDeque<Frame<Bytes>>, inner: Incoming) -> Self {
        Self {
            prefix_frames,
            inner,
        }
    }
}

impl HyperBody for PrefixFramesThenIncomingBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if let Some(frame) = self.prefix_frames.pop_front() {
            return Poll::Ready(Some(Ok(frame)));
        }

        Pin::new(&mut self.inner).poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.prefix_frames.is_empty() && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        prefix_frames_size_hint(&self.prefix_frames, self.inner.size_hint())
    }
}

pub(super) fn prefix_frames_size_hint(
    prefix_frames: &VecDeque<Frame<Bytes>>,
    inner_hint: SizeHint,
) -> SizeHint {
    let prefix_data_length = prefix_frames
        .iter()
        .filter_map(Frame::data_ref)
        .map(|data| u64::try_from(data.len()).unwrap_or(u64::MAX))
        .fold(0_u64, u64::saturating_add);
    let mut size_hint = SizeHint::new();
    size_hint.set_lower(prefix_data_length.saturating_add(inner_hint.lower()));
    if let Some(inner_upper) = inner_hint.upper() {
        size_hint.set_upper(prefix_data_length.saturating_add(inner_upper));
    }

    size_hint
}
