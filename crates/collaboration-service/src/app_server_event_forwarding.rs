//! Live provider event and interaction projection for the Codex app-server face.
use crate::router_session_app_server::thread_alias;
use crate::{
    ApprovalPresentation, ApprovalReply, HubEvent, InteractionDisplayContext, QuestionPresentation,
    QuestionReply, ServiceApprovalBroker, map_approval_reply, map_question_form_reply,
    translate_approval_request, translate_question_request, translate_session_item,
};
use collaboration_protocol::QuestionResponse;
use message_board::{Identity, SessionRef};
use serde_json::{Value, json};
use session_event_model::{
    ApprovalRequest, PendingInteraction, SessionEvent, StopReason, TurnOutcome,
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
    broker: Option<Arc<ServiceApprovalBroker>>,
    turn_ids: HashMap<SessionRef, String>,
    items: HashMap<(SessionRef, String), Value>,
    pending: HashMap<String, PendingAppServerInteraction>,
}

impl AppServerEventForwarding {
    pub(crate) fn new(actor: Identity, broker: Option<Arc<ServiceApprovalBroker>>) -> Self {
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
                render_turn_event(session, event).into_iter().collect()
            }
            SessionEvent::ItemStarted { item } | SessionEvent::ItemUpdated { item } => {
                let turn_id = self.turn_ids.get(session).map(String::as_str).unwrap_or("");
                let translated = translate_session_item(item, &thread_id, turn_id);
                self.items.insert(
                    (session.clone(), item.item_id.clone()),
                    translated.thread_item.clone(),
                );
                let method = if matches!(event.event, SessionEvent::ItemStarted { .. }) {
                    "item/started"
                } else {
                    "item/updated"
                };
                let mut frames = vec![json!({"method":method,"params":{
                    "threadId":thread_id,"turnId":turn_id,"item":translated.thread_item
                }})];
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
                    "threadId":thread_id,"turnId":self.turn_ids.get(session),"item":item
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
