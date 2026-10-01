//! Claude request replay buffering and the status-based response commit boundary.

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use bytes::Bytes;
use bytes::BytesMut;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_core::attempt_outcome::AttemptOutcome;
use codex_router_core::attempt_outcome::PassThroughReason;
use codex_router_core::audit::ResponseCommitState;
use codex_router_core::redaction::SecretString;
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::combinators::BoxBody;
use hyper::body::Body;
use hyper::body::Frame;
use thiserror::Error;
use tokio::sync::oneshot;

use super::response_completion::ClaudeResponseCompletion;
use super::response_completion::observe_completion;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::headers::sanitize_headers_for_upstream;
use crate::http_sse::AsyncHttpBodyError;
use crate::http_sse::AsyncStreamingHttpProxyResponse;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
use crate::http_sse::StreamingUpstreamHttpRequest;
use crate::routes::Method;
use crate::routes::RouteKind;

/// Anthropic Messages request limit, applied before any upstream attempt.
pub const CLAUDE_REQUEST_BODY_LIMIT: usize = 32 * 1024 * 1024;
/// Maximum body evidence supplied to the error classifier.
pub const CLAUDE_ERROR_EVIDENCE_LIMIT: usize = 64 * 1024;

/// Request bytes validated against the Messages replay limit before any attempt.
#[derive(Clone)]
pub(crate) struct BufferedClaudeRequestBody(Bytes);

impl BufferedClaudeRequestBody {
    /// Validates already-buffered bytes at the edge boundary.
    pub(crate) fn new(bytes: Bytes) -> Result<Self, ClaudeRequestBodyError> {
        if bytes.len() > CLAUDE_REQUEST_BODY_LIMIT {
            return Err(ClaudeRequestBodyError::TooLarge);
        }
        Ok(Self(bytes))
    }

    /// Borrows the validated bytes without copying their payload.
    pub(crate) const fn as_bytes(&self) -> &Bytes {
        &self.0
    }

    /// Moves the validated buffer into the bounded attempt loop.
    pub(crate) fn into_bytes(self) -> Bytes {
        self.0
    }
}

/// Claude protocol edge; account selection and recovery remain in the pipeline.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClaudeEdge;

impl ClaudeEdge {
    /// Prepares an admitted Messages request using its validated replay buffer.
    pub(crate) fn prepare_upstream(
        request: &HttpProxyRequest,
        body: &BufferedClaudeRequestBody,
        credential: &ResolvedProviderCredential,
    ) -> Result<StreamingUpstreamHttpRequest, HttpProxyError> {
        let path_component = request
            .path()
            .split_once('?')
            .map_or(request.path(), |(path, _query)| path);
        if request.method() != Method::Post
            || request.websocket_upgrade()
            || path_component != "/anthropic/v1/messages"
        {
            return Err(HttpProxyError::Rejected {
                reason: "unsupported_claude_route",
            });
        }
        let upstream_path =
            request
                .path()
                .strip_prefix("/anthropic")
                .ok_or(HttpProxyError::Rejected {
                    reason: "unsupported_claude_route",
                })?;
        Ok(StreamingUpstreamHttpRequest::new(
            request.method(),
            upstream_path.to_owned(),
            RouteKind::ClaudeMessages,
            Self::prepare_headers(request, credential.access_token()),
            Full::new(body.as_bytes().clone())
                .map_err(|never| -> AsyncHttpBodyError { match never {} })
                .boxed(),
        ))
    }

    /// Preserves client capabilities while replacing all client credential carriers.
    pub(crate) fn prepare_headers(
        request: &HttpProxyRequest,
        access_token: &SecretString,
    ) -> HeaderCollection {
        let beta_values: Vec<&str> = request
            .headers()
            .iter()
            .filter(|header| header.name() == "anthropic-beta")
            .map(Header::value)
            .collect();
        let oauth_present = beta_values.iter().any(|value| {
            value
                .split(',')
                .any(|token| token.trim() == "oauth-2025-04-20")
        });
        let mut beta = beta_values.join(",");
        if !oauth_present {
            if !beta.is_empty() {
                beta.push(',');
            }
            beta.push_str("oauth-2025-04-20");
        }
        let headers = request
            .headers()
            .iter()
            .filter(|header| !matches!(header.name(), "x-api-key" | "anthropic-beta"))
            .cloned()
            .collect();
        let sanitized = sanitize_headers_for_upstream(headers, access_token.clone(), None);
        let mut headers = sanitized.as_slice().to_vec();
        headers.push(Header::new("anthropic-beta", beta));
        HeaderCollection::new(headers)
    }
}

/// Request buffering failed before provider egress.
#[derive(Debug, Error)]
pub enum ClaudeRequestBodyError {
    /// The Messages request exceeds the provider's documented limit.
    #[error("Claude Messages request body exceeds 32 MiB")]
    TooLarge,
    /// The local request body did not complete.
    #[error("Claude Messages request body could not be read")]
    Unreadable,
}

impl ClaudeRequestBodyError {
    /// Returns the local HTTP rejection status.
    #[must_use]
    pub const fn status_code(&self) -> u16 {
        match self {
            Self::TooLarge => 413,
            Self::Unreadable => 400,
        }
    }
}

/// Bounded evidence whose completeness must govern credential attribution.
#[derive(Clone, Copy, Debug)]
pub struct ErrorBodyEvidence<'a> {
    /// At most 64 KiB of response body bytes.
    pub prefix: &'a [u8],
    /// Whether this prefix contains the entire provider error body.
    pub complete: bool,
}

/// A status-known response held until the attempt policy chooses its disposition.
pub struct PreparedClaudeResponse {
    response: AsyncStreamingHttpProxyResponse,
    outcome: AttemptOutcome,
    commit_state: ResponseCommitState,
    completion: Option<oneshot::Receiver<ClaudeResponseCompletion>>,
}

impl PreparedClaudeResponse {
    /// Returns the edge's classified outcome.
    #[must_use]
    pub const fn outcome(&self) -> &AttemptOutcome {
        &self.outcome
    }

    /// Returns whether the 2xx commit point has already prohibited recovery.
    #[must_use]
    pub const fn committed(&self) -> bool {
        matches!(self.commit_state, ResponseCommitState::Committed)
    }

    /// Returns the response with its body bytes and frames preserved.
    #[must_use]
    #[cfg(test)]
    pub fn into_response(self) -> AsyncStreamingHttpProxyResponse {
        self.response
    }

    /// Consumes the result and its typed terminal signal for OnSuccess publication.
    pub(crate) fn into_parts(
        self,
    ) -> (
        AsyncStreamingHttpProxyResponse,
        AttemptOutcome,
        Option<oneshot::Receiver<ClaudeResponseCompletion>>,
    ) {
        (self.response, self.outcome, self.completion)
    }
}

/// Buffers a request before upstream egress.
pub(crate) async fn buffer_request_body(
    mut body: BoxBody<Bytes, AsyncHttpBodyError>,
) -> Result<BufferedClaudeRequestBody, ClaudeRequestBodyError> {
    let mut buffered = BytesMut::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_error| ClaudeRequestBodyError::Unreadable)?;
        let Ok(bytes) = frame.into_data() else {
            continue;
        };
        if bytes.len() > CLAUDE_REQUEST_BODY_LIMIT.saturating_sub(buffered.len()) {
            return Err(ClaudeRequestBodyError::TooLarge);
        }
        buffered.extend_from_slice(&bytes);
    }
    BufferedClaudeRequestBody::new(buffered.freeze())
}

/// Holds a status-known response at the edge's commit boundary.
pub async fn inspect_response<TClassifier>(
    response: AsyncStreamingHttpProxyResponse,
    classifier: &TClassifier,
) -> PreparedClaudeResponse
where
    TClassifier: Fn(u16, &HeaderCollection, ErrorBodyEvidence<'_>) -> AttemptOutcome,
{
    if (200..300).contains(&response.status()) {
        let (response, completion) = observe_completion(response);
        return PreparedClaudeResponse {
            response,
            outcome: AttemptOutcome::Success,
            commit_state: ResponseCommitState::Committed,
            completion: Some(completion),
        };
    }
    let (status, headers, mut body) = response.into_parts();
    let mut prefix = Vec::new();
    let mut retained_frames = VecDeque::new();
    let complete = loop {
        match body.frame().await {
            None => break true,
            Some(Err(error)) => {
                retained_frames.push_back(Err(error));
                break false;
            }
            Some(Ok(frame)) => {
                let Some(bytes) = frame.data_ref() else {
                    retained_frames.push_back(Ok(frame));
                    continue;
                };
                let remaining = CLAUDE_ERROR_EVIDENCE_LIMIT.saturating_sub(prefix.len());
                let prefix_length = remaining.min(bytes.len());
                if let Some(bytes_prefix) = bytes.get(..prefix_length) {
                    prefix.extend_from_slice(bytes_prefix);
                }
                let overflow = bytes.len() > remaining;
                retained_frames.push_back(Ok(frame));
                if overflow {
                    break false;
                }
            }
        }
    };
    let classified = classifier(
        status,
        &headers,
        ErrorBodyEvidence {
            prefix: &prefix,
            complete,
        },
    );
    let outcome = if (!complete
        && !matches!(classified, AttemptOutcome::SharedWindowExhausted { .. }))
        || matches!(
            classified,
            AttemptOutcome::Success | AttemptOutcome::NoResponse(_)
        ) {
        AttemptOutcome::PassThrough(PassThroughReason::MalformedEvidence)
    } else {
        classified
    };
    PreparedClaudeResponse {
        response: AsyncStreamingHttpProxyResponse::new(
            status,
            headers,
            RetainedResponseBody {
                retained_frames,
                body,
            }
            .boxed(),
        ),
        outcome,
        commit_state: ResponseCommitState::NotCommitted,
        completion: None,
    }
}

struct RetainedResponseBody {
    retained_frames: VecDeque<Result<Frame<Bytes>, AsyncHttpBodyError>>,
    body: BoxBody<Bytes, AsyncHttpBodyError>,
}

impl Body for RetainedResponseBody {
    type Data = Bytes;
    type Error = AsyncHttpBodyError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, AsyncHttpBodyError>>> {
        if let Some(frame) = self.retained_frames.pop_front() {
            return Poll::Ready(Some(frame));
        }
        Pin::new(&mut self.body).poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.retained_frames.is_empty() && self.body.is_end_stream()
    }
}

#[cfg(test)]
mod tests;
