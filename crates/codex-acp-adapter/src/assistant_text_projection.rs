//! Project only the selected native thread/turn's assistant text into ACP updates.
use crate::{AcpSchemaCatalog, AcpSchemaError};
use serde_json::{Value, json};

pub struct PromptTarget<'a> {
    pub session_id: &'a str,
    pub turn_id: &'a str,
}
#[derive(Debug, thiserror::Error)]
pub enum TextProjectionError {
    #[error("malformed native assistant text event")]
    InvalidNativeEvent,
    #[error("projected assistant text violates pinned ACP schema")]
    InvalidAcpUpdate,
    #[error(transparent)]
    Schema(#[from] AcpSchemaError),
}
/// Native frames must first pass native schema admission; this helper checks target correlation.
pub fn project_assistant_text(
    catalog: &mut AcpSchemaCatalog,
    target: PromptTarget<'_>,
    message: &Value,
) -> Result<Option<Value>, TextProjectionError> {
    if message.get("method").and_then(Value::as_str) != Some("item/agentMessage/delta") {
        return Ok(None);
    }
    let params = message
        .get("params")
        .ok_or(TextProjectionError::InvalidNativeEvent)?;
    let thread = params
        .get("threadId")
        .and_then(Value::as_str)
        .ok_or(TextProjectionError::InvalidNativeEvent)?;
    let turn = params
        .get("turnId")
        .and_then(Value::as_str)
        .ok_or(TextProjectionError::InvalidNativeEvent)?;
    if thread != target.session_id || turn != target.turn_id {
        return Ok(None);
    }
    let delta = params
        .get("delta")
        .and_then(Value::as_str)
        .ok_or(TextProjectionError::InvalidNativeEvent)?;
    let params = json!({"sessionId":target.session_id,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":delta}}});
    if !catalog.validate("SessionNotification", &params)? {
        return Err(TextProjectionError::InvalidAcpUpdate);
    }
    Ok(Some(
        json!({"jsonrpc":"2.0","method":"session/update","params":params}),
    ))
}
