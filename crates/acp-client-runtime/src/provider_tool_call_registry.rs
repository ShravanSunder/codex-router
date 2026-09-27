//! Resolve Cursor requests that carry a tool-call ID but no Session ID.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};

#[derive(Default)]
pub(crate) struct ProviderToolCallRegistry(Mutex<RegistryState>);

#[derive(Default)]
struct RegistryState {
    running_sessions: HashSet<String>,
    tool_sessions: HashMap<String, HashSet<String>>,
}

impl ProviderToolCallRegistry {
    pub(crate) fn turn_started(&self, session_id: &str) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.running_sessions.insert(session_id.to_owned());
    }

    pub(crate) fn observe_tool_call(&self, session_id: &str, tool_call_id: &str) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .tool_sessions
            .entry(tool_call_id.to_owned())
            .or_default()
            .insert(session_id.to_owned());
    }

    pub(crate) fn resolve_session(&self, tool_call_id: &str) -> Option<String> {
        let state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(owners) = state.tool_sessions.get(tool_call_id) {
            return (owners.len() == 1)
                .then(|| owners.iter().next().cloned())
                .flatten();
        }
        (state.running_sessions.len() == 1)
            .then(|| state.running_sessions.iter().next().cloned())
            .flatten()
    }

    pub(crate) fn turn_ended(&self, session_id: &str) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.running_sessions.remove(session_id);
        state.tool_sessions.retain(|_, owners| {
            owners.remove(session_id);
            !owners.is_empty()
        });
    }
}

/// Observe tool ownership before the SDK hands a session/update to its
/// Session actor. Cursor may send a connection request immediately afterward.
pub(crate) struct ToolCallOwnershipHandler(Arc<ProviderToolCallRegistry>);

impl ToolCallOwnershipHandler {
    pub(crate) fn new(registry: Arc<ProviderToolCallRegistry>) -> Self {
        Self(registry)
    }
}

impl HandleDispatchFrom<Agent> for ToolCallOwnershipHandler {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        _connection: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, Error> {
        if let Dispatch::Notification(notification) = &message
            && notification.method() == "session/update"
        {
            let params = notification.params();
            let update = params.get("update");
            let kind = update
                .and_then(|update| update.get("sessionUpdate"))
                .and_then(serde_json::Value::as_str);
            if matches!(kind, Some("tool_call" | "tool_call_update"))
                && let (Some(session_id), Some(tool_call_id)) = (
                    params.get("sessionId").and_then(serde_json::Value::as_str),
                    update
                        .and_then(|update| update.get("toolCallId"))
                        .and_then(serde_json::Value::as_str),
                )
            {
                self.0.observe_tool_call(session_id, tool_call_id);
            }
        }
        Ok(Handled::No {
            message,
            retry: false,
        })
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ToolCallOwnershipHandler"
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderToolCallRegistry;

    /// Cursor omits sessionId. A unique observed tool call wins over the
    /// single-running-Turn fallback; concurrent unknowns remain unresolved.
    #[test]
    fn resolves_only_unique_tool_ownership_or_one_running_turn() {
        let registry = ProviderToolCallRegistry::default();
        registry.turn_started("first");
        assert_eq!(
            registry.resolve_session("unknown").as_deref(),
            Some("first")
        );
        registry.turn_started("second");
        assert_eq!(registry.resolve_session("unknown"), None);
        registry.observe_tool_call("first", "first-tool");
        registry.observe_tool_call("second", "second-tool");
        assert_eq!(
            registry.resolve_session("first-tool").as_deref(),
            Some("first")
        );
        assert_eq!(
            registry.resolve_session("second-tool").as_deref(),
            Some("second")
        );
        registry.observe_tool_call("second", "first-tool");
        assert_eq!(registry.resolve_session("first-tool"), None);
        registry.turn_ended("second");
        assert_eq!(
            registry.resolve_session("first-tool").as_deref(),
            Some("first")
        );
        assert_eq!(
            registry.resolve_session("second-tool").as_deref(),
            Some("first")
        );
        registry.turn_ended("first");
        assert_eq!(registry.resolve_session("first-tool"), None);
    }
}
