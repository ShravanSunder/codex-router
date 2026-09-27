//! Track active provider Turns and resolve tool-scoped connection requests.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use agent_client_protocol::{Agent, ConnectionTo, Dispatch, Error, HandleDispatchFrom, Handled};

#[derive(Default)]
pub(crate) struct ProviderConnectionActivity(Mutex<RegistryState>);

#[derive(Default)]
struct RegistryState {
    running_sessions: HashMap<String, String>,
    tool_sessions: HashMap<String, HashSet<String>>,
    output_overflows: HashSet<String>,
}

impl ProviderConnectionActivity {
    pub(crate) fn turn_started(&self, session_id: &str, turn_id: &str) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .running_sessions
            .insert(session_id.to_owned(), turn_id.to_owned());
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
            .then(|| state.running_sessions.keys().next().cloned())
            .flatten()
    }

    pub(crate) fn turn_ended(&self, session_id: &str) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.running_sessions.remove(session_id);
        state.output_overflows.remove(session_id);
        state.tool_sessions.retain(|_, owners| {
            owners.remove(session_id);
            !owners.is_empty()
        });
    }

    pub(crate) fn claim_turn_end(&self, session_id: &str, turn_id: &str) -> bool {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.running_sessions.get(session_id).map(String::as_str) != Some(turn_id) {
            return false;
        }
        state.running_sessions.remove(session_id);
        state.output_overflows.remove(session_id);
        state.tool_sessions.retain(|_, owners| {
            owners.remove(session_id);
            !owners.is_empty()
        });
        true
    }

    pub(crate) fn drain_running_turns(&self) -> Vec<(String, String)> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.tool_sessions.clear();
        state.output_overflows.clear();
        state.running_sessions.drain().collect()
    }

    pub(crate) fn output_overflowed(&self, session_id: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .output_overflows
            .contains(session_id)
    }

    /// Connection-level callbacks do not pass through the prompt observer.
    /// Record their overflow so its actor can preserve the local cause when
    /// the agent eventually reports the Turn's stop reason.
    pub(crate) fn mark_output_overflow(&self, session_id: &str) -> Option<bool> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .running_sessions
            .contains_key(session_id)
            .then(|| state.output_overflows.insert(session_id.to_owned()))
    }
}

/// Observe tool ownership before the SDK hands a session/update to its
/// Session actor. Cursor may send a connection request immediately afterward.
pub(crate) struct ToolCallOwnershipHandler(Arc<ProviderConnectionActivity>);

impl ToolCallOwnershipHandler {
    pub(crate) fn new(registry: Arc<ProviderConnectionActivity>) -> Self {
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
    use super::ProviderConnectionActivity;

    /// Cursor omits sessionId. A unique observed tool call wins over the
    /// single-running-Turn fallback; concurrent unknowns remain unresolved.
    #[test]
    fn resolves_only_unique_tool_ownership_or_one_running_turn() {
        let registry = ProviderConnectionActivity::default();
        registry.turn_started("first", "turn-first");
        assert_eq!(
            registry.resolve_session("unknown").as_deref(),
            Some("first")
        );
        registry.turn_started("second", "turn-second");
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

    #[test]
    fn connection_retirement_claims_only_unsettled_turns() {
        let registry = ProviderConnectionActivity::default();
        registry.turn_started("settled", "turn-one");
        registry.turn_started("lost", "turn-two");
        assert!(registry.claim_turn_end("settled", "turn-one"));
        assert!(!registry.claim_turn_end("settled", "turn-one"));
        assert_eq!(
            registry.drain_running_turns(),
            vec![("lost".to_owned(), "turn-two".to_owned())]
        );
        assert!(registry.drain_running_turns().is_empty());
    }
}
