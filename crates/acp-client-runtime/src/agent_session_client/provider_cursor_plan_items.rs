//! Keep Cursor plan and todo updates on one stable Session Item.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use serde::Deserialize;
use session_event_model::{SessionEvent, SessionItem, SessionItemKind};

use crate::{SessionEventSink, provider_connection_activity::ProviderConnectionActivity};

#[derive(Default)]
pub(crate) struct CursorPlanItems(Mutex<HashMap<(String, String), CursorPlanRecord>>);

#[derive(Default)]
struct CursorPlanRecord {
    name: Option<String>,
    overview: Option<String>,
    markdown: Option<String>,
    todos: Vec<CursorTodo>,
}

pub(super) struct CursorPlanDefinition {
    pub(super) tool_call_id: String,
    pub(super) name: Option<String>,
    pub(super) overview: Option<String>,
    pub(super) markdown: String,
    pub(super) todos: Vec<CursorTodo>,
}

impl CursorPlanItems {
    pub(crate) fn forget_session(&self, session_id: &str) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(owner, _), _| owner != session_id);
    }

    pub(super) fn set_plan(
        &self,
        session_id: &str,
        plan: CursorPlanDefinition,
    ) -> (bool, SessionItem) {
        let mut plans = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (session_id.to_owned(), plan.tool_call_id);
        let existed = plans.contains_key(&key);
        let record = plans.entry(key.clone()).or_default();
        record.name = plan.name;
        record.overview = plan.overview;
        record.markdown = Some(plan.markdown);
        record.todos = plan.todos;
        (existed, record.item(&key.1))
    }

    fn update(&self, session_id: &str, request: CursorTodoUpdate) -> (bool, SessionItem) {
        let mut plans = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (session_id.to_owned(), request.tool_call_id);
        let existed = plans.contains_key(&key);
        let record = plans.entry(key.clone()).or_default();
        if !request.merge {
            record.todos.clear();
        }
        for update in request.todos {
            if let Some(existing) = record.todos.iter_mut().find(|todo| todo.id == update.id) {
                *existing = update;
            } else {
                record.todos.push(update);
            }
        }
        (existed, record.item(&key.1))
    }
}

impl CursorPlanRecord {
    fn item(&self, tool_call_id: &str) -> SessionItem {
        let mut sections = Vec::new();
        if let Some(name) = &self.name {
            sections.push(format!("# {name}"));
        }
        if let Some(overview) = &self.overview {
            sections.push(overview.clone());
        }
        if let Some(markdown) = &self.markdown {
            sections.push(markdown.clone());
        }
        if !self.todos.is_empty() {
            sections.push(
                self.todos
                    .iter()
                    .map(|todo| format!("- {}: {}", todo.status.label(), todo.content))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        SessionItem {
            item_id: format!("plan:{tool_call_id}"),
            kind: SessionItemKind::Plan,
            text: Some(sections.join("\n\n")),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorTodoUpdate {
    tool_call_id: String,
    todos: Vec<CursorTodo>,
    #[serde(default)]
    merge: bool,
}

#[derive(Clone, Deserialize)]
pub(super) struct CursorTodo {
    pub(super) id: String,
    pub(super) content: String,
    pub(super) status: CursorTodoStatus,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CursorTodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl CursorTodoStatus {
    const fn label(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

pub(super) struct ProviderCursorTodoHandler {
    tool_registry: Arc<ProviderConnectionActivity>,
    todo_state: Arc<CursorPlanItems>,
    event_sink: Arc<dyn SessionEventSink>,
}

impl ProviderCursorTodoHandler {
    pub(super) fn new(
        tool_registry: Arc<ProviderConnectionActivity>,
        todo_state: Arc<CursorPlanItems>,
        event_sink: Arc<dyn SessionEventSink>,
    ) -> Self {
        Self {
            tool_registry,
            todo_state,
            event_sink,
        }
    }

    fn apply_update(&self, params: serde_json::Value) {
        let Ok(update) = serde_json::from_value::<CursorTodoUpdate>(params) else {
            tracing::warn!("malformed Cursor todo update");
            return;
        };
        let Some(session_id) = self.tool_registry.resolve_session(&update.tool_call_id) else {
            tracing::warn!("unresolved Cursor todo update");
            return;
        };
        let (existed, item) = self.todo_state.update(&session_id, update);
        let event = if existed {
            SessionEvent::ItemUpdated { item }
        } else {
            SessionEvent::ItemStarted { item }
        };
        if self.event_sink.publish(&session_id, event).is_err() {
            tracing::error!("provider Session event sink overflow on Cursor todo update");
        }
    }
}

impl HandleDispatchFrom<Agent> for ProviderCursorTodoHandler {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        _connection: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, Error> {
        match message {
            Dispatch::Request(request, responder) if request.method() == "cursor/update_todos" => {
                self.apply_update(request.params().clone());
                responder.respond(serde_json::json!({}))?;
                Ok(Handled::Yes)
            }
            Dispatch::Notification(notification)
                if notification.method() == "cursor/update_todos" =>
            {
                self.apply_update(notification.params().clone());
                Ok(Handled::Yes)
            }
            message => Ok(Handled::No {
                message,
                retry: false,
            }),
        }
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ProviderCursorTodoHandler"
    }
}

#[cfg(test)]
mod tests {
    use super::{CursorPlanItems, CursorTodo, CursorTodoStatus, CursorTodoUpdate};

    #[test]
    fn merge_updates_existing_todos_and_preserves_the_plan_item_id() {
        let state = CursorPlanItems::default();
        let first = state.update(
            "session",
            CursorTodoUpdate {
                tool_call_id: "tool".to_owned(),
                todos: vec![CursorTodo {
                    id: "a".to_owned(),
                    content: "First".to_owned(),
                    status: CursorTodoStatus::Pending,
                }],
                merge: false,
            },
        );
        let second = state.update(
            "session",
            CursorTodoUpdate {
                tool_call_id: "tool".to_owned(),
                todos: vec![
                    CursorTodo {
                        id: "a".to_owned(),
                        content: "First revised".to_owned(),
                        status: CursorTodoStatus::Completed,
                    },
                    CursorTodo {
                        id: "b".to_owned(),
                        content: "Second".to_owned(),
                        status: CursorTodoStatus::InProgress,
                    },
                ],
                merge: true,
            },
        );
        assert!(!first.0);
        assert!(second.0);
        assert_eq!(first.1.item_id, second.1.item_id);
        assert_eq!(
            second.1.text.as_deref(),
            Some("- completed: First revised\n- in progress: Second")
        );
        state.forget_session("session");
        let after_end = state.update(
            "session",
            CursorTodoUpdate {
                tool_call_id: "tool".to_owned(),
                todos: Vec::new(),
                merge: true,
            },
        );
        assert!(!after_end.0);
    }
}
