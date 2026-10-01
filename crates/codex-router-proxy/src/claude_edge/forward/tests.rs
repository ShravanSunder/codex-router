use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use bytes::Bytes;
use codex_router_core::attempt_outcome::AttemptOutcome;
use codex_router_core::attempt_outcome::PassThroughReason;
use codex_router_core::route_profile::WindowKind;
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::StreamBody;
use http_body_util::combinators::BoxBody;
use hyper::body::Frame;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

use super::BufferedClaudeRequestBody;
use super::CLAUDE_ERROR_EVIDENCE_LIMIT;
use super::CLAUDE_REQUEST_BODY_LIMIT;
use super::ClaudeEdge;
use super::ClaudeRequestBodyError;
use super::ErrorBodyEvidence;
use super::buffer_request_body;
use super::inspect_response as inspect_response_with_task_tracker;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::http_sse::AsyncHttpBodyError;
use crate::http_sse::AsyncStreamingHttpProxyResponse;
use crate::http_sse::HttpProxyRequest;
use crate::routes::Method;
use crate::routes::RouteKind;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use tokio_util::task::TaskTracker;

async fn inspect_response<TClassifier>(
    response: AsyncStreamingHttpProxyResponse,
    classifier: &TClassifier,
) -> super::PreparedClaudeResponse
where
    TClassifier: Fn(u16, &HeaderCollection, ErrorBodyEvidence<'_>) -> AttemptOutcome,
{
    inspect_response_with_task_tracker(response, classifier, TaskTracker::new()).await
}

fn credential() -> ResolvedProviderCredential {
    ResolvedProviderCredential::new(
        AccountId::new("account-a").unwrap_or_else(|error| panic!("account: {error}")),
        SecretString::new("pooled-access"),
        3,
    )
}

fn raw_http_header_value<'a>(headers: &'a [u8], header_name: &[u8]) -> Option<&'a [u8]> {
    headers
        .split(|byte| *byte == b'\n')
        .skip(1)
        .find_map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let colon_index = line.iter().position(|byte| *byte == b':')?;
            if !line[..colon_index].eq_ignore_ascii_case(header_name) {
                return None;
            }
            let mut value = &line[colon_index + 1..];
            if value.first() == Some(&b' ') {
                value = &value[1..];
            }
            Some(value)
        })
}

#[tokio::test]
async fn claude_upstream_preparation_preserves_body_headers_and_query_and_removes_local_prefix() {
    let client = HttpProxyRequest::new(Method::Post, "/anthropic/v1/messages?beta=true")
        .with_header(Header::new("authorization", "Bearer local-token"))
        .with_header(Header::new("anthropic-beta", "client-capability"))
        .with_header(Header::new("anthropic-version", "2023-06-01"))
        .with_header(Header::new("x-custom-client-header", "preserved"));
    let original =
        Bytes::from_static(br#"{"messages":[{"role":"user","content":"hello"}],"future_field":7}"#);
    let body = BufferedClaudeRequestBody::new(original.clone())
        .unwrap_or_else(|error| panic!("body: {error}"));
    let prepared = ClaudeEdge::prepare_upstream(&client, &body, &credential())
        .unwrap_or_else(|error| panic!("prepare: {error}"));
    assert_eq!(prepared.method(), Method::Post);
    assert_eq!(prepared.path(), "/v1/messages?beta=true");
    assert_eq!(prepared.route_kind(), RouteKind::ClaudeMessages);
    assert_eq!(
        prepared.headers().value("authorization"),
        Some("Bearer pooled-access")
    );
    assert_eq!(
        prepared.headers().value("anthropic-beta"),
        Some("client-capability,oauth-2025-04-20")
    );
    assert_eq!(
        prepared.headers().value("x-custom-client-header"),
        Some("preserved")
    );
    assert_eq!(
        prepared.headers().value("anthropic-version"),
        Some("2023-06-01")
    );
    assert_eq!(
        prepared
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("request body: {error}"))
            .to_bytes(),
        original
    );
}

#[test]
fn claude_upstream_preparation_rejects_non_messages_routes() {
    let body = BufferedClaudeRequestBody::new(Bytes::new())
        .unwrap_or_else(|error| panic!("body: {error}"));
    for request in [
        HttpProxyRequest::new(Method::Post, "/anthropic/v1/messages/count_tokens"),
        HttpProxyRequest::new(Method::Get, "/anthropic/v1/messages"),
        HttpProxyRequest::new(Method::Post, "/v1/responses"),
        HttpProxyRequest::new(Method::Post, "/anthropic/v1/messages").with_websocket_upgrade(true),
    ] {
        assert!(ClaudeEdge::prepare_upstream(&request, &body, &credential()).is_err());
    }
}

#[tokio::test]
#[cfg(debug_assertions)]
async fn claude_upstream_preparation_forwards_real_headers_and_body_to_mock_anthropic() {
    use crate::http_sse::AsyncStreamingUpstreamHttpTransport;
    use crate::upstream::ClaudeUpstreamEndpoint;
    use crate::upstream::HyperHttpUpstreamTransport;
    use crate::upstream::UpstreamEndpoint;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("mock Anthropic should bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("address: {error}"));
    let server = tokio::spawn(async move {
        let (mut stream, _peer) = listener
            .accept()
            .await
            .unwrap_or_else(|error| panic!("accept: {error}"));
        let mut received = Vec::new();
        let mut read_buffer = [0_u8; 1024];
        loop {
            let read = stream
                .read(&mut read_buffer)
                .await
                .unwrap_or_else(|error| panic!("request read: {error}"));
            assert!(read > 0, "request must finish before connection EOF");
            received.extend_from_slice(
                read_buffer
                    .get(..read)
                    .unwrap_or_else(|| panic!("read length must fit buffer")),
            );
            let Some(header_end) = received.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let header_bytes = received
                .get(..header_end)
                .unwrap_or_else(|| panic!("headers must fit buffer"));
            let content_length = raw_http_header_value(header_bytes, b"content-length")
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_else(|| panic!("bounded Full body must carry Content-Length"));
            if received.len() >= header_end + 4 + content_length {
                break;
            }
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap_or_else(|error| panic!("response: {error}"));
        received
    });
    let opaque_beta_value = b"client-capability\x80";
    let beta_header = http::HeaderValue::from_bytes(opaque_beta_value)
        .unwrap_or_else(|error| panic!("opaque HTTP beta header should be valid: {error}"));
    let client = HttpProxyRequest::new(Method::Post, "/anthropic/v1/messages?beta=true")
        .with_header(Header::new("authorization", "Bearer local-token"))
        .with_header(Header::from_http("anthropic-beta", &beta_header))
        .with_header(Header::new("x-custom-client-header", "preserved"));
    let mut expected_beta_value = opaque_beta_value.to_vec();
    expected_beta_value.extend_from_slice(b",oauth-2025-04-20");
    let expected_body = Bytes::from_static(br#"{"messages":[{"role":"user","content":"hello"}]}"#);
    let body = BufferedClaudeRequestBody::new(expected_body.clone())
        .unwrap_or_else(|error| panic!("body: {error}"));
    let prepared = ClaudeEdge::prepare_upstream(&client, &body, &credential())
        .unwrap_or_else(|error| panic!("prepare: {error}"));
    let endpoint = UpstreamEndpoint::new("http://127.0.0.1:1/unused-codex")
        .unwrap_or_else(|error| panic!("endpoint: {error}"));
    let claude_endpoint =
        ClaudeUpstreamEndpoint::isolated_debug_override(format!("http://{address}"), true)
            .unwrap_or_else(|error| panic!("isolated Claude endpoint: {error}"));
    let upstream = HyperHttpUpstreamTransport::new(endpoint)
        .with_debug_claude_upstream_endpoint(claude_endpoint);
    assert_eq!(
        upstream.upstream_url(prepared.route_kind(), prepared.path()),
        format!("http://{address}/v1/messages?beta=true"),
        "the transport must target the isolated mock before any request is sent",
    );
    let reply = upstream
        .send_streaming(prepared)
        .await
        .unwrap_or_else(|error| panic!("upstream: {error}"));
    assert_eq!(reply.status(), 200);
    let (_, _, reply_body) = reply.into_parts();
    assert_eq!(
        reply_body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("reply body: {error}"))
            .to_bytes(),
        Bytes::from_static(b"ok")
    );
    let received = server
        .await
        .unwrap_or_else(|error| panic!("mock Anthropic: {error}"));
    let header_end = received
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("request must contain headers"));
    let header_bytes = received
        .get(..header_end)
        .unwrap_or_else(|| panic!("headers must fit"));
    assert!(header_bytes.starts_with(b"POST /v1/messages?beta=true HTTP/1.1\r\n"));
    assert!(
        raw_http_header_value(header_bytes, b"authorization")
            .is_some_and(|value| value.eq_ignore_ascii_case(b"Bearer pooled-access"))
    );
    assert!(
        raw_http_header_value(header_bytes, b"anthropic-beta")
            .is_some_and(|value| value == expected_beta_value.as_slice())
    );
    assert!(
        raw_http_header_value(header_bytes, b"x-custom-client-header")
            .is_some_and(|value| value == &b"preserved"[..])
    );
    assert!(
        !header_bytes
            .windows(b"local-token".len())
            .any(|window| window == b"local-token")
    );
    assert_eq!(received.get(header_end + 4..), Some(expected_body.as_ref()));
}

#[test]
fn claude_headers_replace_local_auth_and_preserve_capabilities_with_oauth_beta() {
    let request = HttpProxyRequest::new(Method::Post, "/anthropic/v1/messages")
        .with_header(Header::new("authorization", "Bearer local-token"))
        .with_header(Header::new("x-codex-router-token", "local-token"))
        .with_header(Header::new("x-api-key", "client-key"))
        .with_header(Header::new("cookie", "session=local"))
        .with_header(Header::new("host", "localhost"))
        .with_header(Header::new("content-length", "55"))
        .with_header(Header::new("anthropic-version", "2023-06-01"))
        .with_header(Header::new("anthropic-beta", "client-beta-a"))
        .with_header(Header::new("anthropic-beta", "client-beta-b"))
        .with_header(Header::new("x-claude-code-session-id", "session-a"));
    let headers = ClaudeEdge::prepare_headers(&request, &SecretString::new("pooled-access"));
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer pooled-access"]
    );
    assert_eq!(
        headers.value("anthropic-beta"),
        Some("client-beta-a,client-beta-b,oauth-2025-04-20")
    );
    assert_eq!(headers.values("anthropic-beta").len(), 1);
    assert_eq!(headers.value("anthropic-version"), Some("2023-06-01"));
    assert_eq!(headers.value("x-claude-code-session-id"), Some("session-a"));
    for stripped in [
        "x-codex-router-token",
        "x-api-key",
        "cookie",
        "host",
        "content-length",
    ] {
        assert_eq!(
            headers.value(stripped),
            None,
            "{stripped} must not reach upstream"
        );
    }
}

#[test]
fn claude_headers_supply_oauth_beta_without_a_client_beta_header() {
    let request = HttpProxyRequest::new(Method::Post, "/anthropic/v1/messages");
    assert_eq!(
        ClaudeEdge::prepare_headers(&request, &SecretString::new("pooled-access"))
            .value("anthropic-beta"),
        Some("oauth-2025-04-20")
    );
}

#[test]
fn claude_headers_keep_existing_oauth_beta_without_duplication() {
    let request = HttpProxyRequest::new(Method::Post, "/anthropic/v1/messages").with_header(
        Header::new("anthropic-beta", "client-beta,oauth-2025-04-20"),
    );
    assert_eq!(
        ClaudeEdge::prepare_headers(&request, &SecretString::new("pooled-access"))
            .value("anthropic-beta"),
        Some("client-beta,oauth-2025-04-20")
    );
}

fn full_body(bytes: Bytes) -> BoxBody<Bytes, AsyncHttpBodyError> {
    Full::new(bytes)
        .map_err(|never| -> AsyncHttpBodyError { match never {} })
        .boxed()
}

fn response(
    status: u16,
    body: BoxBody<Bytes, AsyncHttpBodyError>,
) -> AsyncStreamingHttpProxyResponse {
    AsyncStreamingHttpProxyResponse::new(
        status,
        HeaderCollection::new(vec![Header::new("content-type", "application/json")]),
        body,
    )
}

fn shared_rejection() -> AttemptOutcome {
    AttemptOutcome::SharedWindowExhausted {
        windows: vec![WindowKind::FiveHour],
        resets: vec![Some(123)],
    }
}

#[tokio::test]
async fn claude_request_body_accepts_exactly_32_mib_and_replays_unchanged() {
    let expected = Bytes::from(vec![b'x'; CLAUDE_REQUEST_BODY_LIMIT]);
    let buffered = buffer_request_body(full_body(expected.clone()))
        .await
        .unwrap_or_else(|error| panic!("bounded request should buffer: {error}"));
    assert_eq!(buffered.as_bytes(), &expected);
}

#[test]
fn claude_buffered_body_constructor_rejects_unbounded_attempt_input() {
    assert!(matches!(
        BufferedClaudeRequestBody::new(Bytes::from(vec![b'x'; CLAUDE_REQUEST_BODY_LIMIT + 1])),
        Err(ClaudeRequestBodyError::TooLarge)
    ));
}

#[tokio::test]
async fn claude_request_body_over_32_mib_returns_413() {
    let result = buffer_request_body(full_body(Bytes::from(vec![
        b'x';
        CLAUDE_REQUEST_BODY_LIMIT + 1
    ])))
    .await;
    assert!(matches!(result, Err(ClaudeRequestBodyError::TooLarge)));
    let error = ClaudeRequestBodyError::TooLarge;
    assert_eq!(error.status_code(), 413);
}

#[tokio::test]
async fn claude_2xx_commits_without_reading_or_classifying_body() {
    let body_polls = Arc::new(AtomicUsize::new(0));
    let poll_counter = Arc::clone(&body_polls);
    let stream = futures_util::stream::poll_fn(move |_context| {
        poll_counter.fetch_add(1, Ordering::SeqCst);
        std::task::Poll::Ready(None::<Result<Frame<Bytes>, AsyncHttpBodyError>>)
    });
    let prepared = inspect_response(
        response(200, StreamBody::new(stream).boxed()),
        &|_, _, _| panic!("2xx must never classify an error body"),
    )
    .await;
    assert!(prepared.committed());
    assert_eq!(prepared.outcome(), &AttemptOutcome::Success);
    assert_eq!(body_polls.load(Ordering::SeqCst), 0);
    let (_response, _outcome, completion) = prepared.into_parts();
    assert!(completion.is_some());
}

#[tokio::test]
async fn claude_complete_error_body_is_classified_and_preserved() {
    let body = Bytes::from_static(br#"{"error":{"type":"authentication_error"}}"#);
    let prepared = inspect_response(
        response(401, full_body(body.clone())),
        &|status, headers, evidence| {
            assert_eq!(status, 401);
            assert_eq!(headers.value("content-type"), Some("application/json"));
            assert!(evidence.complete);
            assert_eq!(evidence.prefix, body.as_ref());
            AttemptOutcome::CredentialRejected
        },
    )
    .await;
    assert!(!prepared.committed());
    assert_eq!(prepared.outcome(), &AttemptOutcome::CredentialRejected);
    let (_, _, preserved) = prepared.into_response().into_parts();
    let preserved = preserved
        .collect()
        .await
        .unwrap_or_else(|error| panic!("body: {error}"));
    assert_eq!(preserved.to_bytes(), body);
}

#[tokio::test]
async fn claude_error_exactly_64_kib_is_complete() {
    let body = Bytes::from(vec![b'e'; CLAUDE_ERROR_EVIDENCE_LIMIT]);
    let prepared = inspect_response(response(429, full_body(body)), &|_, _, evidence| {
        assert!(evidence.complete);
        assert_eq!(evidence.prefix.len(), CLAUDE_ERROR_EVIDENCE_LIMIT);
        shared_rejection()
    })
    .await;
    assert_eq!(prepared.outcome(), &shared_rejection());
}

#[tokio::test]
async fn claude_incomplete_error_cannot_reject_credentials_and_preserves_entire_body() {
    let body = Bytes::from(vec![b'e'; CLAUDE_ERROR_EVIDENCE_LIMIT + 37]);
    let prepared = inspect_response(response(401, full_body(body.clone())), &|_, _, evidence| {
        assert!(!evidence.complete);
        assert_eq!(evidence.prefix.len(), CLAUDE_ERROR_EVIDENCE_LIMIT);
        AttemptOutcome::CredentialRejected
    })
    .await;
    assert_eq!(
        prepared.outcome(),
        &AttemptOutcome::PassThrough(PassThroughReason::MalformedEvidence)
    );
    let (status, _, preserved) = prepared.into_response().into_parts();
    assert_eq!(status, 401);
    let preserved = preserved
        .collect()
        .await
        .unwrap_or_else(|error| panic!("body: {error}"));
    assert_eq!(preserved.to_bytes(), body);
}

#[tokio::test]
async fn claude_incomplete_error_can_use_shared_window_header_evidence() {
    let body = Bytes::from(vec![b'e'; CLAUDE_ERROR_EVIDENCE_LIMIT + 1]);
    let prepared = inspect_response(response(429, full_body(body)), &|_, _, evidence| {
        assert!(!evidence.complete);
        shared_rejection()
    })
    .await;
    assert_eq!(prepared.outcome(), &shared_rejection());
}

#[tokio::test]
async fn claude_error_passthrough_streams_unread_frames_and_trailers_unchanged() {
    let first = Bytes::from(vec![b'a'; CLAUDE_ERROR_EVIDENCE_LIMIT]);
    let tail = Bytes::from_static(b"unread-tail");
    let mut trailers = http::HeaderMap::new();
    trailers.insert("x-upstream-trailer", http::HeaderValue::from_static("kept"));
    let frames: Vec<Result<Frame<Bytes>, AsyncHttpBodyError>> = vec![
        Ok(Frame::data(first.clone())),
        Ok(Frame::data(tail.clone())),
        Ok(Frame::trailers(trailers)),
    ];
    let body = StreamBody::new(futures_util::stream::iter(frames)).boxed();
    let prepared = inspect_response(response(500, body), &|_, _, _| {
        AttemptOutcome::PassThrough(PassThroughReason::ServerError)
    })
    .await;
    let (_, _, body) = prepared.into_response().into_parts();
    let preserved = body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("body: {error}"));
    assert_eq!(
        preserved
            .trailers()
            .and_then(|headers| headers.get("x-upstream-trailer")),
        Some(&http::HeaderValue::from_static("kept"))
    );
    let mut expected = first.to_vec();
    expected.extend_from_slice(&tail);
    assert_eq!(preserved.to_bytes(), Bytes::from(expected));
}

#[tokio::test]
async fn claude_request_body_read_error_rejects_before_upstream() {
    let frames = vec![
        Ok(Frame::data(Bytes::from_static(b"partial"))),
        Err(Box::new(std::io::Error::other("client body failed")) as AsyncHttpBodyError),
    ];
    let body = StreamBody::new(futures_util::stream::iter(frames)).boxed();
    assert!(matches!(
        buffer_request_body(body).await,
        Err(ClaudeRequestBodyError::Unreadable)
    ));
}

#[tokio::test]
async fn claude_error_body_read_failure_is_incomplete_and_stays_in_passthrough_body() {
    let frames = vec![
        Ok(Frame::data(Bytes::from_static(b"partial"))),
        Err(Box::new(std::io::Error::other("upstream body failed")) as AsyncHttpBodyError),
    ];
    let body = StreamBody::new(futures_util::stream::iter(frames)).boxed();
    let prepared = inspect_response(response(401, body), &|_, _, evidence| {
        assert!(!evidence.complete);
        assert_eq!(evidence.prefix, b"partial");
        AttemptOutcome::CredentialRejected
    })
    .await;
    assert_eq!(
        prepared.outcome(),
        &AttemptOutcome::PassThrough(PassThroughReason::MalformedEvidence)
    );
    let (_, _, mut body) = prepared.into_response().into_parts();
    let frame = body
        .frame()
        .await
        .unwrap_or_else(|| panic!("prefix must exist"))
        .unwrap_or_else(|error| panic!("prefix should read: {error}"));
    assert_eq!(
        frame
            .into_data()
            .unwrap_or_else(|_| panic!("data frame expected")),
        Bytes::from_static(b"partial")
    );
    assert!(matches!(body.frame().await, Some(Err(_error))));
}

#[tokio::test]
async fn claude_non_2xx_error_probe_leaves_unread_remainder_unpolled() {
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&polls);
    let first = Bytes::from(vec![b'e'; CLAUDE_ERROR_EVIDENCE_LIMIT + 1]);
    let mut pending_first = Some(first);
    let stream = futures_util::stream::poll_fn(move |_context| {
        counter.fetch_add(1, Ordering::SeqCst);
        std::task::Poll::Ready(
            pending_first
                .take()
                .map(|bytes| Ok::<_, AsyncHttpBodyError>(Frame::data(bytes))),
        )
    });
    let prepared = inspect_response(
        response(429, StreamBody::new(stream).boxed()),
        &|_, _, _| shared_rejection(),
    )
    .await;
    assert_eq!(prepared.outcome(), &shared_rejection());
    assert_eq!(polls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn claude_error_classifier_receives_gzip_evidence_and_preserves_wire_response() {
    let plaintext = Bytes::from_static(
        br#"{"error":{"type":"authentication_error","message":"credential rejected"}}"#,
    );
    let compressed = compress_gzip(plaintext.clone()).await;
    let response = AsyncStreamingHttpProxyResponse::new(
        401,
        HeaderCollection::new(vec![
            Header::new("content-type", "application/json"),
            Header::new("content-encoding", "gzip"),
            Header::new("content-length", compressed.len().to_string()),
        ]),
        Full::new(compressed.clone())
            .map_err(|never| -> AsyncHttpBodyError { match never {} })
            .boxed(),
    );
    let classifier = |status: u16, headers: &HeaderCollection, evidence: ErrorBodyEvidence<'_>| {
        assert_eq!(status, 401);
        assert_eq!(headers.value("content-encoding"), Some("gzip"));
        assert_eq!(evidence.prefix, plaintext.as_ref());
        assert!(evidence.complete);
        AttemptOutcome::CredentialRejected
    };

    let prepared = inspect_response(response, &classifier).await;
    assert_eq!(prepared.outcome(), &AttemptOutcome::CredentialRejected);
    let response = prepared.into_response();
    let (status, headers, body) = response.into_parts();
    assert_eq!(status, 401);
    assert_eq!(headers.value("content-encoding"), Some("gzip"));
    assert_eq!(
        body.collect()
            .await
            .unwrap_or_else(|error| panic!("body: {error}"))
            .to_bytes(),
        compressed
    );
}

#[tokio::test]
async fn claude_compressed_error_evidence_keeps_a_bounded_plaintext_prefix() {
    let mut plaintext = br#"{"error":{"type":"rate_limit_error","message":""#.to_vec();
    plaintext.extend(std::iter::repeat_n(b'x', CLAUDE_ERROR_EVIDENCE_LIMIT + 64));
    plaintext.extend_from_slice(b"\"}}");
    let compressed = compress_gzip(Bytes::from(plaintext)).await;
    assert!(compressed.len() < CLAUDE_ERROR_EVIDENCE_LIMIT);
    let response = AsyncStreamingHttpProxyResponse::new(
        429,
        HeaderCollection::new(vec![
            Header::new("content-type", "application/json"),
            Header::new("content-encoding", "gzip"),
        ]),
        Full::new(compressed)
            .map_err(|never| -> AsyncHttpBodyError { match never {} })
            .boxed(),
    );

    let prepared = inspect_response(response, &|_, _, evidence| {
        assert_eq!(evidence.prefix.len(), CLAUDE_ERROR_EVIDENCE_LIMIT);
        assert!(
            evidence
                .prefix
                .starts_with(b"{\"error\":{\"type\":\"rate_limit_error\"")
        );
        assert!(!evidence.complete);
        shared_rejection()
    })
    .await;

    assert_eq!(prepared.outcome(), &shared_rejection());
}

async fn compress_gzip(plaintext: Bytes) -> Bytes {
    let (writer, mut reader) = tokio::io::duplex(64);
    let encoder_task = tokio::spawn(async move {
        let mut encoder = async_compression::tokio::write::GzipEncoder::new(writer);
        encoder.write_all(&plaintext).await?;
        encoder.shutdown().await
    });
    let mut compressed = Vec::new();
    reader
        .read_to_end(&mut compressed)
        .await
        .unwrap_or_else(|error| panic!("compressed error body: {error}"));
    encoder_task
        .await
        .unwrap_or_else(|error| panic!("gzip encoder task: {error}"))
        .unwrap_or_else(|error| panic!("gzip error body: {error}"));
    Bytes::from(compressed)
}
