//! Endpoint, session, lifecycle-journal and address-book tools.
use super::*;

#[tool_router(router = session_tool_router, vis = "pub(super)")]
impl CollaborationMcpServer {
    #[tool(name = "endpoints_list", description = "Lists collaboration endpoints. Read-only; successful discovery is not evidence that an agent assignment completed.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<EndpointInventory>>())]
    pub(super) async fn endpoints_list(
        &self,
        Parameters(EmptyToolInput {}): Parameters<EmptyToolInput>,
    ) -> CallToolResult {
        application_result(
            self.application.sessions().endpoints_list(),
            OperationEffect::None,
        )
    }

    #[tool(name = "sessions_list", description = "Lists stored, loaded, or active Codex conversations using the selected scope. Claude Code sessions use provider_sessions_list. Read-only and never resumes a conversation.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<NativeSessionListResult>>())]
    pub(super) async fn sessions_list(
        &self,
        Parameters(request): Parameters<NativeSessionListParams>,
    ) -> CallToolResult {
        if String::from(request.endpoint.endpoint_id.clone()) == "claude-local" {
            return failure(
                ClientError::Rejected {
                    code: -32050,
                    data: Some(serde_json::json!({
                        "kind": "unsupportedCapability",
                        "stage": "discovery",
                        "message": "Use provider_sessions_list for Claude Code sessions"
                    })),
                },
                OperationEffect::None,
            );
        }
        application_result(
            self.application
                .sessions()
                .codex_session_list(request, API_RESULT_BUDGET)
                .await,
            OperationEffect::None,
        )
    }

    #[tool(name = "provider_sessions_list", description = "Lists Router-hosted provider Sessions and live Claude Code terminal sessions. Claude terminal discovery supports active or loaded views; stored applies only to hosted Sessions. Live pages are not snapshots, so sessions may move between pages.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ProviderSessionListResult>>())]
    pub(super) async fn provider_sessions_list(
        &self,
        Parameters(request): Parameters<ProviderSessionListParams>,
    ) -> CallToolResult {
        application_result(
            self.application
                .sessions()
                .provider_session_list(request, API_RESULT_BUDGET)
                .await,
            OperationEffect::None,
        )
    }

    #[tool(name = "provider_session_inspect", description = "Inspects one Router-owned provider Session, including live state, capability report, authentication status, and last advertised settings options.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ProviderSessionInspectResult>>())]
    pub(super) async fn provider_session_inspect(
        &self,
        Parameters(request): Parameters<ProviderSessionInspectRequest>,
    ) -> CallToolResult {
        typed_failure_result::<ProviderInspectFailure, _, _>(
            self.application
                .conversations()
                .provider_session_inspect(request)
                .await,
            OperationEffect::None,
        )
    }

    #[tool(name = "session_inspect", description = "Inspects one exact conversation target without changing its identity. Attachment or backend failures retain structured target/effect evidence.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<NativeInspectResult>>())]
    pub(super) async fn session_inspect(
        &self,
        Parameters(request): Parameters<NativeInspectParams>,
    ) -> CallToolResult {
        application_result(
            self.application
                .sessions()
                .codex_session_inspect(request, API_RESULT_BUDGET)
                .await,
            OperationEffect::None,
        )
    }

    #[tool(name = "session_rename", description = "Renames one exact conversation. Dispatches once and never automatically replays an uncertain mutation.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<NativeRenameResult>>())]
    pub(super) async fn session_rename(
        &self,
        Parameters(request): Parameters<NativeRenameParams>,
    ) -> CallToolResult {
        application_result(
            self.application
                .sessions()
                .codex_session_rename(request)
                .await,
            OperationEffect::Unknown,
        )
    }

    #[tool(name = "turn_interrupt", description = "Interrupts only the supplied exact turn with its generation guard. A receipt does not assert assignment success or observed cessation.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<NativeInterruptResult>>())]
    pub(super) async fn turn_interrupt(
        &self,
        Parameters(request): Parameters<NativeInterruptParams>,
    ) -> CallToolResult {
        application_result(
            self.application
                .sessions()
                .codex_turn_interrupt(request, API_RESULT_BUDGET)
                .await,
            OperationEffect::Unknown,
        )
    }

    #[tool(name = "journal_status", description = "Reads lifecycle-journal availability and bounds without mutating state.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<JournalStatus>>())]
    pub(super) async fn journal_status(
        &self,
        Parameters(EmptyToolInput {}): Parameters<EmptyToolInput>,
    ) -> CallToolResult {
        success_result(&self.application.sessions().journal_status().await)
    }

    #[tool(name = "journal_read", description = "Reads a bounded lifecycle-journal page. Read-only and not a durable conversation transcript.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<JournalPage>>())]
    pub(super) async fn journal_read(
        &self,
        Parameters(request): Parameters<JournalReadParams>,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> CallToolResult {
        let sessions = self.application.sessions();
        tokio::select! {
            () = context.ct.cancelled() => caller_cancelled(OperationEffect::None),
            result = sessions.journal_read(request) => {
                application_result(result, OperationEffect::None)
            }
        }
    }

    #[tool(name = "addresses_list", description = "Lists known conversation addresses and their observed lifecycle coverage. Read-only.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<AddressPage>>())]
    pub(super) async fn addresses_list(
        &self,
        Parameters(request): Parameters<AddressListParams>,
    ) -> CallToolResult {
        application_result(
            self.application.sessions().address_list(request).await,
            OperationEffect::None,
        )
    }
}
