use super::*;

pub(super) fn structured_result<TValue: serde::Serialize>(
    result: Result<TValue, ClientError>,
    possible_effect: OperationEffect,
) -> CallToolResult {
    match result {
        Ok(value) => serde_json::to_value(McpToolOutput::Success(value))
            .map(CallToolResult::structured)
            .unwrap_or_else(|_| validation_failure("collaboration result encoding failed")),
        Err(error) => failure(error, possible_effect),
    }
}

pub(super) fn provider_settings_tool_result(
    result: Result<ProviderSettingsResult, ClientError>,
    possible_effect: OperationEffect,
) -> CallToolResult {
    match result {
        Ok(value) => structured_result(Ok(value), OperationEffect::None),
        Err(error @ ClientError::Rejected { .. }) => {
            let typed = match &error {
                ClientError::Rejected {
                    data: Some(data), ..
                } => serde_json::from_value::<ProviderSettingsFailure>(data.clone()).ok(),
                _ => None,
            };
            typed
                .and_then(|failure| serde_json::to_value(failure).ok())
                .map(structured_tool_error)
                .unwrap_or_else(|| failure(error, possible_effect))
        }
        Err(error) => failure(error, possible_effect),
    }
}

pub(super) fn structured_tool_error(value: serde_json::Value) -> CallToolResult {
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

pub(super) fn message_tool_result(
    result: Result<PushMessageSendResult, MessageSendError>,
) -> CallToolResult {
    match result {
        Ok(push) => {
            if push.delivery_state == PushDeliveryState::Held {
                return structured_result(Ok(push), OperationEffect::None);
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
                _ => return structured_result(Ok(push), OperationEffect::None),
            };
            serde_json::to_value(push)
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
        Err(error) => {
            let (failure, target) = error.into_operation_failure_and_target();
            serde_json::to_value(failure)
                .map(|mut value| {
                    if let Some(fields) = value.as_object_mut() {
                        fields.insert("target".to_owned(), serde_json::json!(target));
                    }
                    structured_tool_error(value)
                })
                .unwrap_or_else(|_| validation_failure("collaboration error encoding failed"))
        }
    }
}

pub(super) fn message_reply_tool_result(
    result: Result<SessionMessageReplyResult, MessageReplyError>,
) -> CallToolResult {
    match result {
        Ok(reply) => {
            if reply.delivery_state == PushDeliveryState::Held {
                return structured_result(Ok(reply), OperationEffect::None);
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
                _ => return structured_result(Ok(reply), OperationEffect::None),
            };
            serde_json::to_value(reply)
                .map(|mut value| {
                    if let Some(fields) = value.as_object_mut() {
                        fields.insert("kind".to_owned(), serde_json::json!(kind));
                        fields.insert("message".to_owned(), serde_json::json!(message));
                        fields.insert("effect".to_owned(), serde_json::json!(effect));
                    }
                    structured_tool_error(value)
                })
                .unwrap_or_else(|_| validation_failure("reply result encoding failed"))
        }
        Err(error) => {
            let (failure, caller) = error.into_operation_failure_and_caller();
            serde_json::to_value(failure)
                .map(|mut value| {
                    if let Some(fields) = value.as_object_mut() {
                        fields.insert("caller".to_owned(), serde_json::json!(caller));
                    }
                    structured_tool_error(value)
                })
                .unwrap_or_else(|_| validation_failure("collaboration error encoding failed"))
        }
    }
}

pub(super) fn failure(error: ClientError, possible_effect: OperationEffect) -> CallToolResult {
    let failure = operation_failure_from_client_error(error, possible_effect);
    serde_json::to_value(failure)
        .map(structured_tool_error)
        .unwrap_or_else(|_| validation_failure("collaboration error encoding failed"))
}

pub(super) fn board_result<TValue: serde::Serialize>(
    result: Result<TValue, collaboration_client::BoardClientError>,
    mutation: bool,
) -> CallToolResult {
    match result {
        Ok(value) => structured_result(Ok(value), OperationEffect::None),
        Err(collaboration_client::BoardClientError::Rejected(error)) => serde_json::to_value(error)
            .map(structured_tool_error)
            .unwrap_or_else(|_| validation_failure("board rejection encoding failed")),
        Err(collaboration_client::BoardClientError::WaitOutcomeUnknown { actor, filter }) => {
            structured_tool_error(serde_json::json!({
                "kind": "outcomeUnknown",
                "stage": "response",
                "effect": "unknown",
                "message": "The wait result was lost; activity may have been handed off. Inspect the Reader's subscriptions and unread inbox (run board thread subscriptions, then board inbox fetch) before waiting again.",
                "actor": actor,
                "filter": filter,
            }))
        }
        Err(collaboration_client::BoardClientError::OutcomeUnknown {
            resource,
            message,
            next_action,
        }) => structured_tool_error(serde_json::json!({
            "kind": "outcomeUnknown",
            "stage": "response",
            "effect": "unknown",
            "message": message,
            "resource": resource,
            "nextAction": next_action,
        })),
        Err(collaboration_client::BoardClientError::Connection(error)) => failure(
            error,
            if mutation {
                OperationEffect::Unknown
            } else {
                OperationEffect::None
            },
        ),
    }
}

pub(super) fn automation_inspection_result<TValue: serde::Serialize>(
    result: Result<TValue, collaboration_client::AutomationInspectionClientError>,
) -> CallToolResult {
    match result {
        Ok(value) => structured_result(Ok(value), OperationEffect::None),
        Err(collaboration_client::AutomationInspectionClientError::Rejected(error)) => {
            serde_json::to_value(error)
                .map(structured_tool_error)
                .unwrap_or_else(|_| validation_failure("automation rejection encoding failed"))
        }
        Err(collaboration_client::AutomationInspectionClientError::Connection(error)) => {
            failure(error, OperationEffect::None)
        }
    }
}

macro_rules! domain_error_converter {
    ($function:ident, $error:ty, $rejected:path, $connection:path) => {
        pub(super) fn $function<TValue: serde::Serialize>(
            result: Result<TValue, $error>,
            mutation: bool,
        ) -> CallToolResult {
            match result {
                Ok(value) => structured_result(Ok(value), OperationEffect::None),
                Err($rejected(error)) => serde_json::to_value(error)
                    .map(structured_tool_error)
                    .unwrap_or_else(|_| validation_failure("domain rejection encoding failed")),
                Err($connection(error)) => failure(
                    error,
                    if mutation {
                        OperationEffect::Unknown
                    } else {
                        OperationEffect::None
                    },
                ),
            }
        }
    };
}

domain_error_converter!(
    instruction_result,
    collaboration_client::InstructionClientError,
    collaboration_client::InstructionClientError::Rejected,
    collaboration_client::InstructionClientError::Connection
);
domain_error_converter!(
    wake_result,
    collaboration_client::WakeClientError,
    collaboration_client::WakeClientError::Rejected,
    collaboration_client::WakeClientError::Connection
);
domain_error_converter!(
    schedule_result,
    collaboration_client::ScheduleClientError,
    collaboration_client::ScheduleClientError::Rejected,
    collaboration_client::ScheduleClientError::Connection
);
domain_error_converter!(
    run_result,
    collaboration_client::RunClientError,
    collaboration_client::RunClientError::Rejected,
    collaboration_client::RunClientError::Connection
);
domain_error_converter!(
    configuration_result,
    collaboration_client::ConfigurationClientError,
    collaboration_client::ConfigurationClientError::Rejected,
    collaboration_client::ConfigurationClientError::Connection
);

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
