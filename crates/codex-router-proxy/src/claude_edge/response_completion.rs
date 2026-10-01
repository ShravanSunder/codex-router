//! Typed Claude response completion evidence consumed by OnSuccess pin publication.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::Context;
use std::task::Poll;

use bytes::Bytes;
use futures_util::future::BoxFuture;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper::body::Body;
use hyper::body::Frame;
use serde::Deserialize;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use super::content_encoding::decoder_for_chunks;
use super::content_encoding::response_content_encodings;
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
    let mut observed = CompletionObservingBody {
        body,
        sender: Some(sender),
        tracker: None,
        compressed_sender: None,
        decoded_completion: None,
        pending_compressed_frame: Mutex::new(None),
        terminal_compressed_frame: None,
        body_ended: false,
    };
    if streaming {
        match response_content_encodings(&headers) {
            Ok(encodings) if encodings.is_empty() => {
                observed.tracker = Some(SseCompletionTracker::default());
            }
            Ok(encodings) => {
                let (compressed_sender, compressed_receiver) = mpsc::channel(1);
                let (completion_sender, completion_receiver) = oneshot::channel();
                let decoder = decoder_for_chunks(compressed_receiver, &encodings);
                tokio::spawn(async move {
                    let _result =
                        completion_sender.send(observe_decoded_sse_completion(decoder).await);
                });
                observed.compressed_sender = Some(compressed_sender);
                observed.decoded_completion = Some(completion_receiver);
            }
            Err(_error) => observed.finish(ClaudeResponseCompletion::Incomplete),
        }
    }
    if observed.body.is_end_stream() {
        if observed.compressed_sender.is_some() {
            observed.end_compressed_input();
            observed.body_ended = true;
        } else {
            observed.finish_completed_body();
            observed.body_ended = true;
        }
    }
    (
        AsyncStreamingHttpProxyResponse::new(status, headers, observed.boxed()),
        receiver,
    )
}

struct CompletionObservingBody {
    body: BoxBody<Bytes, AsyncHttpBodyError>,
    sender: Option<oneshot::Sender<ClaudeResponseCompletion>>,
    tracker: Option<SseCompletionTracker>,
    compressed_sender: Option<mpsc::Sender<Bytes>>,
    decoded_completion: Option<oneshot::Receiver<ClaudeResponseCompletion>>,
    pending_compressed_frame: Mutex<Option<PendingCompressedFrame>>,
    terminal_compressed_frame: Option<Frame<Bytes>>,
    body_ended: bool,
}

struct PendingCompressedFrame {
    frame: Frame<Bytes>,
    bytes: Bytes,
    finishes_body: bool,
    permit: BoxFuture<'static, Result<mpsc::OwnedPermit<Bytes>, mpsc::error::SendError<()>>>,
}

impl CompletionObservingBody {
    fn finish_completed_body(&mut self) {
        if let Some(tracker) = &mut self.tracker {
            tracker.finish_at_eof();
        }
        let completion = self.tracker.as_ref().map_or(
            ClaudeResponseCompletion::Success,
            SseCompletionTracker::completion,
        );
        self.finish(completion);
    }

    fn finish(&mut self, completion: ClaudeResponseCompletion) {
        if let Some(sender) = self.sender.take() {
            let _result = sender.send(completion);
        }
    }

    fn end_compressed_input(&mut self) {
        self.compressed_sender.take();
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
        let this = self.as_mut().get_mut();
        loop {
            if this.body_ended {
                if let Some(decoded_completion) = &mut this.decoded_completion {
                    match Pin::new(decoded_completion).poll(context) {
                        Poll::Ready(Ok(completion)) => {
                            this.decoded_completion.take();
                            this.finish(completion);
                        }
                        Poll::Ready(Err(_error)) => {
                            this.decoded_completion.take();
                            this.finish(ClaudeResponseCompletion::Incomplete);
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                return this
                    .terminal_compressed_frame
                    .take()
                    .map_or(Poll::Ready(None), |frame| Poll::Ready(Some(Ok(frame))));
            }

            let pending = this
                .pending_compressed_frame
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(mut pending) = pending {
                match pending.permit.as_mut().poll(context) {
                    Poll::Ready(Ok(permit)) => {
                        permit.send(pending.bytes);
                        if pending.finishes_body {
                            this.end_compressed_input();
                            this.body_ended = true;
                            this.terminal_compressed_frame = Some(pending.frame);
                            continue;
                        }
                        return Poll::Ready(Some(Ok(pending.frame)));
                    }
                    Poll::Ready(Err(_error)) => {
                        this.end_compressed_input();
                        this.finish(ClaudeResponseCompletion::Incomplete);
                        if pending.finishes_body {
                            this.body_ended = true;
                        }
                        return Poll::Ready(Some(Ok(pending.frame)));
                    }
                    Poll::Pending => {
                        *this
                            .pending_compressed_frame
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pending);
                        return Poll::Pending;
                    }
                }
            }

            match Pin::new(&mut this.body).poll_frame(context) {
                Poll::Ready(Some(Ok(frame))) => {
                    let finishes_body = this.body.is_end_stream();
                    if let (Some(compressed_sender), Some(bytes)) =
                        (&this.compressed_sender, frame.data_ref().cloned())
                    {
                        *this
                            .pending_compressed_frame
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some(PendingCompressedFrame {
                                frame,
                                bytes,
                                finishes_body,
                                permit: Box::pin(compressed_sender.clone().reserve_owned()),
                            });
                        continue;
                    }
                    if let (Some(tracker), Some(bytes)) = (&mut this.tracker, frame.data_ref()) {
                        tracker.observe(bytes);
                    }
                    if finishes_body {
                        if this.compressed_sender.is_some() {
                            this.end_compressed_input();
                            this.body_ended = true;
                            this.terminal_compressed_frame = Some(frame);
                            continue;
                        }
                        this.finish_completed_body();
                        this.body_ended = true;
                    }
                    return Poll::Ready(Some(Ok(frame)));
                }
                Poll::Ready(Some(Err(error))) => {
                    this.end_compressed_input();
                    this.decoded_completion.take();
                    this.finish(ClaudeResponseCompletion::Incomplete);
                    this.body_ended = true;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    if this.compressed_sender.is_some() {
                        this.end_compressed_input();
                        this.body_ended = true;
                        continue;
                    }
                    this.finish_completed_body();
                    this.body_ended = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body_ended && self.sender.is_none() && self.terminal_compressed_frame.is_none()
    }
}

async fn observe_decoded_sse_completion(
    mut reader: Pin<Box<dyn AsyncRead + Send>>,
) -> ClaudeResponseCompletion {
    let mut tracker = SseCompletionTracker::default();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                tracker.finish_at_eof();
                return tracker.completion();
            }
            Ok(count) => match buffer.get(..count) {
                Some(decoded_bytes) => tracker.observe(decoded_bytes),
                None => return ClaudeResponseCompletion::Incomplete,
            },
            Err(_error) => return tracker.incomplete_after_decode_error(),
        }
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

    fn finish_at_eof(&mut self) {
        if !self.pending_line.is_empty() && !self.oversized_line {
            self.observe_line();
        }
        self.pending_line.clear();
        self.oversized_line = false;
        self.finish_event();
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

    const fn incomplete_after_decode_error(&self) -> ClaudeResponseCompletion {
        if self.provider_error {
            ClaudeResponseCompletion::ProviderError
        } else {
            ClaudeResponseCompletion::Incomplete
        }
    }
}

#[cfg(test)]
mod tests;
