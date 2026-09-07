//! Minimal lifecycle projection from a schema-admitted native notification stream.
use crate::JournalError;
use communication_protocol::{
    LifecycleChange, LifecycleObservation, LifecycleSubject, ObservationOrdering, ObservationScope,
    ObservationSource, ObservationTimestamp, SessionId, TerminalStatus, ThreadAddress,
};
use serde_json::Value;

/// Ordering is supplied by reconciliation; receipt alone does not establish freshness.
pub fn map_native_lifecycle(
    message: &Value,
    scope: &ObservationScope,
    at: ObservationTimestamp,
    ordering: ObservationOrdering,
) -> Result<Vec<LifecycleObservation>, JournalError> {
    if message.get("id").is_some() {
        return Ok(Vec::new());
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(Vec::new());
    };
    if !matches!(
        method,
        "thread/started"
            | "thread/status/changed"
            | "thread/archived"
            | "thread/unarchived"
            | "thread/deleted"
            | "thread/closed"
            | "turn/completed"
    ) {
        return Ok(Vec::new());
    }
    let params = message.get("params").ok_or(JournalError::InvalidRecord)?;
    let (thread, changes) = match method {
        "thread/started" => {
            let thread = params.get("thread").ok_or(JournalError::InvalidRecord)?;
            let status = serde_json::from_value(
                thread
                    .get("status")
                    .cloned()
                    .ok_or(JournalError::InvalidRecord)?,
            )
            .map_err(|_| JournalError::InvalidRecord)?;
            (
                thread.get("id"),
                vec![
                    LifecycleChange::ThreadDiscovered,
                    LifecycleChange::ThreadStatus { status, ordering },
                ],
            )
        }
        "thread/status/changed" => {
            let status = serde_json::from_value(
                params
                    .get("status")
                    .cloned()
                    .ok_or(JournalError::InvalidRecord)?,
            )
            .map_err(|_| JournalError::InvalidRecord)?;
            (
                params.get("threadId"),
                vec![LifecycleChange::ThreadStatus { status, ordering }],
            )
        }
        "turn/completed" => {
            let turn = params.get("turn").ok_or(JournalError::InvalidRecord)?;
            let turn_id = parse_id(turn.get("id"))?;
            let status = match turn.get("status").and_then(Value::as_str) {
                Some("completed") => TerminalStatus::Completed,
                Some("interrupted") => TerminalStatus::Interrupted,
                Some("failed") => TerminalStatus::Failed,
                _ => return Err(JournalError::InvalidRecord),
            };
            (
                params.get("threadId"),
                vec![LifecycleChange::TurnTerminal { turn_id, status }],
            )
        }
        _ => {
            let change = match method {
                "thread/archived" => LifecycleChange::ThreadArchived,
                "thread/unarchived" => LifecycleChange::ThreadUnarchived,
                "thread/deleted" => LifecycleChange::ThreadDeleted,
                "thread/closed" => LifecycleChange::ThreadClosed,
                _ => return Err(JournalError::InvalidRecord),
            };
            (params.get("threadId"), vec![change])
        }
    };
    let thread_id = parse_id(thread)?;
    changes
        .into_iter()
        .map(|change| {
            let observation = LifecycleObservation {
                observed_at: at.clone(),
                source: ObservationSource::NativeNotification,
                scope: scope.clone(),
                subject: LifecycleSubject::Thread {
                    address: ThreadAddress {
                        endpoint: scope.endpoint.clone(),
                        native_thread_id: thread_id.clone(),
                    },
                },
                change,
            };
            observation
                .validate()
                .map_err(|_| JournalError::InvalidRecord)?;
            Ok(observation)
        })
        .collect()
}
fn parse_id(value: Option<&Value>) -> Result<SessionId, JournalError> {
    SessionId::try_from(
        value
            .and_then(Value::as_str)
            .ok_or(JournalError::InvalidRecord)?
            .to_owned(),
    )
    .map_err(|_| JournalError::InvalidRecord)
}
