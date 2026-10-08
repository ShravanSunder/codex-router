//! Bounded observation shared by native and external provider Sessions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CodexGeneration, SessionRef};

#[derive(JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObservationEndReason {
    DeadlineReached,
    ResultLimitReached,
    CallerCancelled,
    BackendDisconnected,
    ResyncRequired,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoundedObservationResult {
    pub target: SessionRef,
    pub generation: CodexGeneration,
    pub attached: bool,
    pub events: Vec<Value>,
    pub end_reason: ObservationEndReason,
    pub continuation_gap: bool,
    /// Present for provider hub history; native results omit this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoundedObservationRequest {
    pub target: SessionRef,
    #[schemars(range(min = 1))]
    pub timeout_seconds: u64,
    #[schemars(range(min = 1, max = 4096))]
    pub max_events: usize,
    #[schemars(range(min = 1, max = 1048576))]
    pub max_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
}

/// A retained event could not fit inside one Control frame after JSON encoding.
#[derive(JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderObservationEventTooLarge {
    pub kind: ProviderObservationEventTooLargeKind,
    pub sequence: u64,
    pub item_id: Option<String>,
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderObservationEventTooLargeKind {
    EventTooLarge,
}

#[cfg(test)]
mod tests {
    use super::{BoundedObservationRequest, ProviderObservationEventTooLarge};
    use serde_json::json;

    #[test]
    fn paging_fields_are_optional_and_marker_has_typed_kind() {
        let base = json!({
            "target": {
                "endpoint": {"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
                "sessionId":"session-one"
            },
            "timeoutSeconds": 1,
            "maxEvents": 1,
            "maxBytes": 1024
        });
        let legacy: BoundedObservationRequest =
            serde_json::from_value(base.clone()).expect("legacy request");
        assert_eq!(serde_json::to_value(&legacy).expect("request JSON"), base);
        let paged = serde_json::from_value::<BoundedObservationRequest>(json!({
            "target": base["target"],
            "timeoutSeconds": 1,
            "maxEvents": 1,
            "maxBytes": 1024,
            "afterSequence": 7,
            "epoch": 3
        }))
        .expect("paged request");
        assert_eq!(paged.after_sequence, Some(7));
        assert_eq!(paged.epoch, Some(3));
        let marker: ProviderObservationEventTooLarge = serde_json::from_value(json!({
            "kind":"eventTooLarge","sequence":8,"itemId":"message"
        }))
        .expect("typed marker");
        assert_eq!(marker.sequence, 8);
        assert!(
            serde_json::from_value::<ProviderObservationEventTooLarge>(json!({
                "kind":"other","sequence":8,"itemId":"message"
            }))
            .is_err()
        );
    }
}
