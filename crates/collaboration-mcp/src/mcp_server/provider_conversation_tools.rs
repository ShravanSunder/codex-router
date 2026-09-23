use super::*;

macro_rules! provider_conversation_tool {
    ($router:expr, $name:literal, $request:ty, $result:ty, $method:ident, $possible_effect:expr) => {
        $router.add_route(ToolRoute::new_dyn(
            Tool::new(
                $name,
                operation_description($name),
                rmcp::handler::server::tool::schema_for_type::<$request>(),
            )
            .with_raw_output_schema(rmcp::handler::server::tool::schema_for_type::<$result>()),
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
                    let mut client = match tokio::select! {
                        _ = cancellation.cancelled() => {
                            return Ok(CallToolResponse::Complete(provider_call_cancelled(OperationEffect::None)));
                        }
                        result = context.service.connect() => result,
                    } {
                        Ok(value) => value,
                        Err(error) => {
                            return Ok(CallToolResponse::Complete(failure(
                                error,
                                OperationEffect::None,
                            )));
                        }
                    };
                    let result = tokio::select! {
                        _ = cancellation.cancelled() => {
                            let _closed = client.close().await;
                            return Ok(CallToolResponse::Complete(provider_call_cancelled($possible_effect)));
                        }
                        result = client.$method(request) => result,
                    };
                    let _closed = client.close().await;
                    Ok(CallToolResponse::Complete(provider_conversation_result(
                        result,
                        $possible_effect,
                    )))
                })
            },
        ));
    };
}

fn provider_call_cancelled(possible_effect: OperationEffect) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({
        "kind": "callerCancelled",
        "stage": "response",
        "effect": possible_effect,
        "message": "caller cancelled the call-local MCP attachment; provider work was not cancelled"
    }))
}

pub(super) fn register_provider_conversation_tools(
    router: &mut ToolRouter<CollaborationMcpServer>,
) {
    use collaboration_protocol::*;

    provider_conversation_tool!(
        router,
        "provider_conversation_create",
        ConversationCreateRequest,
        ConversationOperationSubmission,
        create_provider_conversation,
        OperationEffect::Unknown
    );
    provider_conversation_tool!(
        router,
        "provider_conversation_load",
        ConversationLoadRequest,
        ConversationOperationSubmission,
        load_provider_conversation,
        OperationEffect::Unknown
    );
    provider_conversation_tool!(
        router,
        "provider_conversation_prompt",
        ConversationPromptRequest,
        ConversationOperationSubmission,
        prompt_provider_conversation,
        OperationEffect::Unknown
    );
    provider_conversation_tool!(
        router,
        "provider_conversation_cancel",
        ConversationCancelRequest,
        ConversationOperationSubmission,
        cancel_provider_conversation_operation,
        OperationEffect::Unknown
    );
    provider_conversation_tool!(
        router,
        "provider_conversation_operation_show",
        ConversationOperationShowRequest,
        ConversationOperationSnapshot,
        show_provider_conversation_operation,
        OperationEffect::None
    );
    provider_conversation_tool!(
        router,
        "provider_conversation_operation_wait",
        ConversationOperationWaitRequest,
        ConversationOperationWaitResult,
        wait_for_provider_conversation_operation,
        OperationEffect::None
    );
    provider_conversation_tool!(
        router,
        "provider_conversation_operation_reconcile",
        ConversationOperationReconcileRequest,
        ConversationOperationSnapshot,
        reconcile_provider_conversation_operation,
        OperationEffect::None
    );
}

fn provider_conversation_result<TValue: serde::Serialize>(
    result: Result<TValue, ClientError>,
    possible_effect: OperationEffect,
) -> CallToolResult {
    match result {
        Ok(value) => structured_result(Ok(value), OperationEffect::None),
        Err(ClientError::Rejected {
            data: Some(data), ..
        }) => match serde_json::from_value::<collaboration_protocol::ConversationOperationFailure>(
            data,
        ) {
            Ok(failure) => serde_json::to_value(failure)
                .map(CallToolResult::structured_error)
                .unwrap_or_else(|_| {
                    validation_failure("provider conversation failure encoding failed")
                }),
            Err(_) => failure(
                ClientError::Protocol("invalid provider conversation failure response"),
                possible_effect,
            ),
        },
        Err(error) => failure(error, possible_effect),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_effect_reflects_submission_boundary() {
        let before_submission = provider_call_cancelled(OperationEffect::None)
            .structured_content
            .expect("pre-submission cancellation");
        assert_eq!(before_submission["kind"], "callerCancelled");
        assert_eq!(before_submission["effect"], "none");

        let after_submission = provider_call_cancelled(OperationEffect::Unknown)
            .structured_content
            .expect("post-submission cancellation");
        assert_eq!(after_submission["kind"], "callerCancelled");
        assert_eq!(after_submission["effect"], "unknown");
    }
}
