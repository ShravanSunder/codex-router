//! ACP client presentation and broker replies for provider interactions.
use crate::{InteractionHistoryError, ServiceApprovalBroker};
use collaboration_protocol::QuestionResponse;
use message_board::{Identity, SessionRef};
use serde_json::{Map, Value, json};
use session_event_model::session_profile_codec::{
    ApprovalRequestProfileMetadata, encode_choice_metadata,
};
use session_event_model::{
    ApprovalEffect, ApprovalScope, ApprovalSubject, PendingInteraction, QuestionField,
};

pub(crate) struct OutboundInteraction {
    pub request_id: String,
    pub frame: Value,
    pub pending: PendingInteraction,
}

pub(crate) fn present_interaction(
    session: &SessionRef,
    interaction: PendingInteraction,
    actor: &Identity,
    supports_question_form: bool,
) -> Option<OutboundInteraction> {
    if interaction.approver() != actor {
        return None;
    }
    match &interaction {
        PendingInteraction::Approval { request, .. } => {
            let request_id = format!(
                "router:approval:{}:{}",
                session.session_id.as_str(),
                request.request_id
            );
            let tool_call = match &request.subject {
                Some(ApprovalSubject::ToolCall { tool_call }) => json!({
                    "toolCallId":tool_call.tool_call_id,"title":tool_call.title,"kind":tool_call.kind,
                }),
                Some(ApprovalSubject::Command { .. }) => json!({
                    "toolCallId":request.request_id,"title":request.title,"kind":"execute",
                }),
                None => {
                    json!({"toolCallId":request.request_id,"title":request.title,"kind":"other"})
                }
            };
            let options = request
                .options
                .iter()
                .map(|option| {
                    let kind = match (&option.choice.effect, &option.choice.scope) {
                        (ApprovalEffect::Allow, ApprovalScope::Once) => "allow_once",
                        (ApprovalEffect::Allow, _) => "allow_always",
                        (ApprovalEffect::Decline | ApprovalEffect::Abort, ApprovalScope::Once) => {
                            "reject_once"
                        }
                        (ApprovalEffect::Decline | ApprovalEffect::Abort, _) => "reject_always",
                    };
                    json!({"optionId":option.option_id.as_str(),"name":option.label,"kind":kind,
                    "_meta":{"sessionProfile":{"choice":encode_choice_metadata(&option.choice)}}})
                })
                .collect::<Vec<_>>();
            let frame = json!({"jsonrpc":"2.0","id":request_id,
            "method":"session/request_permission","params":{
                "sessionId":session.session_id.as_str(),"toolCall":tool_call,"options":options,
                "_meta":ApprovalRequestProfileMetadata::from_request(request)
            }});
            Some(OutboundInteraction {
                request_id,
                frame,
                pending: interaction,
            })
        }
        PendingInteraction::Question { request, .. } if supports_question_form => {
            let request_id = format!(
                "router:question:{}:{}",
                session.session_id.as_str(),
                request.request_id
            );
            let mut properties = Map::new();
            let mut required = Vec::new();
            for field in request.fields.iter() {
                let (field_id, label, description, is_required, field_type, choices) = match field {
                    QuestionField::Text {
                        field_id,
                        label,
                        description,
                        required,
                    } => (field_id, label, description, required, "string", None),
                    QuestionField::Number {
                        field_id,
                        label,
                        description,
                        required,
                    } => (field_id, label, description, required, "number", None),
                    QuestionField::Boolean {
                        field_id,
                        label,
                        description,
                        required,
                    } => (field_id, label, description, required, "boolean", None),
                    QuestionField::SingleChoice {
                        field_id,
                        label,
                        description,
                        required,
                        options,
                    } => (
                        field_id,
                        label,
                        description,
                        required,
                        "string",
                        Some(options),
                    ),
                };
                if *is_required {
                    required.push(field_id.clone());
                }
                let mut property = Map::new();
                property.insert("type".into(), json!(field_type));
                property.insert("title".into(), json!(label));
                if let Some(description) = description {
                    property.insert("description".into(), json!(description));
                }
                if let Some(choices) = choices {
                    property.insert("enum".into(), json!(choices));
                }
                properties.insert(field_id.clone(), Value::Object(property));
            }
            let frame = json!({"jsonrpc":"2.0","id":request_id,"method":"elicitation/create",
                "params":{"mode":"form","sessionId":session.session_id.as_str(),
                    "message":request.prompt,"requestedSchema":{"type":"object",
                        "properties":properties,"required":required}}});
            Some(OutboundInteraction {
                request_id,
                frame,
                pending: interaction,
            })
        }
        PendingInteraction::Question { .. } => None,
    }
}

pub(crate) async fn apply_interaction_reply(
    broker: &ServiceApprovalBroker,
    actor: &Identity,
    interaction: &PendingInteraction,
    reply: &Value,
) -> Result<(), InteractionHistoryError> {
    match interaction {
        PendingInteraction::Approval { request, .. } => {
            let option_id = reply
                .pointer("/result/outcome/optionId")
                .and_then(Value::as_str)
                .ok_or(InteractionHistoryError::InvalidOptionId)?;
            let acknowledge_persistent = reply
                .pointer("/result/_meta/sessionProfile/acknowledgePersistent")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            broker
                .decide_typed_interaction(
                    &request.request_id,
                    actor,
                    option_id,
                    acknowledge_persistent,
                )
                .await?;
            Ok(())
        }
        PendingInteraction::Question { request, .. } => {
            let result = reply
                .get("result")
                .ok_or(InteractionHistoryError::InvalidQuestion)?;
            let response = match result.get("action").and_then(Value::as_str) {
                Some("accept") => QuestionResponse::Answered {
                    content: serde_json::from_value(
                        result.get("content").cloned().unwrap_or_else(|| json!({})),
                    )
                    .map_err(|_| InteractionHistoryError::InvalidQuestion)?,
                },
                Some("decline") => QuestionResponse::Declined,
                Some("cancel") => QuestionResponse::Cancelled,
                _ => return Err(InteractionHistoryError::InvalidQuestion),
            };
            broker
                .respond_question(&request.request_id, actor, response)
                .await
        }
    }
}
