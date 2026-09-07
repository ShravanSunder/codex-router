//! Explicit native read/interruption operations; no implicit thread loading or replay.
use crate::{NativeConnectionError, NativeProtocolConnection};
use serde_json::{Value, json};

impl NativeProtocolConnection {
    /// Explicitly attaches/loads an existing native thread without injecting configuration.
    /// Returns the complete effective response; schema/cwd validation belongs to its caller.
    pub async fn resume_thread(&mut self, thread_id: &str) -> Result<Value, NativeConnectionError> {
        validate_id(thread_id)?;
        let result = self
            .request("thread/resume", json!({"threadId":thread_id}))
            .await?;
        if result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            != Some(thread_id)
        {
            return Err(NativeConnectionError::OutcomeUnknown);
        }
        Ok(result)
    }

    /// Reads native metadata without attaching or requesting full conversation content.
    /// The caller's admitted schema profile owns full response validation.
    pub async fn inspect_thread(
        &mut self,
        thread_id: &str,
    ) -> Result<Value, NativeConnectionError> {
        validate_id(thread_id)?;
        let result = self
            .request(
                "thread/read",
                json!({"threadId":thread_id,"includeTurns":false}),
            )
            .await?;
        let thread = result
            .get("thread")
            .filter(|value| value.is_object())
            .ok_or(NativeConnectionError::Protocol)?;
        if thread.get("id").and_then(Value::as_str) != Some(thread_id) {
            return Err(NativeConnectionError::Protocol);
        }
        Ok(thread.clone())
    }
    /// Waits for the native interruption response. Empty startup sentinel is forbidden here.
    pub async fn interrupt_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<(), NativeConnectionError> {
        validate_id(thread_id)?;
        validate_id(turn_id)?;
        let result = self
            .request(
                "turn/interrupt",
                json!({"threadId":thread_id,"turnId":turn_id}),
            )
            .await?;
        if result != json!({}) {
            return Err(NativeConnectionError::OutcomeUnknown);
        }
        Ok(())
    }
}
fn validate_id(value: &str) -> Result<(), NativeConnectionError> {
    if value.is_empty() || value.len() > 4096 || value.contains('\0') {
        Err(NativeConnectionError::InvalidInput)
    } else {
        Ok(())
    }
}
