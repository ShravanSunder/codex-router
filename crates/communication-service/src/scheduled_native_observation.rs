//! Observe the exact native turn through paginated history; never infer completion from idle status alone.
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
    let mut cursor: Option<String> = None;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..100 {
        let result=connection.request_validated(&schemas,NativeOperation::ListTurns,json!({"threadId":String::from(target.session_id.clone()),"cursor":cursor,"limit":100,"sortDirection":"desc","itemsView":"full"})).await?;
        let turns = result
            .get("data")
            .and_then(Value::as_array)
            .ok_or(NativeConnectionError::Protocol)?;
        if let Some(turn) = turns
            .iter()
            .find(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))
        {
            return Ok(Some(turn.clone()));
        }
        let next = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if next.is_none() {
            return Ok(None);
        }
        if let Some(next) = &next
            && !seen.insert(next.clone())
        {
            return Err(NativeConnectionError::Protocol);
        }
        cursor = next;
    }
    Err(NativeConnectionError::Protocol)
}
