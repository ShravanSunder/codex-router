use std::collections::VecDeque;
use std::sync::Mutex;

use bytes::Bytes;
use codex_router_core::attempt_outcome::AttemptOutcome;
use codex_router_core::attempt_outcome::PassThroughReason;
use codex_router_core::attempt_outcome::TransportFailure;
use codex_router_core::route_profile::WindowKind;
use futures_util::future::BoxFuture;
use http_body_util::BodyExt;
use http_body_util::Full;

use super::super::forward::BufferedClaudeRequestBody;
use super::super::forward::ErrorBodyEvidence;
use super::ClaudeAttemptPipeline;
use super::ClaudeAttemptResult;
use super::run_at_most_two as run_at_most_two_with_task_tracker;
use crate::headers::HeaderCollection;
use crate::http_sse::AsyncHttpBodyError;
use crate::http_sse::AsyncStreamingHttpProxyResponse;
use crate::http_sse::HttpProxyError;
use tokio_util::task::TaskTracker;

async fn run_at_most_two<TPipeline, TClassifier>(
    pipeline: &TPipeline,
    first_attempt: TPipeline::Attempt,
    buffered_body: BufferedClaudeRequestBody,
    classifier: &TClassifier,
) -> Result<ClaudeAttemptResult<TPipeline::Attempt>, HttpProxyError>
where
    TPipeline: ClaudeAttemptPipeline,
    TClassifier: Fn(u16, &HeaderCollection, ErrorBodyEvidence<'_>) -> AttemptOutcome,
{
    run_at_most_two_with_task_tracker(
        pipeline,
        first_attempt,
        buffered_body,
        classifier,
        TaskTracker::new(),
    )
    .await
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AttemptAuthority {
    account: &'static str,
    generation: u64,
    pin_version: u64,
}

struct ScriptedPipeline {
    responses: Mutex<VecDeque<Result<AsyncStreamingHttpProxyResponse, TransportFailure>>>,
    sends: Mutex<Vec<(AttemptAuthority, Bytes)>>,
    recovered: Mutex<Vec<AttemptOutcome>>,
    finalized: Mutex<Vec<AttemptOutcome>>,
    second_authority: AttemptAuthority,
}

fn first_authority() -> AttemptAuthority {
    AttemptAuthority {
        account: "account-a",
        generation: 3,
        pin_version: 7,
    }
}

fn buffered(bytes: Bytes) -> BufferedClaudeRequestBody {
    BufferedClaudeRequestBody::new(bytes)
        .unwrap_or_else(|error| panic!("bounded test body: {error}"))
}

fn second_authority() -> AttemptAuthority {
    AttemptAuthority {
        account: "account-b",
        generation: 1,
        pin_version: 8,
    }
}

fn response(
    status: u16,
    bytes: &'static [u8],
) -> Result<AsyncStreamingHttpProxyResponse, TransportFailure> {
    Ok(AsyncStreamingHttpProxyResponse::new(
        status,
        HeaderCollection::default(),
        Full::new(Bytes::from_static(bytes))
            .map_err(|never| -> AsyncHttpBodyError { match never {} })
            .boxed(),
    ))
}

fn shared_rejection() -> AttemptOutcome {
    AttemptOutcome::SharedWindowExhausted {
        windows: vec![WindowKind::Weekly],
        resets: vec![None],
    }
}

fn classify(
    status: u16,
    _headers: &HeaderCollection,
    _evidence: super::ErrorBodyEvidence<'_>,
) -> AttemptOutcome {
    match status {
        429 => shared_rejection(),
        401 => AttemptOutcome::CredentialRejected,
        _ => AttemptOutcome::PassThrough(PassThroughReason::ServerError),
    }
}

impl ScriptedPipeline {
    fn new(responses: Vec<Result<AsyncStreamingHttpProxyResponse, TransportFailure>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            sends: Mutex::new(Vec::new()),
            recovered: Mutex::new(Vec::new()),
            finalized: Mutex::new(Vec::new()),
            second_authority: second_authority(),
        }
    }
}

impl ClaudeAttemptPipeline for ScriptedPipeline {
    type Attempt = AttemptAuthority;

    fn send<'a>(
        &'a self,
        attempt: &'a Self::Attempt,
        body: Bytes,
    ) -> BoxFuture<'a, Result<AsyncStreamingHttpProxyResponse, TransportFailure>> {
        Box::pin(async move {
            self.sends
                .lock()
                .unwrap_or_else(|error| panic!("send log: {error}"))
                .push((attempt.clone(), body));
            self.responses
                .lock()
                .unwrap_or_else(|error| panic!("response script: {error}"))
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected extra upstream attempt"))
        })
    }

    fn recover_first<'a>(
        &'a self,
        _attempt: &'a Self::Attempt,
        outcome: &'a AttemptOutcome,
    ) -> BoxFuture<'a, Result<Self::Attempt, HttpProxyError>> {
        Box::pin(async move {
            self.recovered
                .lock()
                .unwrap_or_else(|error| panic!("recovery log: {error}"))
                .push(outcome.clone());
            Ok(self.second_authority.clone())
        })
    }

    fn record_final<'a>(
        &'a self,
        _attempt: &'a Self::Attempt,
        outcome: &'a AttemptOutcome,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.finalized
                .lock()
                .unwrap_or_else(|error| panic!("final outcome log: {error}"))
                .push(outcome.clone());
        })
    }
}

#[tokio::test]
async fn claude_loop_replays_identical_bytes_once_and_uses_recovery_pin_authority() {
    let pipeline =
        ScriptedPipeline::new(vec![response(429, b"rejected"), response(200, b"success")]);
    let body = Bytes::from_static(b"client-carried conversation");
    let result = run_at_most_two(
        &pipeline,
        first_authority(),
        buffered(body.clone()),
        &classify,
    )
    .await
    .unwrap_or_else(|error| panic!("loop: {error}"));
    assert_eq!(result.attempts, 2);
    assert_eq!(result.attempt, second_authority());
    assert_eq!(result.outcome, AttemptOutcome::Success);
    let sends = pipeline
        .sends
        .lock()
        .unwrap_or_else(|error| panic!("sends: {error}"));
    assert_eq!(
        sends.as_slice(),
        &[
            (first_authority(), body.clone()),
            (second_authority(), body)
        ]
    );
    assert_eq!(
        pipeline
            .recovered
            .lock()
            .unwrap_or_else(|error| panic!("recovered: {error}"))
            .as_slice(),
        &[shared_rejection()]
    );
}

#[tokio::test]
async fn claude_loop_second_rejection_is_final_unchanged_and_records_failure() {
    let pipeline = ScriptedPipeline::new(vec![
        response(429, b"first"),
        response(401, b"second provider error"),
        response(200, b"must not send"),
    ]);
    let result = run_at_most_two(
        &pipeline,
        first_authority(),
        buffered(Bytes::new()),
        &classify,
    )
    .await
    .unwrap_or_else(|error| panic!("loop: {error}"));
    assert_eq!(result.attempts, 2);
    assert_eq!(result.outcome, AttemptOutcome::CredentialRejected);
    assert_eq!(
        pipeline
            .recovered
            .lock()
            .unwrap_or_else(|error| panic!("recovered: {error}"))
            .len(),
        1
    );
    assert_eq!(
        pipeline
            .finalized
            .lock()
            .unwrap_or_else(|error| panic!("finalized: {error}"))
            .as_slice(),
        &[AttemptOutcome::CredentialRejected]
    );
    assert_eq!(
        pipeline
            .responses
            .lock()
            .unwrap_or_else(|error| panic!("responses: {error}"))
            .len(),
        1
    );
    let response = result
        .response
        .unwrap_or_else(|| panic!("provider response must exist"))
        .into_response();
    let (status, _, body) = response.into_parts();
    assert_eq!(status, 401);
    assert_eq!(
        body.collect()
            .await
            .unwrap_or_else(|error| panic!("body: {error}"))
            .to_bytes(),
        Bytes::from_static(b"second provider error")
    );
}

#[tokio::test]
async fn claude_loop_transport_failure_has_no_recovery() {
    for reason in [TransportFailure::Connection, TransportFailure::Timeout] {
        let pipeline = ScriptedPipeline::new(vec![Err(reason)]);
        let result = run_at_most_two(
            &pipeline,
            first_authority(),
            buffered(Bytes::new()),
            &classify,
        )
        .await
        .unwrap_or_else(|error| panic!("loop: {error}"));
        assert_eq!(result.attempts, 1);
        assert_eq!(result.outcome, AttemptOutcome::NoResponse(reason));
        assert!(result.response.is_none());
        assert!(
            pipeline
                .recovered
                .lock()
                .unwrap_or_else(|error| panic!("recovered: {error}"))
                .is_empty()
        );
    }
}

#[tokio::test]
async fn claude_loop_second_transport_failure_never_sends_third_attempt() {
    let pipeline = ScriptedPipeline::new(vec![
        response(401, b"credential"),
        Err(TransportFailure::Timeout),
        response(200, b"must not send"),
    ]);
    let result = run_at_most_two(
        &pipeline,
        first_authority(),
        buffered(Bytes::new()),
        &classify,
    )
    .await
    .unwrap_or_else(|error| panic!("loop: {error}"));
    assert_eq!(result.attempts, 2);
    assert_eq!(
        result.outcome,
        AttemptOutcome::NoResponse(TransportFailure::Timeout)
    );
    assert_eq!(
        pipeline
            .sends
            .lock()
            .unwrap_or_else(|error| panic!("sends: {error}"))
            .len(),
        2
    );
}

#[tokio::test]
async fn claude_loop_passthrough_never_recovers() {
    let pipeline = ScriptedPipeline::new(vec![response(500, b"upstream failure")]);
    let result = run_at_most_two(
        &pipeline,
        first_authority(),
        buffered(Bytes::new()),
        &classify,
    )
    .await
    .unwrap_or_else(|error| panic!("loop: {error}"));
    assert_eq!(result.attempts, 1);
    assert_eq!(
        result.outcome,
        AttemptOutcome::PassThrough(PassThroughReason::ServerError)
    );
    assert!(
        pipeline
            .recovered
            .lock()
            .unwrap_or_else(|error| panic!("recovered: {error}"))
            .is_empty()
    );
}

#[tokio::test]
async fn claude_loop_success_keeps_original_authority_without_recovery() {
    let pipeline = ScriptedPipeline::new(vec![response(200, b"success")]);
    let result = run_at_most_two(
        &pipeline,
        first_authority(),
        buffered(Bytes::new()),
        &classify,
    )
    .await
    .unwrap_or_else(|error| panic!("loop: {error}"));
    assert_eq!(result.attempts, 1);
    assert_eq!(result.attempt, first_authority());
    assert!(
        pipeline
            .recovered
            .lock()
            .unwrap_or_else(|error| panic!("recovered: {error}"))
            .is_empty()
    );
}

#[tokio::test]
async fn claude_loop_refresh_success_keeps_same_account_with_new_generation() {
    let mut pipeline = ScriptedPipeline::new(vec![
        response(401, b"credential"),
        response(200, b"success"),
    ]);
    pipeline.second_authority = AttemptAuthority {
        generation: 4,
        ..first_authority()
    };
    let result = run_at_most_two(
        &pipeline,
        first_authority(),
        buffered(Bytes::new()),
        &classify,
    )
    .await
    .unwrap_or_else(|error| panic!("loop: {error}"));
    assert_eq!(result.attempts, 2);
    assert_eq!(result.attempt.account, "account-a");
    assert_eq!(result.attempt.generation, 4);
    assert_eq!(result.attempt.pin_version, 7);
}

#[tokio::test]
async fn claude_loop_body_error_after_2xx_commit_cannot_recover() {
    let frames = vec![
        Ok(hyper::body::Frame::data(Bytes::from_static(b"partial"))),
        Err(Box::new(std::io::Error::other("upstream cutoff")) as AsyncHttpBodyError),
    ];
    let body = http_body_util::StreamBody::new(futures_util::stream::iter(frames)).boxed();
    let pipeline = ScriptedPipeline::new(vec![
        Ok(AsyncStreamingHttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            body,
        )),
        response(200, b"must not send"),
    ]);
    let result = run_at_most_two(
        &pipeline,
        first_authority(),
        buffered(Bytes::new()),
        &classify,
    )
    .await
    .unwrap_or_else(|error| panic!("loop: {error}"));
    assert_eq!(result.attempts, 1);
    let prepared = result
        .response
        .unwrap_or_else(|| panic!("committed response must exist"));
    let (response, _outcome, completion) = prepared.into_parts();
    let (_, _, body) = response.into_parts();
    assert!(body.collect().await.is_err());
    assert_eq!(
        completion
            .unwrap_or_else(|| panic!("completion signal must exist"))
            .await
            .unwrap_or_else(|error| panic!("completion: {error}")),
        super::super::response_completion::ClaudeResponseCompletion::Incomplete
    );
    assert_eq!(
        pipeline
            .sends
            .lock()
            .unwrap_or_else(|error| panic!("sends: {error}"))
            .len(),
        1
    );
    assert!(
        pipeline
            .recovered
            .lock()
            .unwrap_or_else(|error| panic!("recovery: {error}"))
            .is_empty()
    );
}
