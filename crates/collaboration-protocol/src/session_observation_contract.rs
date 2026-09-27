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
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionListenRequest {
    pub target: SessionRef,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionListenReady {
    pub target: SessionRef,
    pub generation: CodexGeneration,
}
