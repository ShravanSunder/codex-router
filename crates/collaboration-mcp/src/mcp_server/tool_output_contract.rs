//! The advertised MCP output includes the structured error path of every tool.
use rmcp::schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};

/// Success keeps the existing structured content; an error adds one discriminator.
#[derive(JsonSchema, Serialize)]
#[serde(untagged)]
pub(super) enum McpToolOutput<TSuccess> {
    Success(TSuccess),
    Error(Box<McpToolError>),
}

#[derive(JsonSchema, Serialize)]
#[serde(tag = "mcpResult", rename_all = "camelCase")]
pub(super) enum McpToolError {
    Error {
        kind: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        stage: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effect: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<Value>,
        #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
        operation_id: Option<Value>,
        #[serde(flatten)]
        domain_fields: Map<String, Value>,
    },
}

impl McpToolError {
    /// Preserve existing domain fields while making every structured error schema-valid.
    pub(super) fn from_existing(value: Value) -> Self {
        let mut fields = match value {
            Value::Object(fields) => fields,
            value => {
                let mut fields = Map::new();
                fields.insert("data".to_owned(), value);
                fields
            }
        };
        fields.remove("mcpResult");
        let kind = fields
            .remove("kind")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "protocolViolation".to_owned());
        let message = fields
            .remove("message")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "MCP tool error could not be projected".to_owned());
        let stage = take_string(&mut fields, "stage");
        let effect = take_string(&mut fields, "effect");
        let target = fields.remove("target");
        let operation_id = fields.remove("operationId");
        Self::Error {
            kind,
            message,
            stage,
            effect,
            target,
            operation_id,
            domain_fields: fields,
        }
    }
}

fn take_string(fields: &mut Map<String, Value>, key: &str) -> Option<String> {
    match fields.get(key) {
        Some(Value::String(_)) => fields
            .remove(key)
            .and_then(|value| value.as_str().map(str::to_owned)),
        _ => None,
    }
}
