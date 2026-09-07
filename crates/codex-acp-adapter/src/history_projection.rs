//! Ordered supported transcript projection; native history remains authoritative.
use crate::AcpSchemaCatalog;
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum HistoryProjectionError {
    #[error("invalid native history response")]
    InvalidHistory,
    #[error("ACP history buffer capacity exceeded")]
    Capacity,
    #[error("ACP history schema validation failed")]
    Schema,
}
/// Validated native history enters here after exact session/cwd verification.
/// Omitted unsupported native item kinds are not converted into invented ACP messages.
pub fn project_history(
    catalog: &mut AcpSchemaCatalog,
    session_id: &str,
    response: &Value,
) -> Result<Vec<Value>, HistoryProjectionError> {
    let thread = response
        .get("thread")
        .ok_or(HistoryProjectionError::InvalidHistory)?;
    if thread.get("id").and_then(Value::as_str) != Some(session_id) {
        return Err(HistoryProjectionError::InvalidHistory);
    }
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .ok_or(HistoryProjectionError::InvalidHistory)?;
    let mut updates = Vec::new();
    let mut bytes = 0_usize;
    for turn in turns {
        let items = turn
            .get("items")
            .and_then(Value::as_array)
            .ok_or(HistoryProjectionError::InvalidHistory)?;
        for item in items {
            let (kind, content) = match item.get("type").and_then(Value::as_str) {
                Some("agentMessage") => (
                    "agent_message_chunk",
                    vec![
                        json!({"type":"text","text":item.get("text").and_then(Value::as_str).ok_or(HistoryProjectionError::InvalidHistory)?}),
                    ],
                ),
                Some("userMessage") => {
                    let input = item
                        .get("content")
                        .and_then(Value::as_array)
                        .ok_or(HistoryProjectionError::InvalidHistory)?;
                    let mut content = Vec::new();
                    for value in input {
                        if value.get("type").and_then(Value::as_str) == Some("text") {
                            content.push(json!({"type":"text","text":value.get("text").and_then(Value::as_str).ok_or(HistoryProjectionError::InvalidHistory)?}));
                        }
                    }
                    ("user_message_chunk", content)
                }
                _ => continue,
            };
            for content in content {
                let params = json!({"sessionId":session_id,"update":{"sessionUpdate":kind,"content":content}});
                if !catalog
                    .validate("SessionNotification", &params)
                    .map_err(|_| HistoryProjectionError::Schema)?
                {
                    return Err(HistoryProjectionError::Schema);
                }
                let update = json!({"jsonrpc":"2.0","method":"session/update","params":params});
                let size = serde_json::to_vec(&update)
                    .map_err(|_| HistoryProjectionError::Schema)?
                    .len();
                bytes = bytes
                    .checked_add(size)
                    .filter(|bytes| *bytes <= 64 * 1024 * 1024)
                    .ok_or(HistoryProjectionError::Capacity)?;
                if updates.len() >= 1024 {
                    return Err(HistoryProjectionError::Capacity);
                }
                updates.push(update);
            }
        }
    }
    Ok(updates)
}
