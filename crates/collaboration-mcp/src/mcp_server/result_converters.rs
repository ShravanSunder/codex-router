//! Tool results shared by every tool: delivery receipts, operation failures and the client
//! errors the carrier tools still meet.
use super::*;

/// A carrier tool's typed result, or its client error as an operation failure.
pub(super) fn structured_result<TValue: serde::Serialize>(
    result: Result<TValue, ClientError>,
    possible_effect: OperationEffect,
) -> CallToolResult {
    match result {
        Ok(value) => success_result(&value),
        Err(error) => failure(error, possible_effect),
    }
}

pub(crate) fn structured_tool_error(value: serde_json::Value) -> CallToolResult {
    let error =
        McpToolOutput::<serde_json::Value>::Error(Box::new(McpToolError::from_existing(value)));
    match serde_json::to_value(error) {
        Ok(structured) => CallToolResult::structured_error(structured),
        Err(_) => CallToolResult::error(vec![rmcp::model::ContentBlock::text(
            "MCP tool error encoding failed",
        )]),
    }
}

pub(super) fn operation_error_result(
    failure: collaboration_protocol::AdapterOperationFailure,
    target: Option<collaboration_protocol::SessionRef>,
    turn_id: Option<collaboration_protocol::NonEmptyText>,
) -> CallToolResult {
    serde_json::to_value(failure)
        .map(|mut value| {
            if let Some(fields) = value.as_object_mut() {
                fields.insert("target".to_owned(), serde_json::json!(target));
                fields.insert("turnId".to_owned(), serde_json::json!(turn_id));
            }
            structured_tool_error(value)
        })
        .unwrap_or_else(|_| validation_failure("collaboration error encoding failed"))
}

/// An operation failure that also names the session it concerned, as `field`.
pub(super) fn failure_naming(
    failure: &collaboration_protocol::AdapterOperationFailure,
    field: &str,
    session: Option<collaboration_protocol::SessionRef>,
) -> CallToolResult {
    serde_json::to_value(failure)
        .map(|mut value| {
            if let Some(fields) = value.as_object_mut() {
                fields.insert(field.to_owned(), serde_json::json!(session));
            }
            structured_tool_error(value)
        })
        .unwrap_or_else(|_| validation_failure("collaboration error encoding failed"))
}

/// A stored push: success unless its delivery was refused, not submitted or is unknown, which
/// a model must see as an error that still carries the push.
pub(super) fn message_receipt_result(push: PushMessageSendResult) -> CallToolResult {
    if push.delivery_state == PushDeliveryState::Held {
        return success_result(&push);
    }
    let (kind, message, effect) = match &push.receipt.outcome {
        DeliveryOutcome::NotSubmitted { reason, .. } => {
            ("notSubmitted", reason.clone(), OperationEffect::None)
        }
        DeliveryOutcome::Rejected(rejection) => (
            "rejected",
            rejection
                .detail
                .clone()
                .unwrap_or_else(|| "Delivery was rejected".to_owned()),
            OperationEffect::None,
        ),
        DeliveryOutcome::Unknown => (
            "outcomeUnknown",
            "Delivery acceptance is unknown".to_owned(),
            OperationEffect::Unknown,
        ),
        _ => return success_result(&push),
    };
    receipt_error(&push, kind, &message, effect)
}

/// A stored reply, judged by its delivery like a sent message.
pub(super) fn message_reply_receipt_result(reply: SessionMessageReplyResult) -> CallToolResult {
    if reply.delivery_state == PushDeliveryState::Held {
        return success_result(&reply);
    }
    let (kind, message, effect) = match &reply.receipt.outcome {
        DeliveryOutcome::NotSubmitted { reason, .. } => {
            ("notSubmitted", reason.clone(), OperationEffect::None)
        }
        DeliveryOutcome::Rejected(rejection) => (
            "rejected",
            rejection
                .detail
                .clone()
                .unwrap_or_else(|| "Reply delivery was rejected".to_owned()),
            OperationEffect::None,
        ),
        DeliveryOutcome::Unknown => (
            "outcomeUnknown",
            "Reply delivery acceptance is unknown".to_owned(),
            OperationEffect::Unknown,
        ),
        _ => return success_result(&reply),
    };
    receipt_error(&reply, kind, &message, effect)
}

fn receipt_error<TReceipt: serde::Serialize>(
    receipt: &TReceipt,
    kind: &str,
    message: &str,
    effect: OperationEffect,
) -> CallToolResult {
    serde_json::to_value(receipt)
        .map(|mut value| {
            if let Some(fields) = value.as_object_mut() {
                fields.insert("kind".to_owned(), serde_json::json!(kind));
                fields.insert("message".to_owned(), serde_json::json!(message));
                fields.insert("effect".to_owned(), serde_json::json!(effect));
            }
            structured_tool_error(value)
        })
        .unwrap_or_else(|_| validation_failure("push result encoding failed"))
}

/// A subscription wait whose caller went away: a batch may already have been handed to it.
pub(super) fn thread_wait_outcome_unknown(
    actor: &message_board::Identity,
    filter: &collaboration_protocol::ThreadSubscriptionWaitFilter,
) -> CallToolResult {
    structured_tool_error(serde_json::json!({
        "kind": "outcomeUnknown",
        "stage": "response",
        "effect": "unknown",
        "message": "The wait result was lost; activity may have been handed off. Inspect the Reader's subscriptions and unread inbox (run board thread subscriptions, then board inbox fetch) before waiting again.",
        "actor": actor,
        "filter": filter,
    }))
}

pub(super) fn failure(error: ClientError, possible_effect: OperationEffect) -> CallToolResult {
    operation_failure_result(&operation_failure_from_client_error(error, possible_effect))
}

pub(super) fn wake_wait_failure(error: collaboration_client::WakeWaitError) -> CallToolResult {
    serde_json::to_value(error.into_operation_failure())
        .map(structured_tool_error)
        .unwrap_or_else(|_| validation_failure("wake wait error encoding failed"))
}

pub(super) fn validation_failure(message: &str) -> CallToolResult {
    structured_tool_error(serde_json::json!({
        "kind": "protocolViolation",
        "stage": "validation",
        "effect": "none",
        "message": message,
        "code": null,
        "data": null
    }))
}
