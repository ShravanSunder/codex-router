//! Pure projection of provider Session Items into Codex app-server v2 shapes.
use message_board::SessionRef;
use serde_json::{Value, json};
use session_event_model::{SessionItem, SessionItemKind, StopReason, ToolCallStatus, TurnOutcome};
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
