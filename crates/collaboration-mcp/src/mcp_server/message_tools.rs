//! Direct-message tools: send, reply, push inspection, inbox and history.
use super::*;
use collaboration_protocol::{SessionMessageReplyParams, SessionMessageSendParams};
use collaboration_service::collaboration_application::{MessageFailure, MessageFailureKind};

#[tool_router(router = message_tool_router, vis = "pub(super)")]
impl CollaborationMcpServer {
    #[tool(name = "message_send", description = "Submits one agent-authored or explicit human message with exact auto, queue, or steer semantics. Agent sender identity is reported by the request's client, not authenticated. The result includes the stored push id, link, target and delivery receipt; accepted input or a peer write does not prove completion or an agent reply. Router-authored content is not a public caller input, and uncertain dispatch is never replayed automatically.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<PushMessageSendResult>>())]
    pub(super) async fn message_send(
        &self,
        Parameters(request): Parameters<MessageSendRequest>,
    ) -> CallToolResult {
        let target = request.target.clone();
        let params = SessionMessageSendParams {
            target: request.target,
            message: request.message.into(),
            mode: request.delivery,
            generation_guard: request.generation_guard,
        };
        match self.application.messages().message_send(params).await {
            Ok(push) => message_receipt_result(push),
            Err(rejection) => message_failure_result(&rejection, "target", target),
        }
    }

    #[tool(name = "router_show", description = "Shows one stored Router push by id or router link, including its full body or expanded thread activity. The caller is reported by the MCP request, not authenticated; the service uses it as a confusion guard. A direct-message show by its target marks the message read.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<PushRecordShowResult>>())]
    pub(super) async fn router_show(
        &self,
        Parameters(request): Parameters<PushRecordShowParams>,
    ) -> CallToolResult {
        application_result(
            self.application.messages().push_show(request).await,
            OperationEffect::None,
        )
    }

    #[tool(name = "message_inbox", description = "Lists retained unread direct messages for the reported caller session. Caller identity is supplied by the MCP request and is not authenticated; the service uses it as a confusion guard.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<PushRecordListResult>>())]
    pub(super) async fn message_inbox(
        &self,
        Parameters(request): Parameters<PushRecordListParams>,
    ) -> CallToolResult {
        application_result(
            self.application.messages().message_inbox(request).await,
            OperationEffect::None,
        )
    }

    #[tool(name = "message_history", description = "Lists retained direct messages between the reported caller session and the explicit with session. Caller identity is supplied by the MCP request and is not authenticated; the service uses it as a confusion guard.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<PushRecordListResult>>())]
    pub(super) async fn message_history(
        &self,
        Parameters(request): Parameters<PushRecordHistoryParams>,
    ) -> CallToolResult {
        application_result(
            self.application.messages().message_history(request).await,
            OperationEffect::None,
        )
    }

    #[tool(name = "message_reply", description = "Replies to one direct message by its stored push id or router link, and records that reference on the reply. Caller identity is reported by the MCP request, not authenticated; the service uses it as a confusion guard. The result names the selected recipient.", output_schema = rmcp::handler::server::tool::schema_for_type::<McpToolOutput<SessionMessageReplyResult>>())]
    pub(super) async fn message_reply(
        &self,
        Parameters(request): Parameters<MessageReplyRequest>,
    ) -> CallToolResult {
        let caller = request.caller.clone();
        let params = SessionMessageReplyParams {
            caller: request.caller,
            reference: request.reference,
            text: request.text,
        };
        match self.application.messages().message_reply(params).await {
            Ok(reply) => message_reply_receipt_result(reply),
            Err(rejection) => message_failure_result(&rejection, "caller", caller),
        }
    }
}

/// A refused send or reply. Refused before anything was stored, it had no effect and names no
/// session; once stored, its delivery is unknown and it names the session it concerned, as
/// `field`.
pub(super) fn message_failure_result(
    rejection: &MessageFailure,
    field: &str,
    session: collaboration_protocol::SessionRef,
) -> CallToolResult {
    if rejected_before_submission(rejection) {
        failure_naming(
            &rejection_failure(rejection, OperationEffect::None),
            field,
            None,
        )
    } else {
        failure_naming(
            &rejection_failure(rejection, OperationEffect::Unknown),
            field,
            Some(session),
        )
    }
}

/// Whether the message was refused before anything was stored or sent.
fn rejected_before_submission(rejection: &MessageFailure) -> bool {
    match rejection.kind {
        MessageFailureKind::InvalidField
        | MessageFailureKind::WrongService
        | MessageFailureKind::Unavailable
        | MessageFailureKind::NotFound
        | MessageFailureKind::NotPermitted
        | MessageFailureKind::NotDirectMessage
        | MessageFailureKind::OwnerReplyUnsupported => true,
        MessageFailureKind::ForeignMachine | MessageFailureKind::OutcomeUnknown => false,
    }
}
