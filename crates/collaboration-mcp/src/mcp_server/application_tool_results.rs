//! How the collaboration application's typed results reach a model as tool results.
//!
//! A success is the typed result. A failure is the family's typed rejection, presented the way
//! models have always read that rejection: the domain families (board, wakes, schedules, runs,
//! instructions, automation configuration and inspection) hand back their typed failure, and
//! every other operation an operation failure that carries the tool's possible effect. Only a
//! rejection without a message of its own now reads the application's message.
use super::*;
use collaboration_protocol::AdapterOperationFailure;
use collaboration_service::collaboration_application::{
    CollaborationRejection, PublishedRejection,
};

/// The rejection as an operation failure, with the effect the tool declares unless the
/// rejection itself shows nothing took effect.
pub(super) fn rejection_failure(
    rejection: &impl CollaborationRejection,
    possible_effect: OperationEffect,
) -> AdapterOperationFailure {
    published_failure(rejection.published_rejection(), possible_effect)
}

fn published_failure(
    published: PublishedRejection,
    possible_effect: OperationEffect,
) -> AdapterOperationFailure {
    let carries_message = published
        .data
        .as_ref()
        .and_then(|data| data.get("message"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|message| !message.is_empty());
    let mut failure = operation_failure_from_client_error(
        ClientError::Rejected {
            code: published.code,
            data: published.data,
        },
        possible_effect,
    );
    if !carries_message {
        failure.message = published.message;
    }
    failure
}

/// A typed result, or the rejection as an operation failure.
pub(super) fn application_result<TValue, TFailure>(
    result: Result<TValue, TFailure>,
    possible_effect: OperationEffect,
) -> CallToolResult
where
    TValue: serde::Serialize,
    TFailure: CollaborationRejection,
{
    match result {
        Ok(value) => success_result(&value),
        Err(rejection) => operation_failure_result(&rejection_failure(&rejection, possible_effect)),
    }
}

/// A domain family's typed result or typed failure. A rejection the family does not type is
/// an operation failure whose effect is unknown for a mutation.
pub(super) fn domain_result<TValue, TFailure>(
    result: Result<TValue, TFailure>,
    mutation: bool,
) -> CallToolResult
where
    TValue: serde::Serialize,
    TFailure: CollaborationRejection,
{
    match result {
        Ok(value) => success_result(&value),
        Err(rejection) => {
            let published = rejection.published_rejection();
            match published.data {
                Some(data) if published.code == PublishedRejection::OPERATION_FAILED => {
                    structured_tool_error(data)
                }
                _ => operation_failure_result(&published_failure(
                    published,
                    if mutation {
                        OperationEffect::Unknown
                    } else {
                        OperationEffect::None
                    },
                )),
            }
        }
    }
}

/// A typed result, the rejection as `TTypedFailure` when it is one, or else an operation
/// failure.
pub(super) fn typed_failure_result<TTypedFailure, TValue, TFailure>(
    result: Result<TValue, TFailure>,
    possible_effect: OperationEffect,
) -> CallToolResult
where
    TTypedFailure: serde::de::DeserializeOwned + serde::Serialize,
    TValue: serde::Serialize,
    TFailure: CollaborationRejection,
{
    match result {
        Ok(value) => success_result(&value),
        Err(rejection) => {
            let published = rejection.published_rejection();
            let typed = published
                .data
                .clone()
                .and_then(|data| serde_json::from_value::<TTypedFailure>(data).ok())
                .and_then(|typed| serde_json::to_value(typed).ok());
            match typed {
                Some(typed) => structured_tool_error(typed),
                None => operation_failure_result(&published_failure(published, possible_effect)),
            }
        }
    }
}

pub(super) fn success_result<TValue: serde::Serialize>(value: &TValue) -> CallToolResult {
    serde_json::to_value(McpToolOutput::Success(value))
        .map(CallToolResult::structured)
        .unwrap_or_else(|_| validation_failure("collaboration result encoding failed"))
}

pub(super) fn operation_failure_result(failure: &AdapterOperationFailure) -> CallToolResult {
    serde_json::to_value(failure)
        .map(structured_tool_error)
        .unwrap_or_else(|_| validation_failure("collaboration error encoding failed"))
}

/// The tool error for a call the Router abandoned because it was still running when the
/// listener's shutdown grace period ended. It may have taken effect.
pub(super) fn call_abandoned_at_shutdown() -> CallToolResult {
    structured_tool_error(serde_json::json!({
        "kind": "unavailable",
        "stage": "shutdown",
        "effect": "unknown",
        "message": "The Router stopped before this call finished and abandoned it; inspect the affected resource before retrying."
    }))
}

/// The tool error for a call whose caller went away before it finished. Nothing reads it; it
/// exists so a cancelled handler returns promptly and releases what it holds.
pub(super) fn caller_cancelled(possible_effect: OperationEffect) -> CallToolResult {
    structured_tool_error(serde_json::json!({
        "kind": "callerCancelled",
        "stage": "response",
        "effect": possible_effect,
        "message": "The caller cancelled this call; no work was replayed."
    }))
}
