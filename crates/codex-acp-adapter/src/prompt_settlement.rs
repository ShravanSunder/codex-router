//! ACP terminal selection is distinct from evidence that native work was interrupted.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeInterruptionState {
    Confirmed,
    NotDispatched,
    Rejected,
    Unknown,
}
#[derive(Clone, Copy, Debug)]
pub enum NativePromptTerminal {
    Completed,
    Interrupted,
    Failed,
    GenerationLost,
}
pub struct PromptSettlement {
    request_id: Value,
    dispatched: bool,
    turn_id: Option<String>,
    cancelled: bool,
    settled: bool,
    unresolved_cancellation: bool,
}
impl PromptSettlement {
    pub fn new(request_id: Value) -> Result<Self, &'static str> {
        if !(request_id.is_null() || request_id.is_string() || request_id.as_i64().is_some()) {
            return Err("invalid ACP request ID");
        }
        Ok(Self {
            request_id,
            dispatched: false,
            turn_id: None,
            cancelled: false,
            settled: false,
            unresolved_cancellation: false,
        })
    }
    pub fn mark_dispatched(&mut self) -> Result<(), &'static str> {
        if self.settled || self.cancelled || self.dispatched {
            return Err("prompt cannot dispatch");
        }
        self.dispatched = true;
        Ok(())
    }
    pub fn accepted_turn(&mut self, turn_id: String) -> Result<(), &'static str> {
        if self.settled || !self.dispatched || turn_id.is_empty() || self.turn_id.is_some() {
            return Err("invalid accepted turn");
        }
        self.turn_id = Some(turn_id);
        Ok(())
    }
    /// Returns the exact interruption target, if acceptance already identified it.
    pub fn request_cancel(&mut self) -> Option<&str> {
        if self.settled {
            return None;
        }
        self.cancelled = true;
        self.unresolved_cancellation = self.dispatched;
        self.turn_id.as_deref()
    }
    pub fn settle_cancel(&mut self, observed: NativeInterruptionState) -> Option<Value> {
        if self.settled || !self.cancelled {
            return None;
        }
        let state = if !self.dispatched {
            NativeInterruptionState::NotDispatched
        } else if self.turn_id.is_none() || observed == NativeInterruptionState::NotDispatched {
            NativeInterruptionState::Unknown
        } else {
            observed
        };
        self.unresolved_cancellation = !matches!(
            state,
            NativeInterruptionState::Confirmed | NativeInterruptionState::NotDispatched
        );
        self.settled = true;
        Some(
            json!({"jsonrpc":"2.0","id":self.request_id,"result":{"stopReason":"cancelled","_meta":{"codex-router/nativeInterruption":{"state":state}}}}),
        )
    }
    /// Ignore unrelated turns. A settled prompt never emits a second response.
    pub fn observe_terminal(
        &mut self,
        turn_id: Option<&str>,
        terminal: NativePromptTerminal,
    ) -> Option<Value> {
        if !matches!(terminal, NativePromptTerminal::GenerationLost)
            && (turn_id.is_none() || turn_id != self.turn_id.as_deref())
        {
            return None;
        }
        if !matches!(terminal, NativePromptTerminal::GenerationLost) {
            self.unresolved_cancellation = false;
        }
        if self.settled {
            return None;
        }
        if self.cancelled {
            return self.settle_cancel(
                if matches!(terminal, NativePromptTerminal::GenerationLost) {
                    NativeInterruptionState::Unknown
                } else {
                    NativeInterruptionState::Confirmed
                },
            );
        }
        self.settled = true;
        Some(match terminal {
            NativePromptTerminal::Completed => {
                json!({"jsonrpc":"2.0","id":self.request_id,"result":{"stopReason":"end_turn"}})
            }
            _ => {
                json!({"jsonrpc":"2.0","id":self.request_id,"error":{"code":-32603,"message":"Native prompt did not complete successfully"}})
            }
        })
    }
    #[must_use]
    pub fn blocks_next_prompt(&self) -> bool {
        !self.settled || self.unresolved_cancellation
    }
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.settled
    }
    #[must_use]
    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }
}
