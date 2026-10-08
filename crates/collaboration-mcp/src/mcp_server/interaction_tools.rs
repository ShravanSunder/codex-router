//! Approval, question, provider-settings and provider Session lifecycle tools.
use super::*;

#[tool_router(router = interaction_tool_router, vis = "pub(super)")]
impl CollaborationMcpServer {
    #[tool(name = "approval_list", description = "Lists approval requests. Set includeOptions for offered choices and persistent effects. Read-only.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ApprovalListResponse>>())]
    pub(super) async fn approval_list(
        &self,
        Parameters(request): Parameters<ApprovalListParams>,
    ) -> CallToolResult {
        application_result(
            self.application.interactions().approval_list(request).await,
            OperationEffect::None,
        )
    }

    #[tool(name = "approval_decide", description = "Records one approval decision through the existing authorization checks. MCP never auto-approves.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ApprovalDecideResult>>())]
    pub(super) async fn approval_decide(
        &self,
        Parameters(request): Parameters<ApprovalDecideParams>,
    ) -> CallToolResult {
        match self
            .application
            .interactions()
            .approval_decide(request)
            .await
        {
            Ok(value) => success_result(&value),
            Err(rejection) => staged_failure_result(&rejection, "approval-decision"),
        }
    }

    #[tool(name = "question_list", description = "Lists questions with their typed fields and current state. Read-only.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<collaboration_protocol::QuestionListResult>>())]
    pub(super) async fn question_list(
        &self,
        Parameters(request): Parameters<collaboration_protocol::QuestionListParams>,
    ) -> CallToolResult {
        application_result(
            self.application.interactions().question_list(request).await,
            OperationEffect::None,
        )
    }

    #[tool(name = "question_answer", description = "Answers, declines, or cancels one question as its Approver. Choice answers carry selectedOptionIds from the offered options; labels are display text.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<collaboration_protocol::QuestionAnswerResult>>())]
    pub(super) async fn question_answer(
        &self,
        Parameters(request): Parameters<collaboration_protocol::QuestionAnswerParams>,
    ) -> CallToolResult {
        match self
            .application
            .interactions()
            .question_answer(request)
            .await
        {
            Ok(value) => success_result(&value),
            Err(rejection) => staged_failure_result(&rejection, "question-answer"),
        }
    }

    #[tool(name = "conversation_settings_set", description = "Sets one advertised provider Session mode, model, or effort as its creator or Approver. An ambiguous provider response leaves settings gated.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ProviderSettingsResult>>())]
    pub(super) async fn conversation_settings_set(
        &self,
        Parameters(request): Parameters<ProviderSettingsSetRequest>,
    ) -> CallToolResult {
        typed_failure_result::<ProviderSettingsFailure, _, _>(
            self.application.conversations().settings_set(request).await,
            OperationEffect::Unknown,
        )
    }

    #[tool(name = "conversation_settings_accept", description = "Accepts the provider Session's currently reported settings as its creator or Approver and clears the prompt gate without an agent RPC.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ProviderSettingsResult>>())]
    pub(super) async fn conversation_settings_accept(
        &self,
        Parameters(request): Parameters<ProviderSettingsAcceptRequest>,
    ) -> CallToolResult {
        typed_failure_result::<ProviderSettingsFailure, _, _>(
            self.application
                .conversations()
                .settings_accept(request)
                .await,
            OperationEffect::None,
        )
    }

    #[tool(name = "conversation_resume", description = "Resumes one advertised provider Session as an inspectable operation. The agent supplies no history replay; inspect the operation ID after an uncertain response.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ConversationOperationSubmission>>())]
    pub(super) async fn conversation_resume(
        &self,
        Parameters(request): Parameters<ConversationResumeRequest>,
    ) -> CallToolResult {
        application_result(
            self.application
                .conversations()
                .conversation_resume(request)
                .await,
            OperationEffect::Unknown,
        )
    }

    #[tool(name = "conversation_close", description = "Closes one provider Session after its running Turn settles. The operation ID remains inspectable after an uncertain response.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<ConversationOperationSubmission>>())]
    pub(super) async fn conversation_close(
        &self,
        Parameters(request): Parameters<ConversationCloseRequest>,
    ) -> CallToolResult {
        application_result(
            self.application
                .conversations()
                .conversation_close(request)
                .await,
            OperationEffect::Unknown,
        )
    }

    #[tool(name = "automation_status", description = "Reads automation readiness and configured attempt budgets without mutating them.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<collaboration_protocol::AutomationStatus>>())]
    pub(super) async fn automation_status(
        &self,
        Parameters(EmptyToolInput {}): Parameters<EmptyToolInput>,
    ) -> CallToolResult {
        success_result(&self.application.automation().automation_status().await)
    }
}

/// An approval or answer refused by the broker: the decision's outcome is unknown to the
/// caller, at the named stage, with no target or turn known.
fn staged_failure_result(
    rejection: &impl collaboration_service::collaboration_application::CollaborationRejection,
    stage: &str,
) -> CallToolResult {
    let mut failure = rejection_failure(rejection, OperationEffect::Unknown);
    stage.clone_into(&mut failure.stage);
    operation_error_result(failure, None, None)
}
