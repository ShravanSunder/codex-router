use collaboration_client::{
    BoundedObservationRequest, BoundedObservationResult, ClientError, ControlClient,
    ConversationCancelInput, ConversationClient, ConversationClientError,
    ConversationCreatePromptOutcome, ConversationOperationResult, MessageSendError,
    MessageSendRequest, NativeObservation, OperationEffect, operation_failure_from_client_error,
};
use collaboration_protocol::{
    AddressListParams, AddressPage, ApprovalDecideParams, ApprovalDecideResult, ApprovalListParams,
    ApprovalListResult, ConversationCreateOutcome, ConversationOperationSubmission,
    DeliveryOutcome, DeliveryReceipt, EndpointInventory, JournalPage, JournalReadParams,
    JournalStatus, NativeInspectParams, NativeInspectResult, NativeInterruptParams,
    NativeInterruptResult, NativeRenameParams, NativeRenameResult, NativeSessionListParams,
    NativeSessionListResult, OperationId,
};
use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRoute,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    schemars, tool, tool_router,
};
use serde::Deserialize;
use std::os::unix::fs::MetadataExt;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const MCP_INSTALLED_VERSION_TIMEOUT: Duration = Duration::from_secs(2);

mod catalog_descriptions;
mod catalog_tools;
mod conversation_operation_tools;
mod conversation_tool_requests;
mod conversation_tool_results;
mod schema_binding;
use catalog_descriptions::operation_description;
use catalog_tools::{
    EmptyToolInput, ThreadWaitToolInput, register_automation_inspection_tools,
    register_automation_mutation_tools, register_board_tools,
};
use conversation_operation_tools::register_conversation_operation_tools;
use conversation_tool_requests::{
    ConversationCreatePromptToolRequest, ConversationCreateToolRequest,
    ConversationLoadToolRequest, ConversationPromptToolRequest,
};
#[cfg(test)]
use conversation_tool_results::common_prompt_tool_result;
use conversation_tool_results::{
    conversation_call_cancelled, conversation_create_tool_result, conversation_tool_result,
};
use schema_binding::{bind_native_schema_refs, load_advertised_native_definitions};

#[derive(Clone, Debug)]
pub(crate) struct CollaborationMcpServer {
    service_directory: PathBuf,
    tool_router: ToolRouter<Self>,
    _lifecycle: ActiveServiceGuard,
    router_executable_observation: Arc<std::sync::Mutex<McpExecutableObservation>>,
}

pub(crate) struct McpExecutableObservation {
    launch_path: Option<PathBuf>,
    running_version: String,
    startup_identity: Option<McpExecutableFileIdentity>,
    #[cfg(test)]
    installed_version_override: Option<String>,
}

impl std::fmt::Debug for McpExecutableObservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpExecutableObservation")
            .field("launch_path", &self.launch_path)
            .field("running_version", &self.running_version)
            .field("startup_identity", &self.startup_identity)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct McpExecutableFileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

impl McpExecutableObservation {
    pub(crate) fn capture() -> Self {
        let running_version = std::env::var("CODEX_ROUTER_DEBUG_RUNNING_VERSION")
            .ok()
            .filter(|_| cfg!(debug_assertions))
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
        let launch_path = std::env::current_exe().ok();
        let startup_identity = launch_path
            .as_deref()
            .and_then(|path| mcp_executable_file_identity(path).ok());
        Self {
            launch_path,
            running_version,
            startup_identity,
            #[cfg(test)]
            installed_version_override: None,
        }
    }

    #[cfg(test)]
    fn capture_from(launch_path: Option<PathBuf>, running_version: String) -> Self {
        let startup_identity = launch_path
            .as_deref()
            .and_then(|path| mcp_executable_file_identity(path).ok());
        Self {
            launch_path,
            running_version,
            startup_identity,
            installed_version_override: None,
        }
    }

    fn drift_warning(&mut self) -> Option<String> {
        let path = self.launch_path.as_deref()?;
        let current_identity = match mcp_executable_file_identity(path) {
            Ok(identity) => Some(identity),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_error) => return None,
        };
        if current_identity.is_some() && current_identity == self.startup_identity {
            return None;
        }
        let installed_version = match current_identity {
            Some(_) => match self.installed_version(path) {
                Some(version) if version == self.running_version => {
                    self.startup_identity = current_identity;
                    return None;
                }
                Some(version) => version,
                None => return None,
            },
            None => "unknown".to_owned(),
        };
        Some(format!(
            "⚠ Router Host is stale (running {}, installed {installed_version}); run `codex-router host restart`",
            self.running_version
        ))
    }

    fn installed_version(&self, path: &std::path::Path) -> Option<String> {
        #[cfg(test)]
        if let Some(version) = &self.installed_version_override {
            return Some(version.clone());
        }
        mcp_installed_version(path)
    }
}

fn mcp_executable_file_identity(
    path: &std::path::Path,
) -> std::io::Result<McpExecutableFileIdentity> {
    let metadata = std::fs::metadata(path)?;
    Ok(McpExecutableFileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.size(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
    })
}

fn mcp_installed_version(path: &std::path::Path) -> Option<String> {
    let executable_path = path.to_owned();
    if tokio::runtime::Handle::try_current().is_ok() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let _task = tokio::task::spawn_blocking(move || {
            let result = run_bounded_version_command(&executable_path);
            let _sent = sender.send(result);
        });
        return receiver
            .recv_timeout(MCP_INSTALLED_VERSION_TIMEOUT + Duration::from_millis(100))
            .ok()
            .flatten();
    }
    run_bounded_version_command(&executable_path)
}

fn run_bounded_version_command(path: &std::path::Path) -> Option<String> {
    let mut child = std::process::Command::new(path)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + MCP_INSTALLED_VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let output = child.wait_with_output().ok()?;
                return codex_native_integration::parse_executable_version(&output.stdout).ok();
            }
            Ok(Some(_status)) => return None,
            Err(_error) => return None,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _killed = child.kill();
                let _reaped = child.wait();
                return None;
            }
        }
    }
}

#[derive(Debug)]
struct ActiveServiceGuard(Arc<AtomicUsize>);

impl ActiveServiceGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Clone for ActiveServiceGuard {
    fn clone(&self) -> Self {
        Self::new(Arc::clone(&self.0))
    }
}

impl Drop for ActiveServiceGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl CollaborationMcpServer {
    #[cfg(test)]
    pub(crate) fn new(service_directory: PathBuf) -> Self {
        Self::with_lifecycle(
            service_directory,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(std::sync::Mutex::new(McpExecutableObservation::capture())),
        )
    }

    pub(crate) fn with_lifecycle(
        service_directory: PathBuf,
        active_services: Arc<AtomicUsize>,
        router_executable_observation: Arc<std::sync::Mutex<McpExecutableObservation>>,
    ) -> Self {
        let mut tool_router = Self::tool_router();
        register_board_tools(&mut tool_router);
        register_automation_inspection_tools(&mut tool_router);
        register_automation_mutation_tools(&mut tool_router);
        register_conversation_operation_tools(&mut tool_router);
        Self {
            service_directory,
            tool_router,
            _lifecycle: ActiveServiceGuard::new(active_services),
            router_executable_observation,
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

    pub(crate) fn resolved_tools(&self) -> Vec<Tool> {
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
                    normalize_boolean_json_schemas(&mut output);
                    if let serde_json::Value::Object(mut fields) = output {
                        fields
                            .entry("type".to_owned())
                            .or_insert_with(|| serde_json::Value::String("object".to_owned()));
                        tool.output_schema = Some(std::sync::Arc::new(fields));
                    }
                }
                tool
            })
            .collect()
    }
}

fn normalize_boolean_json_schemas(schema: &mut serde_json::Value) {
    match schema {
        serde_json::Value::Bool(true) => {
            *schema = serde_json::Value::Object(serde_json::Map::new());
        }
        serde_json::Value::Bool(false) => {
            *schema = serde_json::json!({"not": {}});
        }
        serde_json::Value::Object(fields) => {
            for keyword in [
                "additionalItems",
                "additionalProperties",
                "contains",
                "contentSchema",
                "else",
                "if",
                "items",
                "not",
                "propertyNames",
                "then",
                "unevaluatedItems",
                "unevaluatedProperties",
            ] {
                if let Some(subschema) = fields.get_mut(keyword) {
                    normalize_schema_or_schema_array(subschema);
                }
            }
            for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
                if let Some(serde_json::Value::Array(subschemas)) = fields.get_mut(keyword) {
                    for subschema in subschemas {
                        normalize_boolean_json_schemas(subschema);
                    }
                }
            }
            for keyword in [
                "$defs",
                "definitions",
                "dependentSchemas",
                "patternProperties",
                "properties",
            ] {
                if let Some(serde_json::Value::Object(subschemas)) = fields.get_mut(keyword) {
                    for subschema in subschemas.values_mut() {
                        normalize_boolean_json_schemas(subschema);
                    }
                }
            }
        }
        _ => {}
    }
}

fn normalize_schema_or_schema_array(schema: &mut serde_json::Value) {
    if let serde_json::Value::Array(subschemas) = schema {
        for subschema in subschemas {
            normalize_boolean_json_schemas(subschema);
        }
    } else {
        normalize_boolean_json_schemas(schema);
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

    #[tool(name = "message_send", description = "Submits one agent-authored or explicit human message with exact auto, queue, or steer semantics. The receipt reports the selected route and strongest observed outcome; accepted input or a peer write does not prove completion or an agent reply. Router-authored content is not a public caller input, and uncertain dispatch is never replayed automatically.", output_schema = rmcp::handler::server::tool::schema_for_type::<DeliveryReceipt>())]
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
        match result {
            Ok(value) => structured_result(Ok(value), OperationEffect::None),
            Err(error) => {
                let (failure, target, turn_id) = error.into_parts();
                operation_error_result(failure, target, turn_id)
            }
        }
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
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        let wakeup_id = request.wakeup_id.clone();
        let client = match tokio::select! {
            _ = context.ct.cancelled() => {
                return wake_wait_failure(collaboration_client::WakeWaitError::CallerCancelled { wakeup_id: wakeup_id.clone() });
            }
            result = self.connect() => result,
        } {
            Ok(value) => value,
            Err(error) => {
                return wake_wait_failure(collaboration_client::WakeWaitError::Connection(error));
            }
        };
        let wait = match tokio::select! {
            _ = context.ct.cancelled() => {
                return wake_wait_failure(collaboration_client::WakeWaitError::CallerCancelled { wakeup_id });
            }
            result = client.subscribe_wakeup(request) => result,
        } {
            Ok(value) => value,
            Err(error) => return wake_wait_failure(error),
        };
        match wait
            .wait_until_first_fire_with_cancellation(context.ct)
            .await
        {
            Ok(value) => structured_result(Ok(value), OperationEffect::None),
            Err(error) => wake_wait_failure(error),
        }
    }

    #[tool(name = "conversation_create", description = "Creates one conversation through its advertised client. Requires a caller UUIDv7 operationId and exact endpoint, working directory, access, and creator; approver defaults to creator. model and effort are required for Codex endpoints and rejected for provider endpoints. Returns created with target or pending with the inspectable operation ID; fork is Codex-only.", output_schema = rmcp::handler::server::tool::schema_for_type::<ConversationCreateOutcome>())]
    async fn conversation_create(
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
            connected = ConversationClient::connect(&self.service_directory, &endpoint) => connected,
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

    #[tool(name = "conversation_load", description = "Loads one conversation through its advertised client using an exact target, working directory, access and requester. External providers require a caller UUIDv7 operation ID; Codex load must omit it because it is not inspectable. Returns a completed settlement or an inspectable provider operation when pending; uncertain work is never replayed.", output_schema = rmcp::handler::server::tool::schema_for_type::<ConversationOperationResult>())]
    async fn conversation_load(
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
            connected = ConversationClient::connect(&self.service_directory, &endpoint) => connected,
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

    #[tool(name = "conversation_prompt", description = "Prompts one conversation through its advertised client. External providers require a caller UUIDv7 operation ID; Codex prompt must omit it because it is not inspectable. The result records a completed turn or a pending provider operation. Caller cancellation detaches from provider work while Codex uses its native cancellation path.", output_schema = rmcp::handler::server::tool::schema_for_type::<ConversationOperationResult>())]
    async fn conversation_prompt(
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
            connected = ConversationClient::connect(&self.service_directory, &endpoint) => connected,
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

    #[tool(name = "conversation_cancel", description = "Requests cancellation of one exact conversation operation. Requires a new operation ID and the target operation, conversation, and requester; accepted cancellation is distinct from confirmed cessation. Codex ACP reports unsupportedCapability with a turn interrupt fix.", output_schema = rmcp::handler::server::tool::schema_for_type::<ConversationOperationSubmission>())]
    async fn conversation_cancel(
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
            connected = ConversationClient::connect(&self.service_directory, &endpoint) => connected,
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

    #[tool(name = "conversation_create_and_prompt", description = "Creates a fresh conversation and prompts it through the advertised client. model and effort are required for Codex endpoints and rejected for provider endpoints. The create operation ID is inspectable; provider prompt requires a second caller UUIDv7 ID, while Codex prompt omits it because it is not inspectable. The result names a pending create or the prompt settlement. A completed turn is not an assignment verdict or peer reply; cancellation never silently replays a submission.", output_schema = rmcp::handler::server::tool::schema_for_type::<ConversationCreatePromptOutcome>())]
    async fn conversation_create_and_prompt(
        &self,
        Parameters(mut request): Parameters<ConversationCreatePromptToolRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        request.input.create.default_approver();
        let operation_id = request.input.create.operation_id.clone();
        let timeout =
            Duration::from_secs(u64::from(request.timeout_seconds.map_or(300, u32::from)));
        let result = ConversationClient::create_and_prompt(
            &self.service_directory,
            request.input,
            timeout,
            context.ct,
        )
        .await;
        conversation_tool_result(result, Some(operation_id))
    }

    #[tool(name = "events_observe", description = "Explicitly attaches to one conversation and returns bounded call-local events. It never interrupts work and provides no replay cursor or ordering guarantee with concurrent sends.", output_schema = rmcp::handler::server::tool::schema_for_type::<BoundedObservationResult>())]
    async fn events_observe(
        &self,
        Parameters(request): Parameters<BoundedObservationRequest>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        match NativeObservation::observe_bounded(&self.service_directory, request, context.ct).await
        {
            Ok(value) => structured_result(Ok(value), OperationEffect::None),
            Err(error) => {
                let (failure, target, turn_id) = error.into_parts();
                operation_error_result(failure, target, turn_id)
            }
        }
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

fn operation_error_result(
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
            CallToolResult::structured_error(value)
        })
        .unwrap_or_else(|_| validation_failure("collaboration error encoding failed"))
}

fn message_tool_result(result: Result<DeliveryReceipt, MessageSendError>) -> CallToolResult {
    match result {
        Ok(receipt)
            if matches!(
                receipt.outcome,
                DeliveryOutcome::Rejected(_)
                    | DeliveryOutcome::NotSubmitted { .. }
                    | DeliveryOutcome::Unknown
            ) =>
        {
            serde_json::to_value(receipt)
                .map(CallToolResult::structured_error)
                .unwrap_or_else(|_| validation_failure("delivery receipt encoding failed"))
        }
        Ok(receipt) => structured_result(Ok(receipt), OperationEffect::None),
        Err(error) => {
            let (failure, target) = error.into_operation_failure_and_target();
            serde_json::to_value(failure)
                .map(|mut value| {
                    if let Some(fields) = value.as_object_mut() {
                        fields.insert("target".to_owned(), serde_json::json!(target));
                    }
                    CallToolResult::structured_error(value)
                })
                .unwrap_or_else(|_| validation_failure("collaboration error encoding failed"))
        }
    }
}

fn failure(error: ClientError, possible_effect: OperationEffect) -> CallToolResult {
    let failure = operation_failure_from_client_error(error, possible_effect);
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
    serde_json::to_value(error.into_operation_failure())
        .map(CallToolResult::structured_error)
        .unwrap_or_else(|_| validation_failure("wake wait error encoding failed"))
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
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "codex-router-collaboration",
                env!("CARGO_PKG_VERSION"),
            ));
        if let Ok(mut observation) = self.router_executable_observation.lock()
            && let Some(warning) = observation.drift_warning()
        {
            config = config.with_instructions(warning);
        }
        config
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
        let tools = self.resolved_tools();
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.resolved_tools()
            .into_iter()
            .find(|tool| tool.name == name)
    }
}

#[cfg(test)]
mod executable_observation_tests {
    use super::CollaborationMcpServer;
    use super::McpExecutableObservation;
    use rmcp::ServerHandler;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn initialize_instructions_report_running_and_installed_router_versions() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("codex-router");
        write_version_script(&path, "0.1.36");
        let server = CollaborationMcpServer::new(directory.path().to_owned());
        let mut observation =
            McpExecutableObservation::capture_from(Some(path.clone()), "0.1.36".to_owned());
        observation.installed_version_override = Some("0.1.37".to_owned());
        *server
            .router_executable_observation
            .lock()
            .expect("executable observer lock") = observation;
        write_version_script(&path, "0.1.37");
        let initialize = serde_json::to_value(server.get_info()).expect("initialize response");
        assert_eq!(
            initialize["instructions"].as_str(),
            Some(
                "⚠ Router Host is stale (running 0.1.36, installed 0.1.37); run `codex-router host restart`"
            )
        );
    }

    fn write_version_script(path: &std::path::Path, version: &str) {
        std::fs::write(
            path,
            format!("#!/bin/sh\nprintf 'codex-router {version}\\n'\n"),
        )
        .expect("write version script");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("make version script executable");
    }
}

#[cfg(test)]
mod conversation_result_tests;

#[cfg(test)]
mod tests;
