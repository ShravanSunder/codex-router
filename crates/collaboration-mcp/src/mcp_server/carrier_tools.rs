//! Conversation tools that open a carrier session: create, load, prompt, cancel,
//! create-and-prompt and bounded observation.
//!
//! They run the same carrier clients the CLIs do, inside the Host: a Codex conversation or
//! observation runs on its carrier socket; a provider conversation or observation runs on the
//! Router's own operations.
use super::*;

#[tool_router(router = carrier_tool_router, vis = "pub(super)")]
impl CollaborationMcpServer {
    #[tool(name = "conversation_create", description = "Creates one conversation through its advertised client. Requires a caller UUIDv7 operationId and exact endpoint, working directory, access, and creator; approver defaults to creator. model and effort are required for Codex endpoints; provider endpoints accept advertised mode, model and effort values. fork and rootMessageId are Codex-only and rejected for provider endpoints. Returns created with target or pending with the inspectable operation ID.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ConversationCreateOutcome>>())]
    pub(super) async fn conversation_create(
        &self,
        Parameters(mut request): Parameters<ConversationCreateToolRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        request.create.default_approver();
        let operation_id = request.create.operation_id.clone();
        let endpoint = request.create.endpoint.clone();
        let timeout_seconds = request.timeout_seconds.map_or(300, u32::from);
        if let Err(error) = ConversationClient::validate_create_input(
            &request.create,
            Duration::from_secs(u64::from(timeout_seconds)),
        ) {
            return conversation_create_tool_result(Err(error), operation_id);
        }
        let connected = tokio::select! {
            _ = context.ct.cancelled() => {
                return conversation_call_cancelled(OperationEffect::None, Some(&operation_id));
            }
            connected = ConversationClient::connect(&self.carrier_access, &endpoint) => connected,
        };
        let result = match connected {
            Ok(client) => {
                tokio::select! {
                    _ = context.ct.cancelled() => {
                        return conversation_call_cancelled(OperationEffect::Unknown, Some(&operation_id));
                    }
                    result = client
                    .create(
                        request.create,
                        Duration::from_secs(u64::from(timeout_seconds)),
                    )
                    => result,
                }
            }
            Err(ConversationClientError::Client(error)) => {
                return failure(error, OperationEffect::None);
            }
            Err(error) => Err(error),
        };
        conversation_create_tool_result(result, operation_id)
    }

    #[tool(name = "conversation_load", description = "Loads one conversation through its advertised client using an exact target, working directory, access and requester. External providers require a caller UUIDv7 operation ID; Codex load must omit it because it is not inspectable. Returns a completed settlement or an inspectable provider operation when pending; uncertain work is never replayed.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ConversationOperationResult>>())]
    pub(super) async fn conversation_load(
        &self,
        Parameters(request): Parameters<ConversationLoadToolRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        let operation_id = request.load.operation_id.clone();
        let endpoint = request.load.target.endpoint.clone();
        let timeout =
            Duration::from_secs(u64::from(request.timeout_seconds.map_or(300, u32::from)));
        let connected = tokio::select! {
            _ = context.ct.cancelled() => {
                return conversation_call_cancelled(OperationEffect::None, operation_id.as_ref());
            }
            connected = ConversationClient::connect(&self.carrier_access, &endpoint) => connected,
        };
        let result = match connected {
            Ok(client) => {
                if let Err(error) =
                    client.validate_operation_id(&endpoint, operation_id.as_ref(), "load")
                {
                    return conversation_tool_result::<ConversationOperationResult>(
                        Err(error),
                        operation_id,
                    );
                }
                tokio::select! {
                _ = context.ct.cancelled() => {
                    return conversation_call_cancelled(OperationEffect::Unknown, operation_id.as_ref());
                }
                result = client.load(request.load, timeout) => result,
                }
            }
            Err(ConversationClientError::Client(error)) => {
                return failure(error, OperationEffect::None);
            }
            Err(error) => Err(error),
        };
        conversation_tool_result(result, operation_id)
    }

    #[tool(name = "conversation_prompt", description = "Prompts one conversation through its advertised client. External providers require a caller UUIDv7 operation ID; Codex prompt must omit it because it is not inspectable. A wait deadline or caller cancellation detaches from a running Codex turn without interrupting it. The result records a completed turn, a running Codex turn, or a pending provider operation.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ConversationOperationResult>>())]
    pub(super) async fn conversation_prompt(
        &self,
        Parameters(request): Parameters<ConversationPromptToolRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        let operation_id = request.prompt.operation_id.clone();
        let endpoint = request.prompt.target.endpoint.clone();
        let timeout =
            Duration::from_secs(u64::from(request.timeout_seconds.map_or(300, u32::from)));
        let connected = tokio::select! {
            _ = context.ct.cancelled() => {
                return conversation_call_cancelled(OperationEffect::None, operation_id.as_ref());
            }
            connected = ConversationClient::connect(&self.carrier_access, &endpoint) => connected,
        };
        let client = match connected {
            Ok(client) => client,
            Err(ConversationClientError::Client(error)) => {
                return failure(error, OperationEffect::None);
            }
            Err(error) => {
                return conversation_tool_result::<ConversationOperationResult>(
                    Err(error),
                    operation_id,
                );
            }
        };
        if let Err(error) = client.validate_operation_id(&endpoint, operation_id.as_ref(), "prompt")
        {
            return conversation_tool_result::<ConversationOperationResult>(
                Err(error),
                operation_id,
            );
        }
        let result = if matches!(&client, ConversationClient::ExternalProvider(_)) {
            tokio::select! {
                _ = context.ct.cancelled() => {
                    return conversation_call_cancelled(OperationEffect::Unknown, operation_id.as_ref());
                }
                result = client.prompt(request.prompt, timeout, context.ct.clone()) => result,
            }
        } else {
            client.prompt(request.prompt, timeout, context.ct).await
        };
        conversation_tool_result(result, operation_id)
    }

    #[tool(name = "conversation_cancel", description = "Requests cancellation of one exact conversation operation. Requires a new operation ID and the target operation, conversation, and requester; accepted cancellation is distinct from confirmed cessation. Codex ACP reports unsupportedCapability with a turn interrupt fix.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ConversationOperationSubmission>>())]
    pub(super) async fn conversation_cancel(
        &self,
        Parameters(request): Parameters<ConversationCancelInput>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        let operation_id = request.operation_id.clone();
        let endpoint = request.target.endpoint.clone();
        let connected = tokio::select! {
            _ = context.ct.cancelled() => {
                return conversation_call_cancelled(OperationEffect::None, Some(&operation_id));
            }
            connected = ConversationClient::connect(&self.carrier_access, &endpoint) => connected,
        };
        let result = match connected {
            Ok(client) if matches!(&client, ConversationClient::ExternalProvider(_)) => {
                tokio::select! {
                    _ = context.ct.cancelled() => {
                        return conversation_call_cancelled(OperationEffect::Unknown, Some(&operation_id));
                    }
                    result = client.cancel(request) => result,
                }
            }
            Ok(client) => client.cancel(request).await,
            Err(ConversationClientError::Client(error)) => {
                return failure(error, OperationEffect::None);
            }
            Err(error) => Err(error),
        };
        conversation_tool_result(result, Some(operation_id))
    }

    #[tool(name = "conversation_create_and_prompt", description = "Creates a fresh conversation and prompts it through the advertised client. model and effort are required for Codex endpoints; provider endpoints accept advertised mode, model and effort values. fork and rootMessageId are Codex-only and rejected for provider endpoints. The create operation ID is inspectable; provider prompt requires a second caller UUIDv7 ID, while Codex prompt omits it because it is not inspectable. The result names a pending create or the prompt settlement. A completed turn is not an assignment verdict or peer reply; cancellation never silently replays a submission.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ConversationCreatePromptOutcome>>())]
    pub(super) async fn conversation_create_and_prompt(
        &self,
        Parameters(mut request): Parameters<ConversationCreatePromptToolRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        request.input.create.default_approver();
        let operation_id = request.input.create.operation_id.clone();
        let timeout =
            Duration::from_secs(u64::from(request.timeout_seconds.map_or(300, u32::from)));
        let result = ConversationClient::create_and_prompt(
            &self.carrier_access,
            request.input,
            timeout,
            context.ct,
        )
        .await;
        conversation_tool_result(result, Some(operation_id))
    }

    #[tool(name = "events_observe", description = "Explicitly attaches to one conversation and returns bounded call-local events. Each event is also streamed as a notifications/codexRouter/observationEvent notification ({event, cursor}) while the call is open; the result still lists every event. Use afterSequence with the returned epoch to continue loaded Session history; an epoch mismatch requires resync. Concurrent sends have no ordering guarantee relative to attachment.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<BoundedObservationResult>>())]
    pub(super) async fn events_observe(
        &self,
        Parameters(request): Parameters<BoundedObservationRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        let (observed, mut streamed) = tokio::sync::mpsc::unbounded_channel();
        let observation = collaboration_client::SessionObservation::observe_bounded(
            &self.carrier_access,
            request,
            context.ct.clone(),
            Some(observed),
        );
        match stream_observed_events(&context.peer, &mut streamed, observation).await {
            Ok(value) => structured_result(Ok(value), OperationEffect::None),
            Err(error) => {
                let (failure, target, turn_id) = error.into_parts();
                operation_error_result(failure, target, turn_id)
            }
        }
    }
}

/// Runs one observation, sending each event it observes to the caller as an
/// `observationEvent` notification while the call is open, then answers its result. rmcp
/// answers the call as an SSE stream once a notification precedes the result.
async fn stream_observed_events<TResult>(
    peer: &rmcp::service::Peer<rmcp::service::RoleServer>,
    streamed: &mut tokio::sync::mpsc::UnboundedReceiver<
        collaboration_protocol::ObservationEventNotification,
    >,
    observation: impl std::future::Future<Output = TResult>,
) -> TResult {
    let mut observation = std::pin::pin!(observation);
    let result = loop {
        tokio::select! {
            biased;
            Some(event) = streamed.recv() => notify_observed_event(peer, event).await,
            result = &mut observation => break result,
        }
    };
    // The finished observation dropped its sender; send what it observed last.
    while let Ok(event) = streamed.try_recv() {
        notify_observed_event(peer, event).await;
    }
    result
}

async fn notify_observed_event(
    peer: &rmcp::service::Peer<rmcp::service::RoleServer>,
    event: collaboration_protocol::ObservationEventNotification,
) {
    let Ok(params) = serde_json::to_value(&event) else {
        return;
    };
    // A caller that has gone cannot receive it; the result still lists the event.
    let _sent = peer
        .send_notification(rmcp::model::ServerNotification::CustomNotification(
            rmcp::model::CustomNotification::new(
                collaboration_protocol::OBSERVATION_EVENT_NOTIFICATION,
                Some(params),
            ),
        ))
        .await;
}
