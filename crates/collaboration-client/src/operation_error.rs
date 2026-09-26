//! Stable programmatic classification for SDK operation failures.

use crate::ClientError;
use collaboration_protocol::{AdapterOperationFailure, OperationEffect, OperationFailureKind};
use collaboration_protocol::{NonEmptyText, SessionRef};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub struct OperationError {
    stage: &'static str,
    effect: OperationEffect,
    target: Option<Box<SessionRef>>,
    turn_id: Option<NonEmptyText>,
    #[source]
    source: Box<ClientError>,
}

impl OperationError {
    #[must_use]
    pub fn before_dispatch(
        stage: &'static str,
        target: Option<SessionRef>,
        source: ClientError,
    ) -> Self {
        Self {
            stage,
            effect: OperationEffect::None,
            target: target.map(Box::new),
            turn_id: None,
            source: Box::new(source),
        }
    }

    #[must_use]
    pub fn after_dispatch(
        stage: &'static str,
        target: Option<SessionRef>,
        turn_id: Option<NonEmptyText>,
        source: ClientError,
    ) -> Self {
        Self {
            stage,
            effect: OperationEffect::Unknown,
            target: target.map(Box::new),
            turn_id,
            source: Box::new(source),
        }
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        AdapterOperationFailure,
        Option<SessionRef>,
        Option<NonEmptyText>,
    ) {
        let mut failure = operation_failure_from_client_error(*self.source, self.effect);
        failure.stage = self.stage.to_owned();
        (failure, self.target.map(|target| *target), self.turn_id)
    }

    #[must_use]
    pub fn into_source(self) -> ClientError {
        *self.source
    }

    #[must_use]
    pub fn source(&self) -> &ClientError {
        &self.source
    }

    /// Adds caller-known recovery identity without changing the recorded stage/effect.
    #[must_use]
    pub fn with_known_target(mut self, target: SessionRef) -> Self {
        if self.target.is_none() {
            self.target = Some(Box::new(target));
        }
        self
    }
}
#[must_use]
pub fn operation_failure_from_client_error(
    error: ClientError,
    effect: OperationEffect,
) -> AdapterOperationFailure {
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
    let possible_effect = match &error {
        ClientError::Rejected { data, .. }
            if native_control_rejection_has_no_effect(data.as_ref()) =>
        {
            OperationEffect::None
        }
        _ => effect,
    };
    let message = match &error {
        ClientError::Rejected { data, .. } => data
            .as_ref()
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .filter(|message| !message.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| error.to_string()),
        _ => error.to_string(),
    };
    AdapterOperationFailure {
        kind,
        service_kind,
        stage: stage.to_owned(),
        effect: if matches!(error, ClientError::InvalidRequest(_)) {
            OperationEffect::None
        } else {
            possible_effect
        },
        message,
        code,
        data,
    }
}

fn native_control_rejection_has_no_effect(data: Option<&Value>) -> bool {
    let Some(data) = data else {
        return false;
    };
    let stage = data.get("stage").and_then(Value::as_str);
    if !matches!(stage, Some("inspect" | "rename" | "interrupt")) {
        return false;
    }
    matches!(
        data.get("kind").and_then(Value::as_str),
        Some(
            "unsupportedCapability"
                | "nativeRejected"
                | "wrongService"
                | "endpointNotFound"
                | "unavailable"
                | "staleGeneration"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::operation_failure_from_client_error;
    use crate::ClientError;
    use collaboration_protocol::{OperationEffect, OperationFailureKind};
    use serde_json::json;

    #[test]
    fn preserves_service_kind_stage_code_and_data() {
        let failure = operation_failure_from_client_error(
            ClientError::Rejected {
                code: -32050,
                data: Some(json!({"kind":"staleGeneration","stage":"discovery","expected":2})),
            },
            OperationEffect::None,
        );
        assert_eq!(failure.kind, OperationFailureKind::Rejected);
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
        let failure = operation_failure_from_client_error(
            ClientError::Discovery {
                stage: "manifest-read",
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "private path"),
            },
            OperationEffect::None,
        );
        assert_eq!(failure.kind, OperationFailureKind::Unavailable);
        assert_eq!(failure.stage, "manifest-read");
        assert_eq!(
            failure.data,
            Some(json!({"ioKind":"NotFound","osCode":null}))
        );
        assert!(!failure.message.contains("private path"));
    }

    #[test]
    fn native_rejections_keep_the_server_message_and_do_not_claim_an_unknown_effect() {
        let failure = operation_failure_from_client_error(
            ClientError::Rejected {
                code: -32050,
                data: Some(json!({
                    "kind":"unsupportedCapability",
                    "stage":"rename",
                    "message":"Codex app-server method `thread/name/set` is missing"
                })),
            },
            OperationEffect::Unknown,
        );

        assert_eq!(
            failure.message,
            "Codex app-server method `thread/name/set` is missing"
        );
        assert_eq!(failure.effect, OperationEffect::None);
    }

    #[test]
    fn outcome_unknown_rejections_keep_the_server_message_and_unknown_effect() {
        let failure = operation_failure_from_client_error(
            ClientError::Rejected {
                code: -32050,
                data: Some(json!({
                    "kind":"outcomeUnknown",
                    "stage":"rename",
                    "message":"native request outcome is unknown after dispatch"
                })),
            },
            OperationEffect::Unknown,
        );

        assert_eq!(
            failure.message,
            "native request outcome is unknown after dispatch"
        );
        assert_eq!(failure.effect, OperationEffect::Unknown);
    }

    #[test]
    fn conversation_busy_rejections_keep_the_possible_effect_classification() {
        let failure = operation_failure_from_client_error(
            ClientError::Rejected {
                code: -32050,
                data: Some(json!({"kind":"busy","stage":"load"})),
            },
            OperationEffect::Unknown,
        );

        assert_eq!(failure.effect, OperationEffect::Unknown);
    }
}
