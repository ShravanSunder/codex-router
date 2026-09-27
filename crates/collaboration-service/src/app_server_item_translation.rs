//! Pure projection of provider Session Items into Codex app-server v2 shapes.
use message_board::Identity;
use message_board::SessionRef;
use serde_json::{Value, json};
use session_event_model::{
    ApprovalEffect, ApprovalRequest, ApprovalScope, ApprovalSubject, OfferedOptionId,
    OptionsOrigin, QuestionField, QuestionRequest,
};
use session_event_model::{SessionItem, SessionItemKind, StopReason, ToolCallStatus, TurnOutcome};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub struct TranslatedSessionItem {
    pub thread_item: Value,
    pub notification: Option<Value>,
}

/// Produces only ThreadItem tags defined by Codex app-server protocol v2.
/// Source: openai/codex e72da2b538, v2/item.rs:236-429.
pub fn translate_session_item(
    item: &SessionItem,
    thread_id: &str,
    turn_id: &str,
) -> TranslatedSessionItem {
    let item_id = &item.item_id;
    let text = item.text.as_deref().unwrap_or("");
    let (thread_item, notification) = match &item.kind {
        SessionItemKind::UserMessage => (
            json!({"type":"userMessage","id":item_id,
                "content":[{"type":"text","text":text}]}),
            None,
        ),
        SessionItemKind::AgentMessage => (
            json!({"type":"agentMessage","id":item_id,"text":text}),
            None,
        ),
        SessionItemKind::AgentThought => (
            json!({"type":"reasoning","id":item_id,
                "summary":if text.is_empty() { Vec::new() } else { vec![text] },
                "content":[]}),
            None,
        ),
        SessionItemKind::ToolCall { tool_kind, status } => {
            let status = match status {
                ToolCallStatus::Pending | ToolCallStatus::InProgress => "inProgress",
                ToolCallStatus::Completed => "completed",
                ToolCallStatus::Failed => "failed",
            };
            let result = if status == "completed" && !text.is_empty() {
                Some(json!({"content":[{"type":"text","text":text}],
                    "structuredContent":null,"_meta":null}))
            } else {
                None
            };
            let error = if status == "failed" {
                Some(json!({"message":if text.is_empty() { "Tool call failed" } else { text }}))
            } else {
                None
            };
            (
                json!({"type":"mcpToolCall","id":item_id,
                    "server":"provider","tool":tool_kind,
                    "status":status,"arguments":{},
                    "result":result,"error":error}),
                None,
            )
        }
        SessionItemKind::Plan => (
            json!({"type":"plan","id":item_id,"text":text}),
            Some(json!({"method":"turn/plan/updated","params":{
                "threadId":thread_id,"turnId":turn_id,
                "explanation":text,"plan":[]
            }})),
        ),
        other => {
            let visible_text = item.text.clone().unwrap_or_else(|| match other {
                SessionItemKind::Unknown { source_kind } => {
                    format!("Provider item: {source_kind}")
                }
                _ => format!("Provider item: {other:?}"),
            });
            (
                json!({"type":"agentMessage","id":item_id,"text":visible_text}),
                None,
            )
        }
    };
    TranslatedSessionItem {
        thread_item,
        notification,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoricalTurn {
    pub turn_id: String,
    pub items: Vec<SessionItem>,
    pub outcome: TurnOutcome,
}

/// Replay has no prompt results and cannot reveal whether an Input was steered.
/// Each user message starts a historical Turn. Preceding output is retained in
/// the first Turn, or in one synthetic Turn when replay has no user message.
pub fn group_historical_turns(session: &SessionRef, replay: &[SessionItem]) -> Vec<HistoricalTurn> {
    let mut groups: Vec<Vec<SessionItem>> = Vec::new();
    let mut leading_items = Vec::new();
    for item in replay {
        if matches!(item.kind, SessionItemKind::UserMessage) {
            groups.push(vec![item.clone()]);
        } else if let Some(group) = groups.last_mut() {
            group.push(item.clone());
        } else {
            leading_items.push(item.clone());
        }
    }
    if !leading_items.is_empty() {
        if let Some(first_group) = groups.first_mut() {
            leading_items.append(first_group);
            *first_group = leading_items;
        } else {
            groups.push(leading_items);
        }
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(index, items)| {
            // Groups are opened by an Item above; this label also keeps the
            // identifier stable if that construction changes later.
            let first_item_id = items
                .first()
                .map_or("empty-replay-group", |item| item.item_id.as_str());
            let name = format!(
                "codex-router:replay:{}:{}:{}:{}:{}",
                session.endpoint.service_id.as_str(),
                session.endpoint.endpoint_id.as_str(),
                session.session_id.as_str(),
                index,
                first_item_id
            );
            HistoricalTurn {
                turn_id: Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes())
                    .hyphenated()
                    .to_string(),
                items,
                outcome: TurnOutcome::Ended {
                    stop_reason: StopReason::Unknown("replayed".into()),
                    local_cause: None,
                },
            }
        })
        .collect()
}

pub fn render_historical_turn(turn: &HistoricalTurn) -> Value {
    let items = turn
        .items
        .iter()
        .map(|item| translate_session_item(item, "", &turn.turn_id).thread_item)
        .collect::<Vec<_>>();
    json!({
        "id":turn.turn_id,"items":items,"itemsView":"full",
        "status":"completed","error":null,
        "startedAt":null,"completedAt":null,"durationMs":null
    })
}

#[derive(Clone, Copy)]
pub struct InteractionDisplayContext<'a> {
    pub thread_id: &'a str,
    pub turn_id: &'a str,
    pub started_at_ms: i64,
}

pub enum ApprovalPresentation {
    Interactive {
        method: String,
        params: Value,
        decision_options: BTreeMap<String, OfferedOptionId>,
        form_option_ids: BTreeSet<OfferedOptionId>,
    },
    ReadOnly {
        summary: Value,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalReply {
    Selected(OfferedOptionId),
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InteractionReplyError {
    #[error("this actor may observe but not decide the interaction")]
    ReadOnly,
    #[error("the interaction reply is invalid")]
    InvalidReply,
}

fn native_command_decision(effect: ApprovalEffect, scope: &ApprovalScope) -> Option<&'static str> {
    match (effect, scope) {
        (ApprovalEffect::Allow, ApprovalScope::Once) => Some("accept"),
        (ApprovalEffect::Allow, ApprovalScope::Session) => Some("acceptForSession"),
        (ApprovalEffect::Decline, ApprovalScope::Once) => Some("decline"),
        _ => None,
    }
}

fn approval_item_id(request: &ApprovalRequest) -> &str {
    match &request.subject {
        Some(ApprovalSubject::ToolCall { tool_call }) => &tool_call.tool_call_id,
        Some(ApprovalSubject::Plan { tool_call_id, .. }) => tool_call_id,
        _ => &request.request_id,
    }
}

fn approval_form_message(request: &ApprovalRequest) -> String {
    let mut message = request.title.clone();
    if let Some(description) = &request.description {
        message.push('\n');
        message.push_str(description);
    }
    if request.options_origin == OptionsOrigin::RouterSynthesized {
        message.push_str("\nRouter synthesized these choices; the agent did not offer them.");
    }
    if let Some(ApprovalSubject::Plan { plan_item_id, .. }) = &request.subject {
        message.push_str("\nPlan item: ");
        message.push_str(plan_item_id);
    }
    message
}

/// Exact native prompts are used only if their decision controls represent
/// every offered agent option once. Otherwise the form retains all option IDs.
/// Oracle: Codex v2/item.rs:66-124,1549-1652 and v2/mcp.rs:645-670.
pub fn translate_approval_request(
    request: &ApprovalRequest,
    actor: &Identity,
    approver: &Identity,
    context: InteractionDisplayContext<'_>,
) -> ApprovalPresentation {
    if actor != approver {
        return ApprovalPresentation::ReadOnly {
            summary: json!({
                "type":"agentMessage","id":request.request_id,
                "text":format!("Approval pending: {}", request.title)
            }),
        };
    }

    let tool_kind = match &request.subject {
        Some(ApprovalSubject::ToolCall { tool_call }) => Some(tool_call.kind.as_str()),
        _ => None,
    };
    let command_kind = matches!(request.subject, Some(ApprovalSubject::Command { .. }))
        || tool_kind == Some("execute");
    let file_kind = matches!(tool_kind, Some("edit" | "delete" | "move"));
    let mut decisions = BTreeMap::new();
    let exact_command = request.options.iter().all(|option| {
        native_command_decision(option.choice.effect, &option.choice.scope).is_some_and(
            |decision| {
                decisions
                    .insert(decision.into(), option.option_id.clone())
                    .is_none()
            },
        )
    });
    if command_kind && exact_command && request.options_origin == OptionsOrigin::AgentOffered {
        let mut available_decisions = request
            .options
            .iter()
            .filter_map(|option| {
                native_command_decision(option.choice.effect, &option.choice.scope)
            })
            .collect::<Vec<_>>();
        available_decisions.push("cancel");
        let (command, cwd) = match &request.subject {
            Some(ApprovalSubject::Command { command, cwd }) => {
                (Some(command.as_str()), Some(cwd.as_str()))
            }
            _ => (None, None),
        };
        let params = json!({
            "threadId":context.thread_id,"turnId":context.turn_id,
            "itemId":approval_item_id(request),"startedAtMs":context.started_at_ms,
            "reason":request.description.as_deref().unwrap_or(&request.title),
            "availableDecisions":available_decisions,
            "command":command,"cwd":cwd
        });
        return ApprovalPresentation::Interactive {
            method: "item/commandExecution/requestApproval".into(),
            params,
            decision_options: decisions,
            form_option_ids: BTreeSet::new(),
        };
    }

    let offered = request.options.iter().collect::<Vec<_>>();
    let file_decisions = offered
        .iter()
        .map(|option| native_command_decision(option.choice.effect, &option.choice.scope))
        .collect::<Vec<_>>();
    if request.options_origin == OptionsOrigin::AgentOffered
        && file_kind
        && offered.len() == 2
        && file_decisions.contains(&Some("accept"))
        && file_decisions.contains(&Some("acceptForSession"))
    {
        let decision_options = offered
            .iter()
            .zip(file_decisions)
            .filter_map(|(option, decision)| {
                decision.map(|decision| (decision.into(), option.option_id.clone()))
            })
            .collect();
        return ApprovalPresentation::Interactive {
            method: "item/fileChange/requestApproval".into(),
            params: json!({
                "threadId":context.thread_id,"turnId":context.turn_id,
                "itemId":approval_item_id(request),"startedAtMs":context.started_at_ms,
                "reason":request.description.as_deref().unwrap_or(&request.title)
            }),
            decision_options,
            form_option_ids: BTreeSet::new(),
        };
    }

    let choices = request
        .options
        .iter()
        .map(|option| {
            let title = match &option.choice.scope {
                ApprovalScope::Persistent { where_stored } => {
                    format!("{} — persists in {}", option.label, where_stored.as_str())
                }
                _ => option.label.clone(),
            };
            json!({"const":option.option_id.as_str(),"title":title})
        })
        .collect::<Vec<_>>();
    let form_option_ids = request
        .options
        .iter()
        .map(|option| option.option_id.clone())
        .collect();
    ApprovalPresentation::Interactive {
        method: "mcpServer/elicitation/request".into(),
        params: json!({
            "threadId":context.thread_id,"turnId":context.turn_id,
            "serverName":"codex-router","mode":"form",
            "message":approval_form_message(request),
            "requestedSchema":{"type":"object","properties":{
                "choice":{"type":"string","title":request.title,"oneOf":choices}
            },"required":["choice"]}
        }),
        decision_options: BTreeMap::new(),
        form_option_ids,
    }
}

pub fn map_approval_reply(
    presentation: &ApprovalPresentation,
    reply: &Value,
) -> Result<ApprovalReply, InteractionReplyError> {
    let ApprovalPresentation::Interactive {
        method,
        decision_options,
        form_option_ids,
        ..
    } = presentation
    else {
        return Err(InteractionReplyError::ReadOnly);
    };
    if method == "mcpServer/elicitation/request" {
        return match reply.get("action").and_then(Value::as_str) {
            Some("decline" | "cancel") => Ok(ApprovalReply::Cancelled),
            Some("accept") => {
                let choice = reply
                    .pointer("/content/choice")
                    .and_then(Value::as_str)
                    .ok_or(InteractionReplyError::InvalidReply)?;
                let option_id = OfferedOptionId::new(choice)
                    .map_err(|_| InteractionReplyError::InvalidReply)?;
                if form_option_ids.contains(&option_id) {
                    Ok(ApprovalReply::Selected(option_id))
                } else {
                    Err(InteractionReplyError::InvalidReply)
                }
            }
            _ => Err(InteractionReplyError::InvalidReply),
        };
    }
    match reply.get("decision").and_then(Value::as_str) {
        Some("cancel") => Ok(ApprovalReply::Cancelled),
        Some(decision) => decision_options
            .get(decision)
            .cloned()
            .map(ApprovalReply::Selected)
            .ok_or(InteractionReplyError::InvalidReply),
        None => Err(InteractionReplyError::InvalidReply),
    }
}

pub enum QuestionPresentation {
    InteractiveForm { params: Value },
    ReadOnly { summary: Value },
}

#[derive(Clone, Debug, PartialEq)]
pub enum QuestionReply {
    Answered(Value),
    Declined,
    Cancelled,
}

/// Typed Question fields become an MCP elicitation form without losing their
/// field labels or required markers. Source: Codex v2/mcp.rs:429-590,776-826.
pub fn translate_question_request(
    request: &QuestionRequest,
    actor: &Identity,
    approver: &Identity,
    context: InteractionDisplayContext<'_>,
) -> QuestionPresentation {
    if actor != approver {
        return QuestionPresentation::ReadOnly {
            summary: json!({"type":"agentMessage","id":request.request_id,
                "text":format!("Question pending: {}",request.prompt)}),
        };
    }
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for field in request.fields.iter() {
        let (field_id, is_required, schema) = match field {
            QuestionField::Text {
                field_id,
                label,
                description,
                required,
            } => (
                field_id,
                required,
                json!({"type":"string","title":label,"description":description}),
            ),
            QuestionField::Number {
                field_id,
                label,
                description,
                required,
            } => (
                field_id,
                required,
                json!({"type":"number","title":label,"description":description}),
            ),
            QuestionField::Boolean {
                field_id,
                label,
                description,
                required,
            } => (
                field_id,
                required,
                json!({"type":"boolean","title":label,"description":description}),
            ),
            QuestionField::SingleChoice {
                field_id,
                label,
                description,
                required,
                options,
            } => (
                field_id,
                required,
                json!({"type":"string","title":label,"description":description,
                    "oneOf":options.iter().map(|option|
                        json!({"const":option.option_id,"title":option.label})).collect::<Vec<_>>()}),
            ),
            QuestionField::MultiChoice {
                field_id,
                label,
                description,
                required,
                options,
                min,
                max,
            } => (
                field_id,
                required,
                json!({"type":"array","title":label,"description":description,
                    "uniqueItems":true,"minItems":min,"maxItems":max,
                    "items":{"type":"string","oneOf":options.iter().map(|option|
                        json!({"const":option.option_id,"title":option.label})).collect::<Vec<_>>()}}),
            ),
        };
        properties.insert(field_id.clone(), schema);
        if *is_required {
            required.push(field_id.clone());
        }
    }
    QuestionPresentation::InteractiveForm {
        params: json!({
            "threadId":context.thread_id,"turnId":context.turn_id,
            "serverName":"codex-router","mode":"form",
            "message":request.prompt,
            "requestedSchema":{"type":"object","properties":properties,"required":required}
        }),
    }
}

pub fn map_question_form_reply(reply: &Value) -> Result<QuestionReply, InteractionReplyError> {
    match reply.get("action").and_then(Value::as_str) {
        Some("accept") => reply
            .get("content")
            .filter(|content| content.is_object())
            .cloned()
            .map(QuestionReply::Answered)
            .ok_or(InteractionReplyError::InvalidReply),
        Some("decline") => Ok(QuestionReply::Declined),
        Some("cancel") => Ok(QuestionReply::Cancelled),
        _ => Err(InteractionReplyError::InvalidReply),
    }
}

/// Reverse mapping for a simple text/single-choice requestUserInput prompt.
/// An empty submitted answer is skip/decline; Turn interruption is cancel.
pub fn map_request_user_input_reply(
    reply: &Value,
    interrupted: bool,
) -> Result<QuestionReply, InteractionReplyError> {
    if interrupted {
        return Ok(QuestionReply::Cancelled);
    }
    let answers = reply
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(InteractionReplyError::InvalidReply)?;
    let mut content = serde_json::Map::new();
    for (field_id, answer) in answers {
        let values = answer
            .get("answers")
            .and_then(Value::as_array)
            .ok_or(InteractionReplyError::InvalidReply)?;
        if values.len() > 1 {
            return Err(InteractionReplyError::InvalidReply);
        }
        if let Some(value) = values.first() {
            let value = value.as_str().ok_or(InteractionReplyError::InvalidReply)?;
            content.insert(field_id.clone(), json!(value));
        }
    }
    if content.is_empty() {
        Ok(QuestionReply::Declined)
    } else {
        Ok(QuestionReply::Answered(Value::Object(content)))
    }
}
