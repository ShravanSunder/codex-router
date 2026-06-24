//! Test support helpers for codex-router.

pub mod installed_codex;
pub mod mock_upstream;
pub mod route_native;
pub mod transcript;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-test-support"
}

#[cfg(test)]
mod tests {
    use super::package_name;
    use crate::mock_upstream::MockUpstreamTranscript;
    use crate::transcript::TranscriptRequest;

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-test-support");
    }

    #[test]
    fn mock_upstream_records_transcript_without_interpreting_body() {
        let mut upstream = MockUpstreamTranscript::default();
        let request = TranscriptRequest::new(
            "POST",
            "/v1/responses",
            br#"{"unknown_codex_field":true}"#.to_vec(),
        );

        upstream.record(request.clone());

        assert_eq!(upstream.requests(), &[request]);
    }
}
