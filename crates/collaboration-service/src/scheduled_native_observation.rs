//! Exact history through the supported native thread read; schema export alone is not runtime support.
use crate::NativeAdmission;
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use collaboration_protocol::SessionRef;
use serde_json::{Value, json};
pub(crate) struct ObservedTurn {
    pub turn: Value,
    pub model: Option<String>,
    pub effort: Option<String>,
}
pub(crate) async fn read_turn(
    admission: &NativeAdmission,
    target: &SessionRef,
    turn_id: &str,
) -> Result<Option<Value>, NativeConnectionError> {
    read_turn_and_choice(admission, target, turn_id)
        .await
        .map(|observed| observed.map(|observed| observed.turn))
}
pub(crate) async fn read_turn_and_choice(
    admission: &NativeAdmission,
    target: &SessionRef,
    turn_id: &str,
) -> Result<Option<ObservedTurn>, NativeConnectionError> {
    let schemas = admission
        .schemas()
        .ok_or(NativeConnectionError::InvalidInput)?;
    let mut connection = NativeProtocolConnection::connect(admission.backend_path()).await?;
    let thread_id = String::from(target.session_id.clone());
    // Fresh accepted turns can briefly reject history until native metadata is materialized.
    // The caller retains the Run and retries observation; this read never resubmits input.
    // The native carrier bounds the complete response; oversized history remains an explicit failure.
    let result = connection
        .request_validated(
            &schemas,
            NativeOperation::ReadThread,
            json!({"threadId":thread_id,"includeTurns":true}),
        )
        .await?;
    if result.pointer("/thread/id").and_then(Value::as_str) != Some(thread_id.as_str()) {
        return Err(NativeConnectionError::Protocol);
    }
    let thread = result
        .get("thread")
        .ok_or(NativeConnectionError::Protocol)?;
    let model = thread
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let effort = thread
        .get("reasoningEffort")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .ok_or(NativeConnectionError::Protocol)?;
    let mut matched = turns
        .iter()
        .filter(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id));
    let result = matched.next().cloned();
    if matched.next().is_some() {
        return Err(NativeConnectionError::Protocol);
    }
    Ok(result.map(|turn| ObservedTurn {
        turn,
        model,
        effort,
    }))
}
