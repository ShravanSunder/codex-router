//! Exact history through the supported native thread read; schema export alone is not runtime support.
use crate::NativeAdmission;
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use communication_protocol::SessionRef;
use serde_json::{Value, json};
pub(crate) async fn read_turn(
    admission: &NativeAdmission,
    target: &SessionRef,
    turn_id: &str,
) -> Result<Option<Value>, NativeConnectionError> {
    let schemas = admission
        .schemas()
        .ok_or(NativeConnectionError::InvalidInput)?;
    let mut connection = NativeProtocolConnection::connect(admission.backend_path()).await?;
    let thread_id = String::from(target.session_id.clone());
    // Codex 0.153.4 exports history-list schemas but may reject list_turns at runtime.
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
    let turns = result
        .pointer("/thread/turns")
        .and_then(Value::as_array)
        .ok_or(NativeConnectionError::Protocol)?;
    let mut matched = turns
        .iter()
        .filter(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id));
    let result = matched.next().cloned();
    if matched.next().is_some() {
        return Err(NativeConnectionError::Protocol);
    }
    Ok(result)
}
