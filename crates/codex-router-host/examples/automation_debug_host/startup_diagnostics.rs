//! Opt-in sink for fixed startup events and existing scrubbed child diagnostics.
use tracing_subscriber::{filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt};

pub(super) fn enable_startup_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_DEBUG_READINESS_TIMING").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        tracing_subscriber::registry()
            .with(diagnostic_layer(std::io::stderr))
            .try_init()?;
    }
    Ok(())
}

fn diagnostic_layer<Writer>(
    writer: Writer,
) -> impl tracing_subscriber::Layer<tracing_subscriber::Registry>
where
    Writer: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    use tracing_subscriber::Layer;
    tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer)
        .with_filter(filter_fn(|metadata| {
            matches!(
                metadata.target(),
                "codex_router_host::startup_diagnostics" | "codex_router_host::child_diagnostics"
            )
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    struct CapturedDiagnostics(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturedDiagnostics {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedDiagnostics {
        type Writer = Self;
        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn startup_sink_excludes_unrelated_private_events() {
        let capture = CapturedDiagnostics::default();
        let subscriber = tracing_subscriber::registry().with(diagnostic_layer(capture.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(private_payload = "PRIVATE_CANARY", "unrelated event");
            tracing::debug!(target: "codex_router_host::startup_diagnostics", {
                event.name = "codex_router.host.router_startup_child",
                child_state = "running", elapsed_ms = 250_u64 }, "Observed retained Router child");
        });
        let bytes = capture.0.lock().expect("capture lock").clone();
        let output = String::from_utf8(bytes).expect("UTF-8 diagnostic");
        assert!(output.contains("router_startup_child"), "{output}");
        assert!(!output.contains("PRIVATE_CANARY"), "{output}");
        assert!(!output.contains("private_payload"), "{output}");
    }
}
