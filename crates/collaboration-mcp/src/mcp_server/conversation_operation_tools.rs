//! Inspectable conversation operations: show, bounded wait and reconcile.
use super::*;

macro_rules! conversation_operation_tool {
    ($router:expr, $name:literal, $request:ty => $result:ty, $operation:ident) => {
        $router.add_route(ToolRoute::new_dyn(
            Tool::new(
                $name,
                operation_description($name),
                rmcp::handler::server::tool::schema_for_type::<$request>(),
            )
            .with_raw_output_schema(rmcp::handler::server::tool::schema_for_type::<
                McpToolOutput<$result>,
            >()),
            |context: ToolCallContext<'_, CollaborationMcpServer>| {
                Box::pin(async move {
                    let cancellation = context.request_context.ct.clone();
                    let request = match serde_json::from_value::<$request>(
                        serde_json::Value::Object(context.arguments.unwrap_or_default()),
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            return Ok(CallToolResponse::Complete(validation_failure(
                                &error.to_string(),
                            )));
                        }
                    };
                    let conversations = context.service.application.conversations();
                    let result = tokio::select! {
                        () = cancellation.cancelled() => {
                            return Ok(CallToolResponse::Complete(operation_call_cancelled()));
                        }
                        result = conversations.$operation(request) => result,
                    };
                    Ok(CallToolResponse::Complete(typed_failure_result::<
                        collaboration_protocol::ConversationOperationFailure,
                        _,
                        _,
                    >(
                        result, OperationEffect::None
                    )))
                })
            },
        ));
    };
}

/// Inspecting an operation never changes it, so a caller that goes away leaves no effect.
fn operation_call_cancelled() -> CallToolResult {
    structured_tool_error(serde_json::json!({
        "kind": "callerCancelled",
        "stage": "response",
        "effect": OperationEffect::None,
        "message": "caller cancelled the call-local MCP attachment; conversation work was not cancelled"
    }))
}

pub(super) fn register_conversation_operation_tools(
    router: &mut ToolRouter<CollaborationMcpServer>,
) {
    use collaboration_protocol::*;
    conversation_operation_tool!(router, "conversation_operation_show", ConversationOperationShowRequest => ConversationOperationSnapshot, operation_show);
    conversation_operation_tool!(router, "conversation_operation_wait", ConversationOperationWaitRequest => ConversationOperationWaitResult, operation_wait);
    conversation_operation_tool!(router, "conversation_operation_reconcile", ConversationOperationReconcileRequest => ConversationOperationSnapshot, operation_reconcile);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_inspection_reports_no_effect() {
        let cancelled = operation_call_cancelled()
            .structured_content
            .expect("cancelled inspection");
        assert_eq!(cancelled["kind"], "callerCancelled");
        assert_eq!(cancelled["effect"], "none");
    }
}
