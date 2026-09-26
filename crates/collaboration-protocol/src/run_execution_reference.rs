//! Public execution identity names the route and exact effect observed for a Run.
use crate::{ObservationTimestamp, OperationId, PositiveSeconds, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeExecution {
    pub target: SessionRef,
    #[serde(rename = "nativeTurnId")]
    pub turn_id: String,
    pub started_at: ObservationTimestamp,
    pub deadline_at: ObservationTimestamp,
    pub effective_timeout_seconds: PositiveSeconds,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RunExecution {
    CodexAppServer(NativeExecution),
    ProviderAcp {
        target: SessionRef,
        operation_id: OperationId,
        started_at: ObservationTimestamp,
        deadline_at: ObservationTimestamp,
        effective_timeout_seconds: PositiveSeconds,
    },
    ClaudeCodePeer {
        target: SessionRef,
        written_at: ObservationTimestamp,
    },
}
