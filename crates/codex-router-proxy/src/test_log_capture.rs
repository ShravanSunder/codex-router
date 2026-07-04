//! Test-only tracing capture serialized across proxy modules.

use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;

static LOG_CAPTURE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn capture_log_output(emit: impl FnOnce()) -> String {
    let _guard = LOG_CAPTURE_LOCK
        .lock()
        .unwrap_or_else(|error| panic!("log capture lock should be available: {error}"));
    let captured = CapturedLogWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();

    tracing::subscriber::with_default(subscriber, emit);
    captured.rendered()
}

#[derive(Clone, Default)]
struct CapturedLogWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl CapturedLogWriter {
    fn rendered(&self) -> String {
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(|error| panic!("captured log lock should be available: {error}"))
            .clone();
        String::from_utf8(bytes)
            .unwrap_or_else(|error| panic!("captured log should be utf-8: {error}"))
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogWriter {
    type Writer = CapturedLogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        CapturedLogBuffer {
            bytes: Arc::clone(&self.bytes),
        }
    }
}

struct CapturedLogBuffer {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl Write for CapturedLogBuffer {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let mut bytes = self
            .bytes
            .lock()
            .map_err(|_| std::io::Error::other("captured log lock poisoned"))?;
        bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
