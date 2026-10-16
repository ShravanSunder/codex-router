//! Opt-in fixed transport witnesses; payloads and peer identities never reach this sink.
use futures_util::{Sink, io::AsyncRead, io::AsyncReadExt};
use serde::Deserialize;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

const DIAGNOSTIC_TARGET: &str = "acp_client_runtime::provider_diagnostics";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderFrameKind {
    Prompt,
    Cancel,
    Permission,
    Elicitation,
    SessionUpdate,
    Result,
    Error,
    Other,
    Malformed,
}
const FRAME_KINDS: [ProviderFrameKind; 9] = [
    ProviderFrameKind::Prompt,
    ProviderFrameKind::Cancel,
    ProviderFrameKind::Permission,
    ProviderFrameKind::Elicitation,
    ProviderFrameKind::SessionUpdate,
    ProviderFrameKind::Result,
    ProviderFrameKind::Error,
    ProviderFrameKind::Other,
    ProviderFrameKind::Malformed,
];
impl ProviderFrameKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Cancel => "cancel",
            Self::Permission => "permission",
            Self::Elicitation => "elicitation",
            Self::SessionUpdate => "sessionUpdate",
            Self::Result => "result",
            Self::Error => "error",
            Self::Other => "other",
            Self::Malformed => "malformed",
        }
    }
    const fn index(self) -> usize {
        match self {
            Self::Prompt => 0,
            Self::Cancel => 1,
            Self::Permission => 2,
            Self::Elicitation => 3,
            Self::SessionUpdate => 4,
            Self::Result => 5,
            Self::Error => 6,
            Self::Other => 7,
            Self::Malformed => 8,
        }
    }
}

/// Inspect only the protocol envelope, decoding the method and skipping all payloads.
#[derive(Deserialize)]
struct DiagnosticEnvelope {
    method: Option<String>,
    #[serde(default)]
    result: DiagnosticFieldPresence,
    #[serde(default)]
    error: DiagnosticFieldPresence,
}

#[derive(Default)]
enum DiagnosticFieldPresence {
    #[default]
    Absent,
    Present,
}

impl<'de> Deserialize<'de> for DiagnosticFieldPresence {
    fn deserialize<Decoder: serde::Deserializer<'de>>(
        decoder: Decoder,
    ) -> Result<Self, Decoder::Error> {
        // JSON null is still a present result/error field; only a missing key defaults.
        serde::de::IgnoredAny::deserialize(decoder).map(|_| Self::Present)
    }
}
fn classify_frame(frame: &str) -> ProviderFrameKind {
    let Ok(envelope) = serde_json::from_str::<DiagnosticEnvelope>(frame) else {
        return ProviderFrameKind::Malformed;
    };
    match envelope.method.as_deref() {
        Some("session/prompt") => ProviderFrameKind::Prompt,
        Some("session/cancel") => ProviderFrameKind::Cancel,
        Some("session/request_permission") => ProviderFrameKind::Permission,
        Some("elicitation/create") => ProviderFrameKind::Elicitation,
        Some("session/update") => ProviderFrameKind::SessionUpdate,
        Some(_) => ProviderFrameKind::Other,
        None if matches!(envelope.error, DiagnosticFieldPresence::Present) => {
            ProviderFrameKind::Error
        }
        None if matches!(envelope.result, DiagnosticFieldPresence::Present) => {
            ProviderFrameKind::Result
        }
        None => ProviderFrameKind::Other,
    }
}

fn enabled() -> bool {
    cfg!(debug_assertions) && tracing::enabled!(target: DIAGNOSTIC_TARGET, tracing::Level::DEBUG)
}
fn record(stage: &'static str, kind: ProviderFrameKind, count: usize) {
    if enabled() {
        tracing::debug!(target: DIAGNOSTIC_TARGET, {
            event.name = "codex_router.acp.transport_diagnostic",
            transport.stage = stage, frame.kind = kind.label(), observed_count = count,
        }, "Observed ACP transport boundary");
    }
}
pub(crate) fn record_prompt_enqueued() {
    record("promptEnqueued", ProviderFrameKind::Prompt, 1);
}
pub(crate) fn record_callback_entered(kind: ProviderFrameKind) {
    record("callbackEntered", kind, 1);
}
pub(crate) fn record_incoming_frame(frame: &str) {
    if enabled() {
        record("incomingFrame", classify_frame(frame), 1);
    }
}
pub(crate) fn record_incoming_error() {
    record("incomingReadError", ProviderFrameKind::Other, 1);
}

/// The original sink remains responsible for IO, buffering and backpressure.
/// Only a successful inner `poll_flush` produces a flushed witness.
pub(crate) struct ObservedProviderSink<Inner> {
    inner: Inner,
    pending_frame_counts: [usize; 9],
}
impl<Inner> ObservedProviderSink<Inner> {
    pub(crate) const fn new(inner: Inner) -> Self {
        Self {
            inner,
            pending_frame_counts: [0; 9],
        }
    }
    fn record_flushed(&mut self) {
        for (kind, pending_count) in FRAME_KINDS
            .into_iter()
            .zip(self.pending_frame_counts.iter_mut())
        {
            let count = std::mem::take(pending_count);
            if count != 0 {
                record("outgoingFlushed", kind, count);
            }
        }
    }
}
impl<Inner: Sink<String, Error = io::Error> + Unpin> Sink<String> for ObservedProviderSink<Inner> {
    type Error = io::Error;
    fn poll_ready(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_ready(context)
    }
    fn start_send(self: Pin<&mut Self>, frame: String) -> Result<(), Self::Error> {
        let this = self.get_mut();
        let observed = enabled();
        let kind = if observed {
            classify_frame(&frame)
        } else {
            ProviderFrameKind::Other
        };
        Pin::new(&mut this.inner).start_send(frame)?;
        if observed {
            if let Some(count) = this.pending_frame_counts.get_mut(kind.index()) {
                *count = count.saturating_add(1);
            }
            record("outgoingBuffered", kind, 1);
        }
        Ok(())
    }
    fn poll_flush(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_flush(context) {
            Poll::Ready(Ok(())) => {
                this.record_flushed();
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => {
                record("outgoingFlushError", ProviderFrameKind::Other, 1);
                Poll::Ready(Err(error))
            }
            Poll::Pending => Poll::Pending,
        }
    }
    fn poll_close(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        // Closing is not relabelled a successful flush.
        Pin::new(&mut self.get_mut().inner).poll_close(context)
    }
}

pub(crate) async fn drain_provider_stderr<Reader: AsyncRead + Unpin>(mut reader: Reader) {
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                record("stderrClosed", ProviderFrameKind::Other, 1);
                break;
            }
            Ok(bytes) => record("stderrBytes", ProviderFrameKind::Other, bytes),
            Err(error) => {
                if enabled() {
                    // ErrorKind carries no provider-controlled text or identity.
                    tracing::debug!(target: DIAGNOSTIC_TARGET, {
                        event.name = "codex_router.acp.transport_diagnostic",
                        transport.stage = "stderrReadError", error.kind = ?error.kind(),
                    }, "Observed ACP stderr read failure");
                }
                break;
            }
        }
    }
}

#[cfg(test)]
#[path = "provider_transport_diagnostic_tests.rs"]
mod tests;
