//! Translate Cursor's tool-scoped todo updates into Session plan Items.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};
use serde::Deserialize;
use session_event_model::{SessionEvent, SessionItem, SessionItemKind};

use crate::{SessionEventSink, provider_tool_call_registry::ProviderToolCallRegistry};

#[derive(Default)]
pub(crate) struct CursorTodoState(Mutex<HashMap<(String, String), Vec<CursorTodo>>>);

impl CursorTodoState {
    pub(crate) fn forget_session(&self, session_id: &str) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(owner, _), _| owner != session_id);
    }

    fn update(&self, session_id: &str, request: CursorTodoUpdate) -> (bool, SessionItem) {
        let mut plans = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (session_id.to_owned(), request.tool_call_id);
        let existed = plans.contains_key(&key);
        let todos = plans.entry(key.clone()).or_default();
        if !request.merge {
            todos.clear();
        }
        for update in request.todos {
            if let Some(existing) = todos.iter_mut().find(|todo| todo.id == update.id) {
                *existing = update;
            } else {
                todos.push(update);
            }
        }
        let text = todos
            .iter()
            .map(|todo| format!("- {}: {}", todo.status.label(), todo.content))
            .collect::<Vec<_>>()
            .join("\n");
        (
            existed,
            SessionItem {
                item_id: format!("plan:{}", key.1),
                kind: SessionItemKind::Plan,
                text: Some(text),
            },
        )
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

#[derive(Deserialize)]
struct CursorTodo {
    id: String,
    content: String,
    status: CursorTodoStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CursorTodoStatus {
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
    tool_registry: Arc<ProviderToolCallRegistry>,
    todo_state: Arc<CursorTodoState>,
    event_sink: Arc<dyn SessionEventSink>,
}

impl ProviderCursorTodoHandler {
    pub(super) fn new(
        tool_registry: Arc<ProviderToolCallRegistry>,
        todo_state: Arc<CursorTodoState>,
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
    use super::{CursorTodo, CursorTodoState, CursorTodoStatus, CursorTodoUpdate};

    #[test]
    fn merge_updates_existing_todos_and_preserves_the_plan_item_id() {
        let state = CursorTodoState::default();
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
