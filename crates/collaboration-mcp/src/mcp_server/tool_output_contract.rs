//! The advertised MCP output includes the structured error path of every tool.
use rmcp::schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};

/// Success keeps the existing structured content; an error adds one discriminator.
#[derive(JsonSchema, Serialize)]
#[serde(untagged)]
#[schemars(description = "A successful tool result or a typed structured MCP tool error.")]
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
            .and_then(|value| value.as_str().map(str::to_owned));
        debug_assert!(
            kind.is_some(),
            "MCP structured error is missing string kind"
        );
        let kind = kind.unwrap_or_else(|| "protocolViolation".to_owned());
        let message = fields
            .remove("message")
            .and_then(|value| value.as_str().map(str::to_owned));
        debug_assert!(
            message.is_some(),
            "MCP structured error is missing string message"
        );
        let message = message.unwrap_or_else(|| "MCP tool error could not be projected".to_owned());
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
    fields
        .remove(key)
        .and_then(|value| value.as_str().map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_optional_stage_and_effect_do_not_escape_the_typed_error() {
        for invalid in [Value::Null, serde_json::json!({"unexpected": true})] {
            let error = McpToolOutput::<Value>::Error(Box::new(McpToolError::from_existing(
                serde_json::json!({
                    "kind": "protocolViolation",
                    "message": "fixture failure",
                    "stage": invalid,
                    "effect": invalid,
                }),
            )));
            let value = serde_json::to_value(error).expect("serialized error");
            assert!(
                value.get("stage").is_none(),
                "invalid stage must be dropped"
            );
            assert!(
                value.get("effect").is_none(),
                "invalid effect must be dropped"
            );
        }
    }

    #[test]
    fn missing_required_fields_and_non_object_input_are_detected_in_debug() {
        for invalid in [
            serde_json::json!({"message":"missing kind"}),
            serde_json::json!({"kind":"invalidRequest"}),
            serde_json::json!("non-object"),
        ] {
            let projection = std::panic::catch_unwind(|| McpToolError::from_existing(invalid));
            if cfg!(debug_assertions) {
                assert!(
                    projection.is_err(),
                    "missing typed field must trip debug assertion"
                );
            } else {
                let value = serde_json::to_value(McpToolOutput::<Value>::Error(Box::new(
                    projection.expect("release fallback"),
                )))
                .expect("serialized fallback");
                assert!(value["kind"].is_string());
                assert!(value["message"].is_string());
                assert_eq!(value["mcpResult"], "error");
            }
        }
    }
}
