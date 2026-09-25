//! Structured results and caller-detach evidence for the shared conversation tools.
use super::*;
use serde::Serialize;

pub(super) fn conversation_call_cancelled(
    effect: OperationEffect,
    operation_id: Option<&OperationId>,
) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({
        "kind":"callerCancelled","stage":"response","effect":effect,
        "operationId":operation_id,
        "message":"caller cancelled the call-local MCP attachment; conversation work was not cancelled"
    }))
}

pub(super) fn conversation_create_tool_result(
    result: Result<ConversationCreateOutcome, ConversationClientError>,
    operation_id: OperationId,
) -> CallToolResult {
    conversation_tool_result(result, Some(operation_id))
}

pub(super) fn conversation_tool_result<TValue: Serialize>(
    result: Result<TValue, ConversationClientError>,
    operation_id: Option<OperationId>,
) -> CallToolResult {
    match result {
        Ok(outcome) => structured_result(Ok(outcome), OperationEffect::None),
        Err(ConversationClientError::Codex(error)) => {
            let (failure, target, turn_id) = error.into_parts();
            operation_error_result(failure, target, turn_id)
        }
        Err(ConversationClientError::CallerCancelled { operation_id }) => {
            conversation_call_cancelled(OperationEffect::Unknown, Some(&operation_id))
        }
        Err(ConversationClientError::AfterCreate {
            create_operation_id,
            target,
            source,
        }) => {
            let mut result =
                conversation_tool_result::<TValue>(Err(*source), Some(create_operation_id.clone()));
            if let Some(serde_json::Value::Object(fields)) = result.structured_content.as_mut() {
                fields.insert(
                    "createOperationId".to_owned(),
                    serde_json::json!(create_operation_id),
                );
                fields.insert("target".to_owned(), serde_json::json!(target));
            }
            result
        }
        Err(ConversationClientError::UnsupportedInput {
            endpoint,
            field,
            fix,
        }) => CallToolResult::structured_error(serde_json::json!({
            "kind":"unsupportedCapability","stage":"validation","effect":"none",
            "operationId":operation_id,"endpoint":endpoint,"field":field,
            "message":format!("{field} is unsupported by {}", String::from(endpoint.endpoint_id)),
            "fix":fix
        })),
        Err(ConversationClientError::OperationFailure(failure)) => serde_json::to_value(failure)
            .map(CallToolResult::structured_error)
            .unwrap_or_else(|_| validation_failure("conversation failure encoding failed")),
        Err(ConversationClientError::Client(ClientError::Rejected {
            data: Some(data), ..
        })) => {
            match serde_json::from_value::<collaboration_protocol::ConversationOperationFailure>(
                data,
            ) {
                Ok(failure) => serde_json::to_value(failure)
                    .map(CallToolResult::structured_error)
                    .unwrap_or_else(|_| {
                        validation_failure("conversation rejection encoding failed")
                    }),
                Err(_) => validation_failure("invalid conversation rejection"),
            }
        }
        Err(ConversationClientError::Client(error)) => failure(error, OperationEffect::Unknown),
        Err(ConversationClientError::InvalidInput(message)) => {
            CallToolResult::structured_error(serde_json::json!({
                "kind":"invalidRequest","stage":"validation","effect":"none",
                "operationId":operation_id,"message":message
            }))
        }
        Err(ConversationClientError::MissingOperationId {
            endpoint,
            operation,
        }) => CallToolResult::structured_error(serde_json::json!({
            "kind":"invalidRequest","stage":"validation","effect":"none",
            "operationId":null,"endpoint":endpoint,"operation":operation,
            "message":format!("provider conversation {operation} requires a caller-supplied canonical lowercase RFC UUIDv7 operationId"),
            "fix":"Generate a UUIDv7 with python3 -c 'import uuid; print(uuid.uuid7())' and pass it as operationId"
        })),
    }
}

#[cfg(test)]
pub(super) fn common_prompt_tool_result(
    result: Result<
        ConversationOperationResult,
        collaboration_client::ExistingConversationPromptError,
    >,
) -> CallToolResult {
    conversation_tool_result(
        result.map_err(ConversationClientError::from),
        Some(OperationId::generate()),
    )
}
