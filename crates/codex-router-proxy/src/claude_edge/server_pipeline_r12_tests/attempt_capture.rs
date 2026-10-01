use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::http_sse::AsyncHttpBodyError;
use crate::server::CapturedClaudeProviderErrorResponse;
use crate::server::claude_selection_unavailable_response;
use bytes::Bytes;
use codex_router_core::attempt_outcome::AttemptOutcome;
use codex_router_selection::selection_outcome::UnavailableReason;
use http::StatusCode;
use http_body_util::BodyExt;
use http_body_util::Full;

#[tokio::test]
async fn claude_exhaustion_preserves_attempt_one_provider_usage_limit_response() {
    let provider_body = Bytes::from_static(
        br#"{"type":"error","error":{"type":"rate_limit_error","message":"provider usage limit sentinel"}}"#,
    );
    let provider_headers = HeaderCollection::new(vec![
        Header::new("content-type", "application/json"),
        Header::new("anthropic-ratelimit-unified-status", "rejected"),
    ]);
    let response = claude_selection_unavailable_response(
        UnavailableReason::AllExhausted {
            earliest_headroom: None,
        },
        &[],
        1_000,
        Some(CapturedClaudeProviderErrorResponse {
            status: 429,
            headers: provider_headers,
            body: vec![provider_body.clone()],
            evidence_prefix: provider_body.clone(),
            evidence_complete: true,
        }),
    );
    let (parts, body) = response.into_parts();
    let actual_body = body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("provider response body should be readable: {error}"))
        .to_bytes();
    assert_eq!(parts.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        parts.headers.get("content-type").unwrap(),
        "application/json"
    );
    assert_eq!(
        parts
            .headers
            .get("anthropic-ratelimit-unified-status")
            .unwrap(),
        "rejected"
    );
    assert_eq!(actual_body, provider_body);
}

#[tokio::test]
async fn attempt_one_capture_retains_large_complete_body_with_bounded_classification_evidence() {
    let provider_body = Bytes::from(vec![b'x'; super::super::CLAUDE_ERROR_EVIDENCE_LIMIT + 19]);
    let headers = HeaderCollection::new(vec![
        Header::new("anthropic-ratelimit-unified-status", "rejected"),
        Header::new(
            "anthropic-ratelimit-unified-representative-claim",
            "five_hour",
        ),
    ]);
    let capture = super::super::FirstAttemptResponseCapture::default();
    let body = Full::new(provider_body.clone())
        .map_err(|never| -> AsyncHttpBodyError { match never {} })
        .boxed();
    let wrapped_body =
        super::super::CapturingProviderErrorBody::new(body, capture.clone(), 429, headers.clone())
            .boxed();
    let forwarded_body = wrapped_body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("wrapped provider body should be readable: {error}"))
        .to_bytes();
    let captured_response = capture
        .take()
        .unwrap_or_else(|| panic!("complete attempt-one response should be captured"));

    assert_eq!(forwarded_body, provider_body);
    assert_eq!(captured_response.status, 429);
    assert_eq!(captured_response.headers, headers);
    assert_eq!(
        captured_response.body.iter().map(Bytes::len).sum::<usize>(),
        provider_body.len()
    );
    assert_eq!(
        captured_response.evidence_prefix.len(),
        super::super::CLAUDE_ERROR_EVIDENCE_LIMIT
    );
    assert!(!captured_response.evidence_complete);
    assert!(matches!(
        super::super::classify_claude_outcome(
            captured_response.status,
            &captured_response.headers,
            super::super::ErrorBodyEvidence {
                prefix: &captured_response.evidence_prefix,
                complete: captured_response.evidence_complete,
            },
        ),
        AttemptOutcome::SharedWindowExhausted { .. }
    ));
    let preserved_response = claude_selection_unavailable_response(
        UnavailableReason::AllExhausted {
            earliest_headroom: None,
        },
        &[],
        1_000,
        Some(captured_response),
    );
    let preserved_body = preserved_response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("preserved provider body should be readable: {error}"))
        .to_bytes();
    assert_eq!(preserved_body, provider_body);
}
