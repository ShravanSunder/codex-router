//! Clipboard command output belongs to this terminal, including an SSH terminal.
use std::{io, sync::Arc};

pub(super) type SessionIdCopier = Arc<dyn Fn(&str) -> io::Result<()> + Send + Sync>;

pub(super) fn terminal_session_id_copier() -> SessionIdCopier {
    Arc::new(|session_id| write_session_id_copy(&mut io::stdout(), session_id))
}

pub(super) fn write_session_id_copy(
    writer: &mut impl io::Write,
    session_id: &str,
) -> io::Result<()> {
    use crossterm::{ExecutableCommand, clipboard::CopyToClipboard};
    writer.execute(CopyToClipboard::to_clipboard_from(session_id))?;
    Ok(())
}

pub(super) fn unavailable_session_id_copier() -> SessionIdCopier {
    Arc::new(|_| {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "clipboard writer unavailable",
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_details_navigation_copy_uses_exact_terminal_clipboard_command() {
        let mut bytes = Vec::new();
        write_session_id_copy(&mut bytes, "thread-a").unwrap();
        assert_eq!(bytes, b"\x1b]52;c;dGhyZWFkLWE=\x1b\\");
    }

    #[test]
    fn owner_details_navigation_copy_returns_writer_failure() {
        struct FailingWriter;
        impl io::Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            write_session_id_copy(&mut FailingWriter, "thread-a")
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}
