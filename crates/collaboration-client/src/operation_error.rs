//! Stable programmatic classification for SDK operation failures.

use crate::ClientError;
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
pub struct OperationFailure {
    pub kind: OperationFailureKind,
    pub stage: String,
    pub effect: OperationEffect,
    pub message: String,
    pub code: Option<i64>,
    pub data: Option<Value>,
}

impl OperationFailure {
    #[must_use]
    pub fn from_client_error(error: ClientError, effect: OperationEffect) -> Self {
        let (kind, stage, code, data) = match &error {
            ClientError::InvalidRequest(_) => (
                OperationFailureKind::ProtocolViolation,
                "validation",
                None,
                None,
            ),
            ClientError::UnsupportedCapability(_) => (
                OperationFailureKind::UnsupportedCapability,
                "validation",
                None,
                None,
            ),
            ClientError::Discovery { stage, .. } => {
                (OperationFailureKind::Unavailable, *stage, None, None)
            }
            ClientError::Transport(_) => {
                (OperationFailureKind::Unavailable, "transport", None, None)
            }
            ClientError::Protocol(_) => (
                OperationFailureKind::ProtocolViolation,
                "response",
                None,
                None,
            ),
            ClientError::Timeout => (OperationFailureKind::Timeout, "response", None, None),
            ClientError::Rejected { code, data } => (
                OperationFailureKind::Rejected,
                "service",
                Some(*code),
                data.clone(),
            ),
        };
        Self {
            kind,
            stage: stage.to_owned(),
            effect: if matches!(error, ClientError::InvalidRequest(_)) {
                OperationEffect::None
            } else {
                effect
            },
            message: error.to_string(),
            code,
            data,
        }
    }
}
