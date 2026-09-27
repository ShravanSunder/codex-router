//! Notify a Session Approver through the existing agent-message route.

use super::{
    APPROVAL_TIMEOUT, InteractionHistoryError, SessionMessageDelivery, deliver_message_via,
};
use collaboration_protocol::SessionRef as ProtocolSessionRef;
use message_board::{Identity, SessionRef};
use session_event_model::{ApprovalRequest, ApprovalScope, QuestionRequest};
use std::sync::Arc;

pub(super) async fn deliver_approval_notice(
    delivery: Option<&Arc<dyn SessionMessageDelivery>>,
    requester: &SessionRef,
    approver: &Identity,
    request: &ApprovalRequest,
) -> Result<(), InteractionHistoryError> {
    let Identity::Session {
        session: approver_session,
    } = approver
    else {
        return Ok(());
    };
    let actor = serde_json::to_string(approver_session)
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    let commands = request
        .options
        .iter()
        .map(|option| {
            let persistent_target = match &option.choice.scope {
                ApprovalScope::Persistent { where_stored } => Some(where_stored.as_str()),
                ApprovalScope::Once | ApprovalScope::Session => None,
            };
            let command = format!(
                "agent-collaboration approval decide --request-id {} --actor {} --option-id {}{}",
                shell_quote(&request.request_id),
                shell_quote(&actor),
                shell_quote(option.option_id.as_str()),
                if persistent_target.is_some() {
                    " --acknowledge-persistent"
                } else {
                    ""
                },
            );
            serde_json::json!({
                "optionId": option.option_id.as_str(),
                "command": command,
                "persistentTarget": persistent_target,
            })
        })
        .collect::<Vec<_>>();
    let notice = serde_json::json!({
        "kind": "externalProviderPermission",
        "method": "session/request_permission",
        "requestId": request.request_id,
        "requester": requester,
        "approver": approver,
        "request": request,
        "decideCommands": commands,
    });
    deliver_notice(delivery, requester, approver_session, notice).await
}

pub(super) async fn deliver_question_notice(
    delivery: Option<&Arc<dyn SessionMessageDelivery>>,
    requester: &SessionRef,
    approver: &Identity,
    request: &QuestionRequest,
) -> Result<(), InteractionHistoryError> {
    let Identity::Session {
        session: approver_session,
    } = approver
    else {
        return Ok(());
    };
    let actor = serde_json::to_string(approver_session)
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    let answer_command = format!(
        "agent-collaboration question answer --request-id {} --actor {} --content '<answers-json>'",
        shell_quote(&request.request_id),
        shell_quote(&actor),
    );
    let notice = serde_json::json!({
        "kind": "externalProviderQuestion",
        "requestId": request.request_id,
        "requester": requester,
        "approver": approver,
        "request": request,
        "answerCommand": answer_command,
    });
    deliver_notice(delivery, requester, approver_session, notice).await
}

async fn deliver_notice(
    delivery: Option<&Arc<dyn SessionMessageDelivery>>,
    requester: &SessionRef,
    approver: &SessionRef,
    notice: serde_json::Value,
) -> Result<(), InteractionHistoryError> {
    let requester = protocol_session_ref(requester)?;
    let approver = protocol_session_ref(approver)?;
    let text = serde_json::to_string(&notice).map_err(|_| InteractionHistoryError::Unavailable)?;
    let delivery = delivery.ok_or(InteractionHistoryError::Unavailable)?;
    tokio::time::timeout(
        APPROVAL_TIMEOUT,
        deliver_message_via(delivery.as_ref(), requester, approver, text),
    )
    .await
    .map_err(|_| InteractionHistoryError::Unavailable)?
    .map_err(|_| InteractionHistoryError::Unavailable)
}

fn protocol_session_ref(
    session: &SessionRef,
) -> Result<ProtocolSessionRef, InteractionHistoryError> {
    let value = serde_json::to_value(session).map_err(|_| InteractionHistoryError::Unavailable)?;
    serde_json::from_value(value).map_err(|_| InteractionHistoryError::Unavailable)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
