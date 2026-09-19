use collaboration_client::{
    AcpConversation, BoundedObservationRequest, BoundedObservationResult, ClientError,
    ControlClient, ConversationCreatePromptError, ConversationCreatePromptRequest,
    ConversationCreatePromptResult, ConversationCreateRequest, ConversationCreateResult,
    ExistingConversationPromptRequest, ExistingConversationPromptResult, MessageSendError,
    MessageSendRequest, NativeObservation, OperationEffect, OperationFailure,
};
use collaboration_protocol::{
    AddressListParams, AddressPage, ApprovalDecideParams, ApprovalDecideResult, ApprovalListParams,
    ApprovalListResult, EndpointInventory, JournalPage, JournalReadParams, JournalStatus,
    NativeInspectParams, NativeInspectResult, NativeInterruptParams, NativeInterruptResult,
    NativeRenameParams, NativeRenameResult, NativeSendReceipt, NativeSessionListParams,
    NativeSessionListResult,
};
use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRoute,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    schemars, tool, tool_router,
};
use serde::Deserialize;
use std::{path::PathBuf, time::Duration};

mod catalog_descriptions;
mod catalog_tools;
mod schema_binding;
use catalog_descriptions::operation_description;
use catalog_tools::{
    EmptyToolInput, ThreadWaitToolInput, register_automation_inspection_tools,
    register_automation_mutation_tools, register_board_tools,
};
use schema_binding::{bind_native_schema_refs, load_advertised_native_definitions};

#[derive(Clone, Debug)]
pub(crate) struct CollaborationMcpServer {
    service_directory: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl CollaborationMcpServer {
    pub(crate) fn new(service_directory: PathBuf) -> Self {
        let mut tool_router = Self::tool_router();
        register_board_tools(&mut tool_router);
        register_automation_inspection_tools(&mut tool_router);
        register_automation_mutation_tools(&mut tool_router);
        Self {
            service_directory,
            tool_router,
        }
    }

    async fn connect(&self) -> Result<ControlClient, ClientError> {
        ControlClient::connect(
            &self.service_directory,
            "collaboration-mcp",
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    fn resolved_tools(&self) -> Vec<Tool> {
        let native_definitions = load_advertised_native_definitions(&self.service_directory);
        self.tool_router
            .list_all()
            .into_iter()
            .map(|mut tool| {
                let mut input = serde_json::Value::Object((*tool.input_schema).clone());
                bind_native_schema_refs(&mut input, native_definitions.as_ref());
                if let serde_json::Value::Object(fields) = input {
                    tool.input_schema = std::sync::Arc::new(fields);
                }
                if let Some(schema) = &tool.output_schema {
                    let mut output = serde_json::Value::Object((**schema).clone());
                    bind_native_schema_refs(&mut output, native_definitions.as_ref());
                    if let serde_json::Value::Object(fields) = output {
                        tool.output_schema = Some(std::sync::Arc::new(fields));
                    }
                }
                tool
            })
            .collect()
    }
}

#[tool_router]
impl CollaborationMcpServer {
    #[tool(name = "endpoints_list", description = "Lists collaboration endpoints. Read-only; successful discovery is not evidence that an agent assignment completed.", output_schema = rmcp::handler::server::tool::schema_for_type::<EndpointInventory>())]
    async fn endpoints_list(
        &self,
        Parameters(EmptyToolInput {}): Parameters<EmptyToolInput>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client.list_endpoints().await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::None)
    }

    #[tool(name = "sessions_list", description = "Lists stored, loaded, or active conversations using the selected endpoint and scope. Read-only and never resumes a conversation.", output_schema = rmcp::handler::server::tool::schema_for_type::<NativeSessionListResult>())]
    async fn sessions_list(
        &self,
        Parameters(request): Parameters<NativeSessionListParams>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client.list_sessions(request).await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::None)
    }

    #[tool(name = "session_inspect", description = "Inspects one exact conversation target without changing its identity. Attachment or backend failures retain structured target/effect evidence.", output_schema = rmcp::handler::server::tool::schema_for_type::<NativeInspectResult>())]
    async fn session_inspect(
        &self,
        Parameters(request): Parameters<NativeInspectParams>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client.inspect_session(&request.target).await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::None)
    }

    #[tool(name = "session_rename", description = "Renames one exact conversation. Dispatches once and never automatically replays an uncertain mutation.", output_schema = rmcp::handler::server::tool::schema_for_type::<NativeRenameResult>())]
    async fn session_rename(
        &self,
        Parameters(request): Parameters<NativeRenameParams>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client.rename_session(request).await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::Unknown)
    }

    #[tool(name = "turn_interrupt", description = "Interrupts only the supplied exact turn with its generation guard. A receipt does not assert assignment success or observed cessation.", output_schema = rmcp::handler::server::tool::schema_for_type::<NativeInterruptResult>())]
    async fn turn_interrupt(
        &self,
        Parameters(request): Parameters<NativeInterruptParams>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client
            .interrupt_turn(
                &request.target,
                &request.generation,
                &String::from(request.turn_id.clone()),
            )
            .await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::Unknown)
    }

    #[tool(name = "message_send", description = "Submits one agent-authored or explicit human message with exact auto, queue, or steer semantics and their loaded/active prerequisites. A returned receipt proves native input was accepted or queued as stated; it does not prove turn completion, assignment success or an agent reply. Router-authored content is not a public caller input, and uncertain dispatch is never replayed automatically.", output_schema = rmcp::handler::server::tool::schema_for_type::<NativeSendReceipt>())]
    async fn message_send(
        &self,
        Parameters(request): Parameters<MessageSendRequest>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client.send_message(request).await;
        let _closed = client.close().await;
        message_tool_result(result)
    }

    #[tool(name = "approval_list", description = "Lists approval requests using the existing Router approval policy. Read-only.", output_schema = rmcp::handler::server::tool::schema_for_type::<ApprovalListResult>())]
    async fn approval_list(
        &self,
        Parameters(request): Parameters<ApprovalListParams>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client.list_pending_approvals(request.pending).await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::None)
    }

    #[tool(name = "approval_decide", description = "Records one approval decision through the existing authorization checks. MCP never auto-approves.", output_schema = rmcp::handler::server::tool::schema_for_type::<ApprovalDecideResult>())]
    async fn approval_decide(
        &self,
        Parameters(request): Parameters<ApprovalDecideParams>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client.decide_approval(request).await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::Unknown)
    }

    #[tool(name = "journal_status", description = "Reads lifecycle-journal availability and bounds without mutating state.", output_schema = rmcp::handler::server::tool::schema_for_type::<JournalStatus>())]
    async fn journal_status(
        &self,
        Parameters(EmptyToolInput {}): Parameters<EmptyToolInput>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client.journal_status().await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::None)
    }

    #[tool(name = "journal_read", description = "Reads a bounded lifecycle-journal page. Read-only and not a durable conversation transcript.", output_schema = rmcp::handler::server::tool::schema_for_type::<JournalPage>())]
    async fn journal_read(
        &self,
        Parameters(request): Parameters<JournalReadParams>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client
            .read_journal(
                &request.endpoint,
                request.after,
                request.page_size,
                request.wait_milliseconds,
            )
            .await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::None)
    }

    #[tool(name = "addresses_list", description = "Lists known conversation addresses and their observed lifecycle coverage. Read-only.", output_schema = rmcp::handler::server::tool::schema_for_type::<AddressPage>())]
    async fn addresses_list(
        &self,
        Parameters(request): Parameters<AddressListParams>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client
            .list_addresses(
                &request.endpoint,
                request.page_size,
                request.cursor.as_deref(),
            )
            .await;
        let _closed = client.close().await;
        structured_result(result, OperationEffect::None)
    }

    #[tool(name = "automation_status", description = "Reads automation readiness and configured attempt budgets without mutating them.", output_schema = rmcp::handler::server::tool::schema_for_type::<collaboration_protocol::AutomationStatus>())]
    async fn automation_status(
        &self,
        Parameters(EmptyToolInput {}): Parameters<EmptyToolInput>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client.automation_status().await;
        let _closed = client.close().await;
        configuration_result(result, false)
    }

    #[tool(name = "board_thread_wait", description = "Waits once for activity on an existing board listener. This observes board activity and does not prove agent completion.", output_schema = rmcp::handler::server::tool::schema_for_type::<collaboration_client::board::ThreadWaitResult>())]
    async fn board_thread_wait(
        &self,
        Parameters(input): Parameters<ThreadWaitToolInput>,
    ) -> CallToolResult {
        let mut client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let result = client
            .board_thread_wait(input.request, Duration::from_secs(input.timeout_seconds))
            .await;
        let _closed = client.close().await;
        board_result(result, false)
    }

    #[tool(name = "wake_wait_until_first_fire", description = "Subscribes on one call-local Control connection and waits for the selected wake-up's first fire. Cancellation closes only this wait; it never recreates or replays the wake-up.", output_schema = rmcp::handler::server::tool::schema_for_type::<collaboration_protocol::FireReceipt>())]
    async fn wake_wait_until_first_fire(
        &self,
        Parameters(request): Parameters<collaboration_protocol::WakeShowRequest>,
    ) -> CallToolResult {
        let client = match self.connect().await {
            Ok(value) => value,
            Err(error) => return failure(error, OperationEffect::None),
        };
        let wait = match client.subscribe_wakeup(request).await {
            Ok(value) => value,
            Err(error) => return wake_wait_failure(error),
        };
        match wait.wait_until_first_fire().await {
            Ok(value) => structured_result(Ok(value), OperationEffect::None),
            Err(error) => wake_wait_failure(error),
        }
    }

    #[tool(name = "conversation_create", description = "Creates, loads, or forks one Codex conversation with explicit endpoint, cwd, model, effort, access, creator, and approver. A lost response may leave creation outcome unknown.", output_schema = rmcp::handler::server::tool::schema_for_type::<ConversationCreateResult>())]
    async fn conversation_create(
        &self,
        Parameters(request): Parameters<ConversationCreateRequest>,
    ) -> CallToolResult {
        let mut emit = |_event| Ok(());
        match AcpConversation::create(&self.service_directory, request, &mut emit).await {
            Ok((_conversation, result)) => structured_result(Ok(result), OperationEffect::Unknown),
            Err(error) => failure(error, OperationEffect::Unknown),
        }
    }

    #[tool(name = "conversation_create_and_prompt", description = "Creates one fresh Codex conversation and submits its first Agent- or Human-authored prompt on the same call-local ACP connection, then waits for correlated settlement. Requires explicit endpoint, cwd, model, effort, access, creator and approver inputs. The returned target is the actual conversation ID; a completed turn is not an assignment verdict or peer reply. Application deadlines return structured settlement when deliverable, while MCP cancellation or disconnection may suppress the response and does not guarantee every spawned effect ceased. A post-create failure retains the created target when known and is never replayed.", output_schema = rmcp::handler::server::tool::schema_for_type::<ConversationCreatePromptResult>())]
    async fn conversation_create_and_prompt(
        &self,
        Parameters(request): Parameters<ConversationCreatePromptRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        create_prompt_tool_result(
            AcpConversation::create_and_prompt(&self.service_directory, request, context.ct).await,
        )
    }

    #[tool(name = "conversation_prompt", description = "Loads an existing materialized conversation, renders the explicit current sender, submits one prompt, and waits for correlated settlement. For a newly created empty conversation, use message_send for its first input (or the CLI create-and-prompt convenience) before this history-resuming operation. Backend rejection remains explicit; no seed or fallback is sent. Cancellation requests backend cancellation but does not prove cessation.", output_schema = rmcp::handler::server::tool::schema_for_type::<ExistingConversationPromptResult>())]
    async fn conversation_prompt(
        &self,
        Parameters(request): Parameters<ExistingConversationPromptRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        structured_result(
            AcpConversation::prompt_existing(&self.service_directory, request, context.ct).await,
            OperationEffect::Unknown,
        )
    }

    #[tool(name = "events_observe", description = "Explicitly attaches to one conversation and returns bounded call-local events. It never interrupts work and provides no replay cursor or ordering guarantee with concurrent sends.", output_schema = rmcp::handler::server::tool::schema_for_type::<BoundedObservationResult>())]
    async fn events_observe(
        &self,
        Parameters(request): Parameters<BoundedObservationRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        structured_result(
            NativeObservation::observe_bounded(&self.service_directory, request, context.ct).await,
            OperationEffect::None,
        )
    }
}

fn structured_result<TValue: serde::Serialize>(
    result: Result<TValue, ClientError>,
    possible_effect: OperationEffect,
) -> CallToolResult {
    match result {
        Ok(value) => serde_json::to_value(value)
            .map(CallToolResult::structured)
            .unwrap_or_else(|_| validation_failure("collaboration result encoding failed")),
        Err(error) => failure(error, possible_effect),
    }
}

fn message_tool_result(result: Result<NativeSendReceipt, MessageSendError>) -> CallToolResult {
    match result {
        Ok(receipt) => structured_result(Ok(receipt), OperationEffect::None),
        Err(MessageSendError::Preparation(error)) => failure(error, OperationEffect::None),
        Err(MessageSendError::Submission(error)) => failure(error, OperationEffect::Unknown),
    }
}

fn create_prompt_tool_result(
    result: Result<ConversationCreatePromptResult, ConversationCreatePromptError>,
) -> CallToolResult {
    match result {
        Ok(value) => structured_result(Ok(value), OperationEffect::None),
        Err(error) => {
            let (target, source) = error.into_parts();
            let effect = if target.is_some()
                || !matches!(
                    &source,
                    ClientError::InvalidRequest(_)
                        | ClientError::UnsupportedCapability(_)
                        | ClientError::Discovery { .. }
                ) {
                OperationEffect::Unknown
            } else {
                OperationEffect::None
            };
            let mut failure =
                serde_json::to_value(OperationFailure::from_client_error(source, effect))
                    .unwrap_or_else(|_| {
                        serde_json::json!({
                            "kind":"protocolViolation",
                            "stage":"validation",
                            "effect":"none",
                            "message":"collaboration error encoding failed"
                        })
                    });
            if let Some(fields) = failure.as_object_mut() {
                fields.insert("target".to_owned(), serde_json::json!(target));
            }
            CallToolResult::structured_error(failure)
        }
    }
}

fn failure(error: ClientError, possible_effect: OperationEffect) -> CallToolResult {
    let failure = OperationFailure::from_client_error(error, possible_effect);
    serde_json::to_value(failure)
        .map(CallToolResult::structured_error)
        .unwrap_or_else(|_| validation_failure("collaboration error encoding failed"))
}

fn board_result<TValue: serde::Serialize>(
    result: Result<TValue, collaboration_client::BoardClientError>,
    mutation: bool,
) -> CallToolResult {
    match result {
        Ok(value) => structured_result(Ok(value), OperationEffect::None),
        Err(collaboration_client::BoardClientError::Rejected(error)) => serde_json::to_value(error)
            .map(CallToolResult::structured_error)
            .unwrap_or_else(|_| validation_failure("board rejection encoding failed")),
        Err(collaboration_client::BoardClientError::OutcomeUnknown {
            resource,
            message,
            next_action,
        }) => CallToolResult::structured_error(serde_json::json!({
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

fn automation_inspection_result<TValue: serde::Serialize>(
    result: Result<TValue, collaboration_client::AutomationInspectionClientError>,
) -> CallToolResult {
    match result {
        Ok(value) => structured_result(Ok(value), OperationEffect::None),
        Err(collaboration_client::AutomationInspectionClientError::Rejected(error)) => {
            serde_json::to_value(error)
                .map(CallToolResult::structured_error)
                .unwrap_or_else(|_| validation_failure("automation rejection encoding failed"))
        }
        Err(collaboration_client::AutomationInspectionClientError::Connection(error)) => {
            failure(error, OperationEffect::None)
        }
    }
}

macro_rules! domain_error_converter {
    ($function:ident, $error:ty, $rejected:path, $connection:path) => {
        fn $function<TValue: serde::Serialize>(
            result: Result<TValue, $error>,
            mutation: bool,
        ) -> CallToolResult {
            match result {
                Ok(value) => structured_result(Ok(value), OperationEffect::None),
                Err($rejected(error)) => serde_json::to_value(error)
                    .map(CallToolResult::structured_error)
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

fn wake_wait_failure(error: collaboration_client::WakeWaitError) -> CallToolResult {
    use collaboration_client::WakeWaitError;
    match error {
        WakeWaitError::Connection(error) => failure(error, OperationEffect::None),
        WakeWaitError::NotFound { wakeup_id } => CallToolResult::structured_error(
            serde_json::json!({"kind":"notFound","stage":"subscribe","effect":"none","wakeupId":wakeup_id}),
        ),
        WakeWaitError::Unavailable { wakeup_id } => CallToolResult::structured_error(
            serde_json::json!({"kind":"unavailable","stage":"wait","effect":"none","wakeupId":wakeup_id}),
        ),
        WakeWaitError::Paused { wakeup_id } => CallToolResult::structured_error(
            serde_json::json!({"kind":"paused","stage":"wait","effect":"none","wakeupId":wakeup_id}),
        ),
        WakeWaitError::Cancelled { wakeup_id } => CallToolResult::structured_error(
            serde_json::json!({"kind":"cancelled","stage":"wait","effect":"none","wakeupId":wakeup_id}),
        ),
        WakeWaitError::Expired { wakeup_id } => CallToolResult::structured_error(
            serde_json::json!({"kind":"expired","stage":"wait","effect":"none","wakeupId":wakeup_id}),
        ),
        WakeWaitError::FinishedWithoutFiring { wakeup_id } => CallToolResult::structured_error(
            serde_json::json!({"kind":"finishedWithoutFiring","stage":"wait","effect":"none","wakeupId":wakeup_id}),
        ),
    }
}

fn validation_failure(message: &str) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({
        "kind": "protocolViolation",
        "stage": "validation",
        "effect": "none",
        "message": message,
        "code": null,
        "data": null
    }))
}

impl ServerHandler for CollaborationMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("codex-router-collaboration", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        self.tool_router
            .call(ToolCallContext::new(self, request, context))
            .await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult::with_all_items(self.resolved_tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.resolved_tools()
            .into_iter()
            .find(|tool| tool.name == name)
    }
}

#[cfg(test)]
mod tests;
