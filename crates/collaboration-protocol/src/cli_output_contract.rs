//! CLI envelopes; operation payloads retain their Control/native/ACP contracts.
use crate::{
    AdapterOperationFailure, CodexGeneration, RouterAccess, SessionRef, SettingsObservation,
};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[schemars(rename = "FiniteCommandRecord")]
pub enum FiniteCommandRecord<TResult, TError> {
    Result {
        cli_version: String,
        service_version: String,
        result: TResult,
    },
    Error {
        /// Known operation target retained when failure follows target resolution.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        target: Option<SessionRef>,
        error: TError,
    },
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
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationRecord {
    ConversationCreated {
        target: SessionRef,
    },
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
        effective_model: String,
        effective_effort: String,
        effective_access: Option<RouterAccess>,
        settings_observation: Box<SettingsObservation>,
        /// Present only when a resume asked for an effort the thread did not
        /// already carry. The turn still runs; the change is reported because
        /// it invalidates the provider's prompt cache for this session.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        effort_change: Option<EffortChange>,
        idle_seconds: u64,
        #[schemars(schema_with = "acp_result_schema")]
        result: Value,
    },
    ConversationSettlement {
        target: SessionRef,
        terminal_reason: ConversationTerminalReason,
        #[schemars(schema_with = "acp_result_schema")]
        result: Value,
    },
    ConversationError {
        target: Option<SessionRef>,
        error: AdapterOperationFailure,
    },
}

#[derive(JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationTerminalReason {
    Cancelled,
    TimedOut,
}
/// One resume's reasoning-effort change, as the caller asked and the thread held.
#[derive(JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffortChange {
    pub previous: String,
    pub requested: String,
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
