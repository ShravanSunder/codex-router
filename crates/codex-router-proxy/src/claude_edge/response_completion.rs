//! Typed Claude response completion evidence consumed by OnSuccess pin publication.

use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper::body::Body;
use hyper::body::Frame;
use serde::Deserialize;
use tokio::sync::oneshot;

use super::forward::CLAUDE_ERROR_EVIDENCE_LIMIT;
use crate::http_sse::AsyncHttpBodyError;
use crate::http_sse::AsyncStreamingHttpProxyResponse;

/// A committed response's terminal result, distinct from its initial 2xx status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeResponseCompletion {
    /// Complete non-stream body, or SSE ending with `message_stop` and no error event.
    Success,
    /// The body was cut off, dropped, or SSE ended without `message_stop`.
    Incomplete,
    /// The committed SSE response contained a provider error event.
    ProviderError,
}

/// Observes completion without collecting the response body or changing its frames.
pub(crate) fn observe_completion(
    response: AsyncStreamingHttpProxyResponse,
) -> (
    AsyncStreamingHttpProxyResponse,
    oneshot::Receiver<ClaudeResponseCompletion>,
) {
    let (status, headers, body) = response.into_parts();
    let streaming = headers.value("content-type").is_some_and(|content_type| {
        content_type
            .split(';')
            .next()
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"))
    });
    let (sender, receiver) = oneshot::channel();
    let observed = CompletionObservingBody {
        body,
        sender: Some(sender),
        tracker: streaming.then(SseCompletionTracker::default),
    };
    (
        AsyncStreamingHttpProxyResponse::new(status, headers, observed.boxed()),
        receiver,
    )
}

struct CompletionObservingBody {
    body: BoxBody<Bytes, AsyncHttpBodyError>,
    sender: Option<oneshot::Sender<ClaudeResponseCompletion>>,
    tracker: Option<SseCompletionTracker>,
}

impl CompletionObservingBody {
    fn finish(&mut self, completion: ClaudeResponseCompletion) {
        if let Some(sender) = self.sender.take() {
            let _result = sender.send(completion);
        }
    }
}

impl Drop for CompletionObservingBody {
    fn drop(&mut self) {
        self.finish(ClaudeResponseCompletion::Incomplete);
    }
}

impl Body for CompletionObservingBody {
    type Data = Bytes;
    type Error = AsyncHttpBodyError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, AsyncHttpBodyError>>> {
        let result = Pin::new(&mut self.body).poll_frame(context);
        match &result {
            Poll::Ready(Some(Ok(frame))) => {
                if let (Some(tracker), Some(bytes)) = (&mut self.tracker, frame.data_ref()) {
                    tracker.observe(bytes);
                }
            }
            Poll::Ready(Some(Err(_error))) => self.finish(ClaudeResponseCompletion::Incomplete),
            Poll::Ready(None) => {
                let completion = self.tracker.as_ref().map_or(
                    ClaudeResponseCompletion::Success,
                    SseCompletionTracker::completion,
                );
                self.finish(completion);
            }
            Poll::Pending => {}
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        // Force the consumer to poll EOF so completion is observed even for empty bodies.
        self.sender.is_none() && self.body.is_end_stream()
    }
}

#[derive(Default)]
struct SseCompletionTracker {
    pending_line: Vec<u8>,
    oversized_line: bool,
    event_name: Option<String>,
    event_data: Vec<u8>,
    oversized_event: bool,
    message_stopped: bool,
    provider_error: bool,
}

#[derive(Deserialize)]
struct ClaudeEventType {
    #[serde(rename = "type")]
    event_type: String,
}

impl SseCompletionTracker {
    fn observe(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if *byte == b'\n' {
                if !self.oversized_line {
                    self.observe_line();
                }
                self.pending_line.clear();
                self.oversized_line = false;
            } else if self.pending_line.len() < CLAUDE_ERROR_EVIDENCE_LIMIT {
                self.pending_line.push(*byte);
            } else {
                self.oversized_line = true;
                self.oversized_event = true;
            }
        }
    }

    fn observe_line(&mut self) {
        let line = self
            .pending_line
            .strip_suffix(b"\r")
            .unwrap_or(&self.pending_line);
        if line.is_empty() {
            self.finish_event();
        } else if let Some(event_name) = line.strip_prefix(b"event:") {
            self.event_name = std::str::from_utf8(event_name)
                .ok()
                .map(|name| name.trim().to_owned());
        } else if let Some(data) = line.strip_prefix(b"data:") {
            let data = data.strip_prefix(b" ").unwrap_or(data);
            let separator_length = usize::from(!self.event_data.is_empty());
            if data.len().saturating_add(separator_length)
                <= CLAUDE_ERROR_EVIDENCE_LIMIT.saturating_sub(self.event_data.len())
            {
                if !self.event_data.is_empty() {
                    self.event_data.push(b'\n');
                }
                self.event_data.extend_from_slice(data);
            } else {
                self.oversized_event = true;
            }
        }
    }

    fn finish_event(&mut self) {
        let event_type = if self.oversized_event {
            None
        } else {
            serde_json::from_slice::<ClaudeEventType>(&self.event_data).ok()
        };
        let data_name = event_type.as_ref().map(|value| value.event_type.as_str());
        if self.event_name.as_deref() == Some("error") || data_name == Some("error") {
            self.provider_error = true;
        }
        if data_name == Some("message_stop")
            && self
                .event_name
                .as_deref()
                .is_none_or(|name| name == "message_stop")
        {
            self.message_stopped = true;
        }
        self.event_name = None;
        self.event_data.clear();
        self.oversized_event = false;
    }

    const fn completion(&self) -> ClaudeResponseCompletion {
        if self.provider_error {
            ClaudeResponseCompletion::ProviderError
        } else if self.message_stopped {
            ClaudeResponseCompletion::Success
        } else {
            ClaudeResponseCompletion::Incomplete
        }
    }
}

#[cfg(test)]
mod tests;
