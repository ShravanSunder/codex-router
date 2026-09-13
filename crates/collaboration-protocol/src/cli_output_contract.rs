//! CLI envelopes; operation payloads retain their Control/native/ACP contracts.
use crate::{CodexGeneration, NonEmptyText, SessionRef};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "FiniteCommandRecord")]
pub enum FiniteCommandRecord<TResult, TError> {
    Result { result: TResult },
    Error { error: TError },
}

#[derive(JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NativeObservationRecord {
    ListenerReady {
        target: SessionRef,
        generation: CodexGeneration,
    },
    NativeMessage {
        target: SessionRef,
        generation: CodexGeneration,
        message: Map<String, Value>,
    },
    ConnectionClosed {
        reason: ObservationCloseReason,
    },
}
#[derive(JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObservationCloseReason {
    ObservationTimeout,
    CallerCancelled,
    NativeConnectionLost,
}

#[derive(JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConversationRecord {
    SessionReady {
        target: SessionRef,
    },
    SessionUpdate {
        target: SessionRef,
        #[schemars(schema_with = "acp_update_schema")]
        update: Value,
    },
    PermissionRequired {
        target: SessionRef,
    },
    PromptResult {
        target: SessionRef,
        #[schemars(schema_with = "acp_result_schema")]
        result: Value,
    },
    ConversationError {
        target: Option<SessionRef>,
        stage: ConversationStage,
        effect: ConversationEffect,
        message: NonEmptyText,
    },
}
#[derive(JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationStage {
    Connect,
    Initialize,
    New,
    Load,
    Prompt,
    Cancel,
}
#[derive(JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationEffect {
    NotDispatched,
    Unknown,
}

fn acp_update_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"$ref":"urn:agent-communication:acp-session-update"})
}
fn acp_result_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"$ref":"urn:agent-communication:acp-prompt-response"})
}

/// Keep upstream definitions authoritative; namespace references without copying a
/// hand-maintained approximation of ACP updates or prompt responses.
pub(crate) fn bind_acp_schemas(schema: &mut Value) -> Result<(), serde_json::Error> {
    let upstream: Value = serde_json::from_slice(crate::ACP_SCHEMA_BYTES)?;
    let Some(definitions) = upstream.get("$defs").and_then(Value::as_object) else {
        return Ok(());
    };
    rewrite_references(schema, false);
    if let Some(root) = schema.as_object_mut() {
        let target = root
            .entry("$defs")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(target) = target.as_object_mut() {
            for (name, definition) in definitions {
                let mut definition = definition.clone();
                rewrite_references(&mut definition, true);
                target.insert(format!("Acp{name}"), definition);
            }
        }
    }
    Ok(())
}
fn rewrite_references(value: &mut Value, upstream: bool) {
    match value {
        Value::Object(fields) => {
            if let Some(Value::String(reference)) = fields.get_mut("$ref") {
                if upstream && let Some(name) = reference.strip_prefix("#/$defs/") {
                    *reference = format!("#/$defs/Acp{name}");
                } else if reference.as_str() == "urn:agent-communication:acp-session-update" {
                    *reference = "#/$defs/AcpSessionUpdate".to_owned();
                } else if reference.as_str() == "urn:agent-communication:acp-prompt-response" {
                    *reference = "#/$defs/AcpPromptResponse".to_owned();
                }
            }
            for child in fields.values_mut() {
                rewrite_references(child, upstream);
            }
        }
        Value::Array(items) => {
            for child in items {
                rewrite_references(child, upstream);
            }
        }
        _ => {}
    }
}
