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
    AdapterOperationFailure {
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
}
