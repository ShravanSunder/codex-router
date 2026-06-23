//! Shared route-band identifiers.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// Quota route band used by selection, proxy routing, state, and status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteBand {
    /// `/v1/responses`.
    Responses,
    /// `/v1/responses/compact`.
    ResponsesCompact,
    /// `/v1/models`.
    Models,
    /// `/v1/memories/trace_summarize`.
    MemoriesTraceSummarize,
}

impl RouteBand {
    /// Returns the stable storage/API name for this route band.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ResponsesCompact => "responses_compact",
            Self::Models => "models",
            Self::MemoriesTraceSummarize => "memories_trace_summarize",
        }
    }
}

impl fmt::Display for RouteBand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::RouteBand;

    #[test]
    fn route_band_has_stable_display_and_json_names() {
        let cases = [
            (RouteBand::Responses, "responses"),
            (RouteBand::ResponsesCompact, "responses_compact"),
            (RouteBand::Models, "models"),
            (
                RouteBand::MemoriesTraceSummarize,
                "memories_trace_summarize",
            ),
        ];

        for (route_band, expected_name) in cases {
            assert_eq!(route_band.as_str(), expected_name);
            assert_eq!(route_band.to_string(), expected_name);

            let serialized = match serde_json::to_string(&route_band) {
                Ok(serialized) => serialized,
                Err(error) => panic!("route band should serialize: {error}"),
            };
            assert_eq!(serialized, format!("\"{expected_name}\""));

            let deserialized: RouteBand = match serde_json::from_str(&serialized) {
                Ok(deserialized) => deserialized,
                Err(error) => panic!("route band should deserialize: {error}"),
            };
            assert_eq!(deserialized, route_band);
        }
    }
}
