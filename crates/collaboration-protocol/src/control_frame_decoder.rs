//! Bounded Control JSON Lines framing; deliberately not a native Codex codec.
use serde_json::Value;

pub const MAX_CONTROL_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FrameError {
    #[error("Control frame exceeds the byte limit")]
    TooLarge,
    #[error("Control frame is not valid JSON")]
    InvalidJson,
    #[error("Control requires one JSON object per frame")]
    InvalidObject,
    #[error("Control stream ended inside a frame")]
    Truncated,
    #[error("Control decoder is closed")]
    Closed,
}

/// Owns only framing state. Request schemas and dispatch remain separate.
#[derive(Default)]
pub struct ControlFrameDecoder {
    pending: Vec<u8>,
    closed: bool,
}

impl ControlFrameDecoder {
    /// Accepts a bounded transport read. A failure makes this decoder terminal.
    pub fn push(&mut self, input: &[u8]) -> Result<Vec<Value>, FrameError> {
        if self.closed {
            return Err(FrameError::Closed);
        }
        let result = self.decode_input(input);
        if result.is_err() {
            self.pending.clear();
            self.closed = true;
        }
        result
    }

    fn decode_input(&mut self, input: &[u8]) -> Result<Vec<Value>, FrameError> {
        let mut frames = Vec::new();
        for segment in input.split_inclusive(|byte| *byte == b'\n') {
            let terminated = segment.last() == Some(&b'\n');
            let content = if terminated {
                segment.strip_suffix(b"\n").ok_or(FrameError::InvalidJson)?
            } else {
                segment
            };
            if content.len() > MAX_CONTROL_FRAME_BYTES.saturating_sub(self.pending.len()) {
                return Err(FrameError::TooLarge);
            }
            self.pending.extend_from_slice(content);
            if terminated {
                let value: Value =
                    serde_json::from_slice(&self.pending).map_err(|_| FrameError::InvalidJson)?;
                if !value.is_object() {
                    return Err(FrameError::InvalidObject);
                }
                self.pending.clear();
                frames.push(value);
            }
        }
        Ok(frames)
    }

    /// Marks EOF, rejecting unterminated content instead of implicitly parsing it.
    pub fn finish(&mut self) -> Result<(), FrameError> {
        if self.closed {
            return Err(FrameError::Closed);
        }
        self.closed = true;
        if self.pending.is_empty() {
            Ok(())
        } else {
            self.pending.clear();
            Err(FrameError::Truncated)
        }
    }

    #[must_use]
    pub fn buffered_bytes(&self) -> usize {
        self.pending.len()
    }
}
