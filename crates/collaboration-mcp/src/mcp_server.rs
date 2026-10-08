//! The collaboration MCP server: one stateless handler per request over the typed application.
//!
//! Tools call `CollaborationApplication` in process. The conversation tools that open carrier
//! sessions (create, load, prompt, cancel, create-and-prompt and bounded observation) run the
//! carrier clients in the Host over the same application.
use crate::collaboration_api_router::CollaborationApiConfig;
use crate::native_schema_definitions::NativeSchemaDefinitions;
use collaboration_client::{
    BoundedObservationRequest, BoundedObservationResult, ClientError, ConversationCancelInput,
    ConversationClient, ConversationClientError, ConversationCreatePromptOutcome,
    ConversationOperationResult, MessageReplyRequest, MessageSendRequest, OperationEffect,
    operation_failure_from_client_error,
};
use collaboration_protocol::{
    AddressListParams, AddressPage, ApprovalDecideParams, ApprovalDecideResult, ApprovalListParams,
    ApprovalListResponse, ConversationCloseRequest, ConversationCreateOutcome,
    ConversationOperationSubmission, ConversationResumeRequest, DeliveryOutcome, EndpointInventory,
    JournalPage, JournalReadParams, JournalStatus, NativeInspectParams, NativeInspectResult,
    NativeInterruptParams, NativeInterruptResult, NativeRenameParams, NativeRenameResult,
    NativeSessionListParams, NativeSessionListResult, OperationId, ProviderInspectFailure,
    ProviderSessionInspectRequest, ProviderSessionInspectResult, ProviderSessionListParams,
    ProviderSessionListResult, ProviderSettingsAcceptRequest, ProviderSettingsFailure,
    ProviderSettingsResult, ProviderSettingsSetRequest, PushDeliveryState, PushMessageSendResult,
    PushRecordHistoryParams, PushRecordListParams, PushRecordListResult, PushRecordShowParams,
    PushRecordShowResult, RouterExecutableRelation, SessionMessageReplyResult,
    ThreadSubscriptionWaitRequest, ThreadSubscriptionWaitResult, router_build_warning,
};
use collaboration_service::CollaborationApplication;
use collaboration_service::collaboration_application::API_RESULT_BUDGET;
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
use std::{sync::Arc, time::Duration};

mod application_tool_results;
mod board_argument_classification;
mod carrier_tools;
mod catalog_descriptions;
mod catalog_tools;
mod conversation_operation_tools;
mod conversation_tool_requests;
mod conversation_tool_results;
mod interaction_tools;
mod message_tools;
mod result_converters;
mod schema_binding;
mod session_tools;
mod tool_output_contract;
mod wait_tools;
use application_tool_results::*;
use catalog_descriptions::operation_description;
use catalog_tools::{
    EmptyToolInput, register_automation_inspection_tools, register_automation_mutation_tools,
    register_board_tools,
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
#[cfg(test)]
use message_tools::message_failure_result;
pub(crate) use result_converters::structured_tool_error;
use result_converters::*;
use schema_binding::bind_native_schema_refs;
use tool_output_contract::{McpToolError, McpToolOutput};

/// The tools every server copy offers, routed and with their schemas bound once.
pub(crate) struct ToolSurface {
    router: ToolRouter<CollaborationMcpServer>,
    tools: Vec<Tool>,
}

impl ToolSurface {
    pub(crate) fn new(native_definitions: Option<&NativeSchemaDefinitions>) -> Self {
        let mut router = CollaborationMcpServer::session_tool_router()
            + CollaborationMcpServer::message_tool_router()
            + CollaborationMcpServer::interaction_tool_router()
            + CollaborationMcpServer::wait_tool_router()
            + CollaborationMcpServer::carrier_tool_router();
        register_board_tools(&mut router);
        register_automation_inspection_tools(&mut router);
        register_automation_mutation_tools(&mut router);
        register_conversation_operation_tools(&mut router);
        let tools = resolve_tool_schemas(
            &router,
            native_definitions.map(NativeSchemaDefinitions::definitions),
        );
        Self { router, tools }
    }

    #[cfg(test)]
    pub(crate) fn tools(&self) -> &[Tool] {
        &self.tools
    }
}

/// The handler rmcp builds for each request. It holds the Router's application; its tool
/// calls run through the listener's registry, so shutdown can stop and join them.
#[derive(Clone)]
pub(crate) struct CollaborationMcpServer {
    surface: Arc<ToolSurface>,
    application: CollaborationApplication,
    /// How the carrier tools reach the Router: in process, with carrier sockets in this
    /// service directory.
    carrier_access: collaboration_client::CollaborationAccess,
    router_executable_relation: tokio::sync::watch::Receiver<RouterExecutableRelation>,
    /// The listener's running calls, which its shutdown aborts and joins.
    tool_calls: crate::tool_call_registry::ToolCallRegistry,
}

impl CollaborationMcpServer {
    pub(crate) fn new(
        config: &CollaborationApiConfig,
        surface: Arc<ToolSurface>,
        tool_calls: crate::tool_call_registry::ToolCallRegistry,
    ) -> Self {
        Self {
            surface,
            application: config.application.clone(),
            carrier_access: carrier_access(&config.application, &config.service_directory),
            router_executable_relation: config.router_executable_relation.clone(),
            tool_calls,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_application(
        application: CollaborationApplication,
        service_directory: std::path::PathBuf,
    ) -> Self {
        let (_sender, relation) = tokio::sync::watch::channel(RouterExecutableRelation::Match);
        Self {
            surface: Arc::new(ToolSurface::new(None)),
            carrier_access: carrier_access(&application, &service_directory),
            application,
            router_executable_relation: relation,
            tool_calls: crate::tool_call_registry::ToolCallRegistry::default(),
        }
    }

    /// A server over a Router with no stores, for tests that read its tool catalog.
    #[cfg(test)]
    pub(crate) fn catalog_only() -> Self {
        Self::for_application(
            CollaborationApplication::new(crate::api_test_harness::test_identity()),
            std::env::temp_dir(),
        )
    }

    #[cfg(test)]
    pub(crate) fn resolved_tools(&self) -> Vec<Tool> {
        self.surface.tools.clone()
    }

    #[cfg(test)]
    pub(crate) fn with_router_relation(
        mut self,
        relation: tokio::sync::watch::Receiver<RouterExecutableRelation>,
    ) -> Self {
        self.router_executable_relation = relation;
        self
    }
}

fn carrier_access(
    application: &CollaborationApplication,
    service_directory: &std::path::Path,
) -> collaboration_client::CollaborationAccess {
    collaboration_client::CollaborationAccess::local(
        service_directory,
        Arc::new(crate::local_collaboration::ApplicationCollaboration::new(
            application.clone(),
        )),
    )
}

fn resolve_tool_schemas(
    router: &ToolRouter<CollaborationMcpServer>,
    native_definitions: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Vec<Tool> {
    router
        .list_all()
        .into_iter()
        .map(|mut tool| {
            let mut input = serde_json::Value::Object((*tool.input_schema).clone());
            bind_native_schema_refs(&mut input, native_definitions);
            if let serde_json::Value::Object(fields) = input {
                tool.input_schema = Arc::new(fields);
            }
            if let Some(schema) = &tool.output_schema {
                let mut output = serde_json::Value::Object((**schema).clone());
                bind_native_schema_refs(&mut output, native_definitions);
                normalize_boolean_json_schemas(&mut output);
                let success_description = output
                    .pointer("/anyOf/0/$ref")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|reference| reference.strip_prefix('#'))
                    .and_then(|pointer| output.pointer(pointer))
                    .and_then(|success| success.get("description"))
                    .cloned();
                if let serde_json::Value::Object(mut fields) = output {
                    if let Some(description) = success_description {
                        fields.insert("description".to_owned(), description);
                    }
                    fields
                        .entry("type".to_owned())
                        .or_insert_with(|| serde_json::Value::String("object".to_owned()));
                    tool.output_schema = Some(Arc::new(fields));
                }
            }
            tool
        })
        .collect()
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

impl ServerHandler for CollaborationMcpServer {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "codex-router-collaboration",
                env!("CARGO_PKG_VERSION"),
            ));
        let relation = self.router_executable_relation.borrow().clone();
        if let Some(warning) = router_build_warning(&relation) {
            config = config.with_instructions(warning);
        }
        config
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let routed = self
            .surface
            .router
            .call(ToolCallContext::new(self, request, context));
        self.tool_calls
            .run(routed, || {
                Ok(CallToolResponse::Complete(call_abandoned_at_shutdown()))
            })
            .await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult::with_all_items(self.surface.tools.clone())
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.surface
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .cloned()
    }
}

#[cfg(test)]
mod router_relation_instruction_tests;

#[cfg(test)]
mod conversation_result_tests;

#[cfg(test)]
mod tests;
