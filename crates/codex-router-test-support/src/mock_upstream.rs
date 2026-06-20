//! Mock upstream transcript recorder.

use crate::transcript::TranscriptRequest;

/// In-memory upstream transcript.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MockUpstreamTranscript {
    requests: Vec<TranscriptRequest>,
}

impl MockUpstreamTranscript {
    /// Records a request.
    pub fn record(&mut self, request: TranscriptRequest) {
        self.requests.push(request);
    }

    /// Returns recorded requests.
    #[must_use]
    pub fn requests(&self) -> &[TranscriptRequest] {
        &self.requests
    }
}
