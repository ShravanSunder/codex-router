//! Live provider event and interaction projection for the Codex app-server face.
use crate::router_session_app_server::thread_alias;
use crate::{
    ApprovalPresentation, ApprovalReply, HubEvent, InteractionDisplayContext, QuestionPresentation,
    QuestionReply, ServiceInteractionBroker, group_historical_turns, map_approval_reply,
    map_question_form_reply, render_historical_turn, translate_approval_request,
    translate_question_request, translate_session_item,
};
use collaboration_protocol::QuestionResponse;
use message_board::{Identity, SessionRef};
use serde_json::{Value, json};
use session_event_model::{
    ApprovalRequest, PendingInteraction, SessionEvent, SessionItem, StopReason, TurnOutcome,
};
use std::{collections::HashMap, sync::Arc};

fn render_turn_event(session: &SessionRef, event: &HubEvent) -> Option<Value> {
    let thread_id = thread_alias(session);
    let (method, turn_id, status) = match &event.event {
        SessionEvent::TurnStarted { turn_id, .. } => ("turn/started", turn_id, "inProgress"),
        SessionEvent::TurnEnded { turn_id, outcome } => {
            let status = match outcome {
                TurnOutcome::Ended {
                    stop_reason: StopReason::Cancelled,
                    ..
                } => "interrupted",
                TurnOutcome::Ended { .. } => "completed",
                TurnOutcome::Lost { .. } => "failed",
            };
            ("turn/completed", turn_id, status)
        }
        _ => return None,
    };
    Some(json!({
        "method":method,
        "params":{
            "threadId":thread_id,
            "turn":{
                "id":turn_id,"items":[],"status":status,"error":null,
                "startedAt":null,"completedAt":null,"durationMs":null
            }
        }
    }))
}

pub(crate) fn historical_turns(session: &SessionRef, snapshot: &[HubEvent]) -> Vec<Value> {
    let mut items = Vec::<SessionItem>::new();
    let mut item_positions = HashMap::<String, usize>::new();
    let mut active_turn: Option<String> = None;
    let mut active_item_start = 0;
    for event in snapshot {
        match &event.event {
            SessionEvent::TurnStarted { turn_id, .. } => {
                active_turn = Some(turn_id.clone());
                active_item_start = items.len();
            }
            SessionEvent::TurnEnded { turn_id, .. } if active_turn.as_deref() == Some(turn_id) => {
                active_turn = None;
            }
            SessionEvent::ItemStarted { item } | SessionEvent::ItemUpdated { item } => {
                if let Some(index) = item_positions.get(&item.item_id).copied() {
                    if let Some(existing) = items.get_mut(index) {
                        *existing = item.clone();
                    }
                } else {
                    item_positions.insert(item.item_id.clone(), items.len());
                    items.push(item.clone());
                }
            }
            _ => {}
        }
    }
    if active_turn.is_some() {
        items.truncate(active_item_start);
    }
    group_historical_turns(session, &items)
        .iter()
        .map(render_historical_turn)
        .collect()
}

enum PendingAppServerInteraction {
    Approval {
        request: Box<ApprovalRequest>,
        presentation: ApprovalPresentation,
    },
    Question {
        request_id: String,
        presentation: QuestionPresentation,
    },
}

pub(crate) struct AppServerEventForwarding {
    actor: Identity,
    broker: Option<Arc<ServiceInteractionBroker>>,
    turn_ids: HashMap<SessionRef, String>,
    items: HashMap<(SessionRef, String), Value>,
    pending: HashMap<String, PendingAppServerInteraction>,
}

impl AppServerEventForwarding {
    pub(crate) fn new(actor: Identity, broker: Option<Arc<ServiceInteractionBroker>>) -> Self {
        Self {
            actor,
            broker,
            turn_ids: HashMap::new(),
            items: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    pub(crate) fn project(&mut self, session: &SessionRef, event: &HubEvent) -> Vec<Value> {
        let thread_id = thread_alias(session);
        match &event.event {
            SessionEvent::TurnStarted { turn_id, .. } => {
                self.turn_ids.insert(session.clone(), turn_id.clone());
                render_turn_event(session, event).into_iter().collect()
            }
            SessionEvent::TurnEnded { .. } => {
                self.turn_ids.remove(session);
                self.items
                    .retain(|(item_session, _), _| item_session != session);
                render_turn_event(session, event).into_iter().collect()
            }
            SessionEvent::ItemStarted { item } | SessionEvent::ItemUpdated { item } => {
                let turn_id = self.turn_ids.get(session).map(String::as_str).unwrap_or("");
                let translated = translate_session_item(item, &thread_id, turn_id);
                let previous = self.items.insert(
                    (session.clone(), item.item_id.clone()),
                    translated.thread_item.clone(),
                );
                let mut frames = if matches!(event.event, SessionEvent::ItemStarted { .. }) {
                    vec![json!({"method":"item/started","params":{
                        "threadId":thread_id,"turnId":turn_id,"item":translated.thread_item,
                        "startedAtMs":chrono::Utc::now().timestamp_millis()
                    }})]
                } else {
                    match &item.kind {
                        session_event_model::SessionItemKind::AgentMessage => {
                            let old_text = previous
                                .as_ref()
                                .and_then(|value| value.get("text"))
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let new_text = item.text.as_deref().unwrap_or("");
                            new_text
                                .strip_prefix(old_text)
                                .filter(|delta| !delta.is_empty())
                                .map(|delta| {
                                    vec![json!({"method":"item/agentMessage/delta","params":{
                                        "threadId":thread_id,"turnId":turn_id,"itemId":item.item_id,
                                        "delta":delta
                                    }})]
                                })
                                .unwrap_or_default()
                        }
                        session_event_model::SessionItemKind::ToolCall { .. } => item
                            .text
                            .as_deref()
                            .filter(|message| !message.is_empty())
                            .map(|message| {
                                vec![json!({"method":"item/mcpToolCall/progress","params":{
                                    "threadId":thread_id,"turnId":turn_id,"itemId":item.item_id,
                                    "message":message
                                }})]
                            })
                            .unwrap_or_default(),
                        _ => Vec::new(),
                    }
                };
                if let Some(notification) = translated.notification {
                    frames.push(notification);
                }
                frames
            }
            SessionEvent::ItemCompleted { item_id } => {
                let Some(item) = self.items.remove(&(session.clone(), item_id.clone())) else {
                    return Vec::new();
                };
                vec![json!({"method":"item/completed","params":{
                    "threadId":thread_id,"turnId":self.turn_ids.get(session),"item":item,
                    "completedAtMs":chrono::Utc::now().timestamp_millis()
                }})]
            }
            SessionEvent::InteractionRequested { interaction } => {
                self.present_interaction(session, interaction, &thread_id)
            }
            _ => Vec::new(),
        }
    }

    fn present_interaction(
        &mut self,
        session: &SessionRef,
        interaction: &PendingInteraction,
        thread_id: &str,
    ) -> Vec<Value> {
        let turn_id = self.turn_ids.get(session).map(String::as_str).unwrap_or("");
        let context = InteractionDisplayContext {
            thread_id,
            turn_id,
            started_at_ms: chrono::Utc::now().timestamp_millis(),
        };
        let may_decide = self.broker.is_some() && interaction.approver() == &self.actor;
        if !may_decide {
            let (item_id, text) = match interaction {
                PendingInteraction::Approval { request, .. } => (
                    &request.request_id,
                    format!("Approval pending: {}", request.title),
                ),
                PendingInteraction::Question { request, .. } => (
                    &request.request_id,
                    format!("Question pending: {}", request.prompt),
                ),
            };
            return vec![json!({"method":"item/started","params":{
                "threadId":thread_id,"turnId":turn_id,
                "item":{"type":"agentMessage","id":item_id,"text":text}
            }})];
        }
        let actor = &self.actor;
        match interaction {
            PendingInteraction::Approval { request, approver } => {
                let presentation = translate_approval_request(request, actor, approver, context);
                match presentation {
                    ApprovalPresentation::Interactive {
                        ref method,
                        ref params,
                        ..
                    } => {
                        let id = format!(
                            "router:approval:{}:{}",
                            session.session_id.as_str(),
                            request.request_id
                        );
                        let frame = json!({"id":id,"method":method,"params":params});
                        self.pending.insert(
                            id,
                            PendingAppServerInteraction::Approval {
                                request: request.clone(),
                                presentation,
                            },
                        );
                        vec![frame]
                    }
                    ApprovalPresentation::ReadOnly { summary } => {
                        vec![json!({"method":"item/started","params":{
                            "threadId":thread_id,"turnId":turn_id,"item":summary
                        }})]
                    }
                }
            }
            PendingInteraction::Question { request, approver } => {
                let presentation = translate_question_request(request, actor, approver, context);
                match presentation {
                    QuestionPresentation::InteractiveForm { ref params } => {
                        let id = format!(
                            "router:question:{}:{}",
                            session.session_id.as_str(),
                            request.request_id
                        );
                        let frame = json!({"id":id,"method":"mcpServer/elicitation/request","params":params});
                        self.pending.insert(
                            id,
                            PendingAppServerInteraction::Question {
                                request_id: request.request_id.clone(),
                                presentation,
                            },
                        );
                        vec![frame]
                    }
                    QuestionPresentation::ReadOnly { summary } => {
                        vec![json!({"method":"item/started","params":{
                            "threadId":thread_id,"turnId":turn_id,"item":summary
                        }})]
                    }
                }
            }
        }
    }

    pub(crate) async fn resolve_reply(&mut self, reply: &Value) {
        let Some(id) = reply.get("id").and_then(Value::as_str) else {
            return;
        };
        let Some(pending) = self.pending.remove(id) else {
            return;
        };
        let Some(broker) = &self.broker else {
            return;
        };
        match pending {
            PendingAppServerInteraction::Approval {
                request,
                presentation,
            } => {
                let Ok(ApprovalReply::Selected(option_id)) =
                    map_approval_reply(&presentation, reply.get("result").unwrap_or(&Value::Null))
                else {
                    return;
                };
                let acknowledge_persistent = request.options.iter().any(|option| {
                    option.option_id == option_id
                        && matches!(
                            option.choice.scope,
                            session_event_model::ApprovalScope::Persistent { .. }
                        )
                });
                let _decision = broker
                    .decide_typed_interaction(
                        &request.request_id,
                        &self.actor,
                        option_id.as_str(),
                        acknowledge_persistent,
                        None,
                    )
                    .await;
            }
            PendingAppServerInteraction::Question {
                request_id,
                presentation,
            } => {
                if !matches!(presentation, QuestionPresentation::InteractiveForm { .. }) {
                    return;
                }
                let Ok(mapped) =
                    map_question_form_reply(reply.get("result").unwrap_or(&Value::Null))
                else {
                    return;
                };
                let response = match mapped {
                    QuestionReply::Answered(content) => match serde_json::from_value(content) {
                        Ok(content) => QuestionResponse::Answered { content },
                        Err(_) => return,
                    },
                    QuestionReply::Declined => QuestionResponse::Declined,
                    QuestionReply::Cancelled => QuestionResponse::Cancelled,
                };
                let _decision = broker
                    .respond_question(&request_id, &self.actor, response)
                    .await;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod notification_wire_tests {
    use super::*;
    use session_event_model::SessionItemKind;

    /// Oracle: Codex 0.157.1 app-server-protocol v2/item.rs:1327-1336,
    /// 1405-1414,1427-1433 and protocol/common.rs:1936-1946.
    #[test]
    fn item_lifecycle_uses_typed_v2_notifications() -> Result<(), Box<dyn std::error::Error>> {
        let session: SessionRef = serde_json::from_value(json!({
            "endpoint":{"serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","endpointId":"claude-local"},
            "sessionId":"provider-1"
        }))?;
        let actor: Identity = serde_json::from_value(json!({"kind":"human","humanId":"owner"}))?;
        let mut projector = AppServerEventForwarding::new(actor, None);
        let turn = projector.project(
            &session,
            &HubEvent {
                sequence: 1,
                event: SessionEvent::TurnStarted {
                    turn_id: "turn-1".into(),
                    input_id: session_event_model::InputId::generate(),
                },
            },
        );
        assert_eq!(turn[0]["method"], "turn/started");
        let item = |text: &str| SessionItem {
            item_id: "reply-1".into(),
            kind: SessionItemKind::AgentMessage,
            text: Some(text.into()),
        };
        let started = projector.project(
            &session,
            &HubEvent {
                sequence: 2,
                event: SessionEvent::ItemStarted {
                    item: item("First"),
                },
            },
        );
        assert_eq!(started[0]["method"], "item/started");
        assert!(started[0]["params"]["startedAtMs"].is_i64());
        let updated = projector.project(
            &session,
            &HubEvent {
                sequence: 3,
                event: SessionEvent::ItemUpdated {
                    item: item("First streamed"),
                },
            },
        );
        assert_eq!(updated[0]["method"], "item/agentMessage/delta");
        assert_eq!(updated[0]["params"]["delta"], " streamed");
        let completed = projector.project(
            &session,
            &HubEvent {
                sequence: 4,
                event: SessionEvent::ItemCompleted {
                    item_id: "reply-1".into(),
                },
            },
        );
        assert_eq!(completed[0]["method"], "item/completed");
        assert_eq!(completed[0]["params"]["item"]["text"], "First streamed");
        assert!(completed[0]["params"]["completedAtMs"].is_i64());
        let ended = projector.project(
            &session,
            &HubEvent {
                sequence: 5,
                event: SessionEvent::TurnEnded {
                    turn_id: "turn-1".into(),
                    outcome: TurnOutcome::Ended {
                        stop_reason: StopReason::EndTurn,
                        local_cause: None,
                    },
                },
            },
        );
        assert_eq!(ended[0]["method"], "turn/completed");
        Ok(())
    }
}
