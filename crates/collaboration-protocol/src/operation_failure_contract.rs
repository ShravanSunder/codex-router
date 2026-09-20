//! Shared programmatic failure evidence emitted by every public adapter.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationFailureKind {
    UnsupportedCapability,
    Unavailable,
    ProtocolViolation,
    Timeout,
    Rejected,
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationEffect {
    None,
    Unknown,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdapterOperationFailure {
    pub kind: OperationFailureKind,
    pub service_kind: Option<String>,
    pub stage: String,
    pub effect: OperationEffect,
    pub message: String,
    pub code: Option<i64>,
    pub data: Option<Value>,
}
