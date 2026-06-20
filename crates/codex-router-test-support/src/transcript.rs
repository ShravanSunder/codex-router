//! Protocol transcript helpers.

/// Captured request transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

impl TranscriptRequest {
    /// Creates a transcript request.
    #[must_use]
    pub fn new(method: impl Into<String>, path: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            body,
        }
    }

    /// Returns body bytes.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}
