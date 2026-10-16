use super::*;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};
use tracing_subscriber::{Layer, layer::SubscriberExt};

#[derive(Clone, Default)]
struct DiagnosticCapture(Arc<Mutex<Vec<u8>>>);
impl Write for DiagnosticCapture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("capture").extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for DiagnosticCapture {
    type Writer = Self;
    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}
impl DiagnosticCapture {
    fn output(&self) -> String {
        String::from_utf8(self.0.lock().expect("capture").clone()).expect("diagnostic UTF8")
    }
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync {
        tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(self.clone())
                .with_current_span(false)
                .with_span_list(false)
                .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                    metadata.target() == DIAGNOSTIC_TARGET
                })),
        )
    }
}

enum FlushDisposition {
    Pending,
    Ready,
    Error,
}
struct ControlledSink {
    disposition: FlushDisposition,
}
impl Sink<String> for ControlledSink {
    type Error = io::Error;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn start_send(self: Pin<&mut Self>, _: String) -> io::Result<()> {
        Ok(())
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.disposition {
            FlushDisposition::Pending => Poll::Pending,
            FlushDisposition::Ready => Poll::Ready(Ok(())),
            FlushDisposition::Error => Poll::Ready(Err(io::Error::other("PRIVATE_CANARY"))),
        }
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[test]
fn buffered_or_pending_prompt_is_never_reported_flushed() {
    let capture = DiagnosticCapture::default();
    tracing::subscriber::with_default(capture.subscriber(), || {
        let mut sink = ObservedProviderSink::new(ControlledSink {
            disposition: FlushDisposition::Pending,
        });
        let mut context = Context::from_waker(futures_util::task::noop_waker_ref());
        Pin::new(&mut sink)
            .start_send(r#"{"method":"session/prompt","params":{"text":"PRIVATE_CANARY"}}"#.into())
            .expect("buffered");
        assert!(Pin::new(&mut sink).poll_flush(&mut context).is_pending());
        assert!(capture.output().contains("outgoingBuffered"));
        assert!(!capture.output().contains("outgoingFlushed"));
        sink.inner.disposition = FlushDisposition::Ready;
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut context),
            Poll::Ready(Ok(()))
        ));
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut context),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(capture.output().matches("outgoingFlushed").count(), 1);
        assert!(!capture.output().contains("PRIVATE_CANARY"));
    });
}

#[test]
fn flush_failure_is_distinct_from_delivery_and_scrubs_error_text() {
    let capture = DiagnosticCapture::default();
    tracing::subscriber::with_default(capture.subscriber(), || {
        let mut sink = ObservedProviderSink::new(ControlledSink {
            disposition: FlushDisposition::Error,
        });
        let mut context = Context::from_waker(futures_util::task::noop_waker_ref());
        Pin::new(&mut sink)
            .start_send(r#"{"method":"session/prompt"}"#.into())
            .expect("buffered");
        assert!(matches!(
            Pin::new(&mut sink).poll_flush(&mut context),
            Poll::Ready(Err(_))
        ));
        assert!(capture.output().contains("outgoingFlushError"));
        assert!(!capture.output().contains("outgoingFlushed"));
        assert!(!capture.output().contains("PRIVATE_CANARY"));
    });
}

#[test]
fn envelope_classes_never_export_arbitrary_methods_or_payloads() {
    for (frame, expected) in [
        (
            r#"{"method":"session\u002fprompt"}"#,
            ProviderFrameKind::Prompt,
        ),
        (r#"{"result":null}"#, ProviderFrameKind::Result),
        (r#"{"error":null}"#, ProviderFrameKind::Error),
        (
            r#"{"method":"session/request_permission","params":{"secret":"PRIVATE_CANARY"}}"#,
            ProviderFrameKind::Permission,
        ),
        (
            r#"{"method":"elicitation/create","id":"PRIVATE_CANARY"}"#,
            ProviderFrameKind::Elicitation,
        ),
        (
            r#"{"method":"session/update","params":{"update":{"text":"PRIVATE_CANARY"}}}"#,
            ProviderFrameKind::SessionUpdate,
        ),
        (
            r#"{"result":{"text":"PRIVATE_CANARY"}}"#,
            ProviderFrameKind::Result,
        ),
        (
            r#"{"error":{"message":"PRIVATE_CANARY"}}"#,
            ProviderFrameKind::Error,
        ),
        (r#"{"method":"PRIVATE_CANARY"}"#, ProviderFrameKind::Other),
        ("PRIVATE_CANARY broken JSON", ProviderFrameKind::Malformed),
    ] {
        assert_eq!(classify_frame(frame), expected);
        assert!(!expected.label().contains("PRIVATE_CANARY"));
    }
}

struct FailedStderr;
impl AsyncRead for FailedStderr {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "PRIVATE_CANARY",
        )))
    }
}
#[tokio::test]
async fn stderr_is_continuously_drained_without_retaining_bytes_or_read_error_text() {
    let capture = DiagnosticCapture::default();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    let bytes = b"PRIVATE_CANARY".repeat(16384);
    let expected_bytes = bytes.len();
    drain_provider_stderr(futures_util::io::Cursor::new(bytes)).await;
    drain_provider_stderr(FailedStderr).await;
    let output = capture.output();
    let observed_bytes: u64 = output
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON diagnostic"))
        .filter(|event| event["fields"]["transport.stage"] == "stderrBytes")
        .map(|event| event["fields"]["observed_count"].as_u64().expect("count"))
        .sum();
    assert_eq!(
        observed_bytes,
        u64::try_from(expected_bytes).expect("bounded bytes")
    );
    assert!(output.contains("stderrClosed"));
    assert!(output.contains("stderrReadError"));
    assert!(output.contains("BrokenPipe"));
    assert!(!output.contains("PRIVATE_CANARY"));
}
