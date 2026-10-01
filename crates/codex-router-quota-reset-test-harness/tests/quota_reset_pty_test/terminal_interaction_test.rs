use std::ffi::OsStr;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use portable_pty::Child;
use portable_pty::CommandBuilder;
use portable_pty::MasterPty;
use portable_pty::PtySize;
use portable_pty::native_pty_system;

use super::isolated_fixture_test::TestResult;

const INITIAL_SIZE: PtySize = PtySize {
    rows: 36,
    cols: 120,
    pixel_width: 0,
    pixel_height: 0,
};

pub(super) struct TerminalDriver {
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    output_receiver: mpsc::Receiver<ReaderEvent>,
    reader_thread: Option<thread::JoinHandle<()>>,
    transcript: Vec<u8>,
    reached_eof: bool,
}

enum ReaderEvent {
    Bytes(Vec<u8>),
    Eof,
}

impl TerminalDriver {
    pub(super) fn spawn(
        program: &Path,
        arguments: impl IntoIterator<Item = impl AsRef<OsStr>>,
        current_directory: &Path,
    ) -> TestResult<Self> {
        let pair = native_pty_system().openpty(INITIAL_SIZE)?;
        let mut command = CommandBuilder::new(program);
        command.args(arguments);
        command.env_clear();
        command.env("HOME", std::env::temp_dir());
        command.env("TERM", "xterm-256color");
        command.env("LANG", "C.UTF-8");
        command.env("TMPDIR", std::env::temp_dir());
        command.cwd(current_directory);
        let child = pair.slave.spawn_command(command)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (output_sender, output_receiver) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(bytes) => {
                        let Some(bytes) = chunk.get(..bytes) else {
                            break;
                        };
                        if output_sender
                            .send(ReaderEvent::Bytes(bytes.to_vec()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) if error.raw_os_error() == Some(5) => break,
                    Err(_error) => break,
                }
            }
            let _ = output_sender.send(ReaderEvent::Eof);
        });
        Ok(Self {
            master: Some(pair.master),
            writer: Some(writer),
            child: Some(child),
            output_receiver,
            reader_thread: Some(reader_thread),
            transcript: Vec::new(),
            reached_eof: false,
        })
    }

    pub(super) fn wait_for_text(&mut self, expected: &str, timeout: Duration) -> TestResult<()> {
        self.wait_until(timeout, |transcript| {
            String::from_utf8_lossy(transcript).contains(expected)
        })
    }

    pub(super) fn transcript_len(&mut self) -> usize {
        self.drain_pending_output();
        self.transcript.len()
    }

    pub(super) fn safe_semantic_diagnostics(&mut self, start: usize) -> String {
        self.drain_pending_output();
        let child_running = self.child_is_running().ok();
        let tail = self.transcript.get(start..).unwrap_or_default();
        let terminal_bytes = String::from_utf8_lossy(tail);
        format!(
            "tail_bytes={} fullscreen_entered={} keyboard_probe={} device_attributes_probe={} mouse_capture_enabled={} synchronized_update_started={} inspecting_footer={} browse_footer={} reset_title={} eof={} child_running={:?}",
            tail.len(),
            terminal_bytes.contains("\u{1b}[?1049h"),
            terminal_bytes.contains("\u{1b}[?u"),
            terminal_bytes.contains("\u{1b}[c"),
            terminal_bytes.contains("\u{1b}[?1003h") && terminal_bytes.contains("\u{1b}[?1006h"),
            terminal_bytes.contains("\u{1b}[?2026h"),
            terminal_bytes.contains("esc/ctrl-r back"),
            terminal_bytes.contains("ctrl-r account options"),
            terminal_bytes.contains("Reset credit"),
            self.reached_eof,
            child_running,
        )
    }

    pub(super) fn wait_for_text_after(
        &mut self,
        expected: &str,
        start: usize,
        timeout: Duration,
    ) -> TestResult<()> {
        self.wait_until(timeout, |transcript| {
            transcript.get(start..).is_some_and(|tail| {
                contains_completed_synchronized_frame_after_text(tail, expected.as_bytes())
            })
        })
    }

    pub(super) fn resize(&self, rows: u16, cols: u16) -> TestResult<()> {
        self.master_ref()?.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    pub(super) fn send(&mut self, bytes: &[u8]) -> TestResult<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| std::io::Error::other("PTY writer is unavailable"))?;
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub(super) fn send_sgr_mouse_left_down(&mut self, column: u16, row: u16) -> TestResult<()> {
        let sequence = format!("\u{1b}[<0;{};{}M", column + 1, row + 1);
        self.send(sequence.as_bytes())
    }

    pub(super) fn child_is_running(&mut self) -> TestResult<bool> {
        Ok(self
            .child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("PTY child is unavailable"))?
            .try_wait()?
            .is_none())
    }

    pub(super) fn finish(mut self, timeout: Duration) -> TestResult<Vec<u8>> {
        self.wait_for_eof(timeout)?;
        let status = self
            .child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("PTY child is unavailable"))?
            .wait()?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "PTY child failed with exit code {}",
                status.exit_code()
            ))
            .into());
        }
        self.child.take();
        self.writer.take();
        self.master.take();
        if let Some(reader_thread) = self.reader_thread.take() {
            reader_thread
                .join()
                .map_err(|_panic| std::io::Error::other("PTY reader thread panicked"))?;
        }
        Ok(std::mem::take(&mut self.transcript))
    }

    pub(super) fn terminate_and_reap(mut self, timeout: Duration) -> TestResult<Vec<u8>> {
        let child = self
            .child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("PTY child is unavailable"))?;
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        let deadline = Instant::now() + timeout;
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::other("PTY child termination exceeded timeout").into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.child.take();
        self.writer.take();
        self.master.take();
        if let Some(reader_thread) = self.reader_thread.take() {
            reader_thread
                .join()
                .map_err(|_panic| std::io::Error::other("PTY reader thread panicked"))?;
        }
        while let Ok(event) = self.output_receiver.try_recv() {
            if let ReaderEvent::Bytes(bytes) = event {
                self.transcript.extend(bytes);
            }
        }
        Ok(std::mem::take(&mut self.transcript))
    }

    fn master_ref(&self) -> TestResult<&(dyn MasterPty + Send)> {
        self.master
            .as_deref()
            .ok_or_else(|| std::io::Error::other("PTY master is unavailable").into())
    }

    fn wait_for_eof(&mut self, timeout: Duration) -> TestResult<()> {
        let deadline = Instant::now() + timeout;
        while !self.reached_eof {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.output_receiver.recv_timeout(remaining)? {
                ReaderEvent::Bytes(bytes) => self.transcript.extend(bytes),
                ReaderEvent::Eof => self.reached_eof = true,
            }
        }
        Ok(())
    }

    fn wait_until(
        &mut self,
        timeout: Duration,
        condition: impl Fn(&[u8]) -> bool,
    ) -> TestResult<()> {
        let deadline = Instant::now() + timeout;
        while !condition(&self.transcript) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.output_receiver.recv_timeout(remaining)? {
                ReaderEvent::Bytes(bytes) => self.transcript.extend(bytes),
                ReaderEvent::Eof => self.reached_eof = true,
            }
            if self.reached_eof && !condition(&self.transcript) {
                return Err(std::io::Error::other(
                    "PTY child exited before expected semantic output",
                )
                .into());
            }
        }
        Ok(())
    }

    fn drain_pending_output(&mut self) {
        while let Ok(event) = self.output_receiver.try_recv() {
            match event {
                ReaderEvent::Bytes(bytes) => self.transcript.extend(bytes),
                ReaderEvent::Eof => self.reached_eof = true,
            }
        }
    }
}

fn contains_completed_synchronized_frame_after_text(transcript: &[u8], expected: &[u8]) -> bool {
    if expected.is_empty() {
        return false;
    }

    const BEGIN: &[u8] = b"\x1b[?2026h";
    const END: &[u8] = b"\x1b[?2026l";

    let mut offset = 0;
    let mut synchronized_update_depth = 0;
    let mut expected_text_seen_in_update = false;
    while offset < transcript.len() {
        let remaining = transcript.get(offset..).unwrap_or_default();
        let next_text = find_sequence(remaining, expected).map(|position| offset + position);
        let next_begin = find_sequence(remaining, BEGIN).map(|position| offset + position);
        let next_end = find_sequence(remaining, END).map(|position| offset + position);
        let next_event = [next_text, next_begin, next_end]
            .into_iter()
            .flatten()
            .min();

        let Some(event_offset) = next_event else {
            break;
        };
        if next_text == Some(event_offset) {
            expected_text_seen_in_update |= synchronized_update_depth > 0;
            offset = event_offset + expected.len();
        } else if next_begin == Some(event_offset) {
            if synchronized_update_depth == 0 {
                expected_text_seen_in_update = false;
            }
            synchronized_update_depth += 1;
            offset = event_offset + BEGIN.len();
        } else {
            if synchronized_update_depth > 0 {
                synchronized_update_depth -= 1;
                if synchronized_update_depth == 0 && expected_text_seen_in_update {
                    return true;
                }
                if synchronized_update_depth == 0 {
                    expected_text_seen_in_update = false;
                }
            }
            offset = event_offset + END.len();
        }
    }

    false
}

fn find_sequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

impl Drop for TerminalDriver {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        self.child.take();
        self.writer.take();
        self.master.take();
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_boundary_includes_bytes_already_queued_by_pty_reader() -> TestResult<()> {
        let (output_sender, output_receiver) = mpsc::channel();
        let queued_frame = b"Reset credit".to_vec();
        output_sender.send(ReaderEvent::Bytes(queued_frame.clone()))?;
        let mut terminal = TerminalDriver {
            master: None,
            writer: None,
            child: None,
            output_receiver,
            reader_thread: None,
            transcript: Vec::new(),
            reached_eof: false,
        };

        if terminal.transcript_len() != queued_frame.len() {
            return Err(std::io::Error::other(
                "transcript boundary excluded bytes already queued by PTY reader",
            )
            .into());
        }
        Ok(())
    }
}
