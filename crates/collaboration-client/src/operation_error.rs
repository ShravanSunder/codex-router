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
    /// Optional backend classification retained losslessly beside the stable
    /// SDK classification.
    pub service_kind: Option<String>,
    pub stage: String,
    pub effect: OperationEffect,
    pub message: String,
    pub code: Option<i64>,
    pub data: Option<Value>,
}

impl OperationFailure {
    #[must_use]
    pub fn from_client_error(error: ClientError, effect: OperationEffect) -> Self {
        let (kind, service_kind, stage, code, data) = match &error {
            ClientError::InvalidRequest(_) => (
                OperationFailureKind::ProtocolViolation,
                None,
                "validation",
                None,
                None,
            ),
            ClientError::UnsupportedCapability(_) => (
                OperationFailureKind::UnsupportedCapability,
                None,
                "validation",
                None,
                None,
            ),
            ClientError::Discovery { stage, source } => (
                OperationFailureKind::Unavailable,
                None,
                *stage,
                None,
                Some(serde_json::json!({
                    "ioKind": format!("{:?}", source.kind()),
                    "osCode": source.raw_os_error(),
                })),
            ),
            ClientError::Transport(source) => (
                OperationFailureKind::Unavailable,
                None,
                "transport",
                None,
                Some(serde_json::json!({
                    "ioKind": format!("{:?}", source.kind()),
                    "osCode": source.raw_os_error(),
                })),
            ),
            ClientError::Protocol(_) => (
                OperationFailureKind::ProtocolViolation,
                None,
                "response",
                None,
                None,
            ),
            ClientError::Timeout => (OperationFailureKind::Timeout, None, "response", None, None),
            ClientError::Rejected { code, data } => (
                OperationFailureKind::Rejected,
                data.as_ref()
                    .and_then(|value| value.get("kind"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                data.as_ref()
                    .and_then(|value| value.get("stage"))
                    .and_then(Value::as_str)
                    .unwrap_or("service"),
                Some(*code),
                data.clone(),
            ),
        };
        Self {
            kind,
            service_kind,
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

#[cfg(test)]
mod tests {
    use super::{OperationEffect, OperationFailure};
    use crate::ClientError;
    use serde_json::json;

    #[test]
    fn preserves_service_kind_stage_code_and_data() {
        let failure = OperationFailure::from_client_error(
            ClientError::Rejected {
                code: -32050,
                data: Some(json!({"kind":"staleGeneration","stage":"discovery","expected":2})),
            },
            OperationEffect::None,
        );
        assert_eq!(failure.kind, super::OperationFailureKind::Rejected);
        assert_eq!(failure.service_kind.as_deref(), Some("staleGeneration"));
        assert_eq!(failure.stage, "discovery");
        assert_eq!(failure.code, Some(-32050));
        assert_eq!(
            failure
                .data
                .as_ref()
                .and_then(|value| value.get("expected")),
            Some(&json!(2))
        );
    }

    #[test]
    fn discovery_failure_keeps_structured_io_category() {
        let failure = OperationFailure::from_client_error(
            ClientError::Discovery {
                stage: "manifest-read",
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "private path"),
            },
            OperationEffect::None,
        );
        assert_eq!(failure.kind, super::OperationFailureKind::Unavailable);
        assert_eq!(failure.stage, "manifest-read");
        assert_eq!(
            failure.data,
            Some(json!({"ioKind":"NotFound","osCode":null}))
        );
        assert!(!failure.message.contains("private path"));
    }
}
