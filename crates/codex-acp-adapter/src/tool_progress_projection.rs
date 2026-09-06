//! Native tool lifecycle and full-plan snapshots become schema-checked ACP updates.
use crate::{AcpSchemaCatalog, PromptTarget};
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
#[error("invalid tool or plan projection")]
pub struct ToolProjectionError;

pub fn project_tool_progress(
    catalog: &mut AcpSchemaCatalog,
    target: PromptTarget<'_>,
    message: &Value,
) -> Result<Option<Value>, ToolProjectionError> {
    let method = message.get("method").and_then(Value::as_str);
    if !matches!(
        method,
        Some("item/started" | "item/completed" | "turn/plan/updated")
    ) {
        return Ok(None);
    }
    let params = message.get("params").ok_or(ToolProjectionError)?;
    if params.get("threadId").and_then(Value::as_str) != Some(target.session_id)
        || params.get("turnId").and_then(Value::as_str) != Some(target.turn_id)
    {
        return Ok(None);
    }
    let update = if method == Some("turn/plan/updated") {
        let plan = params
            .get("plan")
            .and_then(Value::as_array)
            .ok_or(ToolProjectionError)?;
        let mut entries = Vec::with_capacity(plan.len());
        for step in plan {
            let status = match step.get("status").and_then(Value::as_str) {
                Some("pending") => "pending",
                Some("inProgress") => "in_progress",
                Some("completed") => "completed",
                _ => return Err(ToolProjectionError),
            };
            // Native has no priority field; ACP requires one. Use the neutral priority.
            entries.push(json!({"content":step.get("step").and_then(Value::as_str).ok_or(ToolProjectionError)?,"priority":"medium","status":status}));
        }
        json!({"sessionUpdate":"plan","entries":entries})
    } else {
        let item = params.get("item").ok_or(ToolProjectionError)?;
        let (kind, title, content, locations) = match item.get("type").and_then(Value::as_str) {
            Some("commandExecution") => {
                let command = item
                    .get("command")
                    .and_then(Value::as_str)
                    .ok_or(ToolProjectionError)?;
                let content = item
                    .get("aggregatedOutput")
                    .and_then(Value::as_str)
                    .map(|text| vec![text_content(text)])
                    .unwrap_or_default();
                ("execute", command, content, Vec::new())
            }
            Some("fileChange") => {
                let mut content = Vec::new();
                let mut locations = Vec::new();
                for change in item
                    .get("changes")
                    .and_then(Value::as_array)
                    .ok_or(ToolProjectionError)?
                {
                    let path = change
                        .get("path")
                        .and_then(Value::as_str)
                        .ok_or(ToolProjectionError)?;
                    let diff = change
                        .get("diff")
                        .and_then(Value::as_str)
                        .ok_or(ToolProjectionError)?;
                    content.push(text_content(&format!("{path}\n{diff}")));
                    locations.push(json!({"path":path}));
                }
                ("edit", "Apply file changes", content, locations)
            }
            _ => return Ok(None),
        };
        let status = match item.get("status").and_then(Value::as_str) {
            Some("inProgress") => "in_progress",
            Some("completed") => "completed",
            Some("failed" | "declined") => "failed",
            _ => return Err(ToolProjectionError),
        };
        json!({"sessionUpdate":if method==Some("item/started") {"tool_call"} else {"tool_call_update"},"toolCallId":item.get("id").and_then(Value::as_str).ok_or(ToolProjectionError)?,"kind":kind,"title":title,"status":status,"content":content,"locations":locations,"rawOutput":item})
    };
    let params = json!({"sessionId":target.session_id,"update":update});
    if !catalog
        .validate("SessionNotification", &params)
        .map_err(|_| ToolProjectionError)?
    {
        return Err(ToolProjectionError);
    }
    Ok(Some(
        json!({"jsonrpc":"2.0","method":"session/update","params":params}),
    ))
}
fn text_content(text: &str) -> Value {
    json!({"type":"content","content":{"type":"text","text":text}})
}
