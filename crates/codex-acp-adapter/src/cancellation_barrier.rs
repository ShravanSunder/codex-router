//! Unconfirmed native cancellation remains a gate across explicit session reloads.
use serde_json::Value;

#[derive(Clone, Debug)]
pub enum CancellationBarrier {
    UnknownTurn,
    KnownTurn(String),
}
impl CancellationBarrier {
    /// Consumes only the thread snapshot from a schema-validated native resume result.
    #[must_use]
    pub fn resolved_by_resume(&self, response: &Value) -> bool {
        let Some(thread) = response.get("thread") else {
            return false;
        };
        let Some(turns) = thread.get("turns").and_then(Value::as_array) else {
            return false;
        };
        if let Self::KnownTurn(target) = self
            && let Some(turn) = turns
                .iter()
                .find(|turn| turn.get("id").and_then(Value::as_str) == Some(target.as_str()))
        {
            return matches!(
                turn.get("status").and_then(Value::as_str),
                Some("completed" | "interrupted" | "failed")
            );
        }
        // An unknown target cannot be cleared by another turn's completion.
        thread
            .get("status")
            .and_then(|status| status.get("type"))
            .and_then(Value::as_str)
            == Some("idle")
            && turns.iter().all(|turn| {
                matches!(
                    turn.get("status").and_then(Value::as_str),
                    Some("completed" | "interrupted" | "failed")
                )
            })
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reload_clears_only_established_terminal_or_idle_work() {
        // Arrange: native rejected the cancellation of one known turn.
        let known = CancellationBarrier::KnownTurn("target".to_owned());
        let active = json!({"thread":{"status":{"type":"active"},"turns":[{"id":"target","status":"inProgress"},{"id":"other","status":"completed"}]}});
        let terminal = json!({"thread":{"status":{"type":"active"},"turns":[{"id":"target","status":"interrupted"},{"id":"other","status":"inProgress"}]}});
        let idle = json!({"thread":{"status":{"type":"idle"},"turns":[]}});
        // Act / Assert: unrelated completion never clears either uncertain target.
        assert!(!known.resolved_by_resume(&active));
        assert!(!CancellationBarrier::UnknownTurn.resolved_by_resume(&active));
        assert!(known.resolved_by_resume(&terminal));
        assert!(!CancellationBarrier::UnknownTurn.resolved_by_resume(&terminal));
        assert!(known.resolved_by_resume(&idle));
        assert!(CancellationBarrier::UnknownTurn.resolved_by_resume(&idle));
        assert!(!known.resolved_by_resume(&json!({"thread":{"status":{"type":"idle"}}})));
    }
}
