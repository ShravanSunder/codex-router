//! Stored and runtime discovery are distinct observations of one endpoint-scoped session.
use crate::{
    CodexGeneration, EndpointRef, NativeThreadStatus, NonEmptyText, ObservationTimestamp,
    SessionRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(JsonSchema, Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeSessionView {
    Stored,
    Loaded,
    Active,
}
#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeSessionListParams {
    pub endpoint: EndpointRef,
    pub view: NativeSessionView,
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 1024))]
    pub cursor: Option<String>,
}
#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NativeSessionObservation {
    #[serde(rename_all = "camelCase")]
    Stored { updated_at: ObservationTimestamp },
    #[serde(rename_all = "camelCase")]
    Runtime {
        #[schemars(schema_with = "crate::native_schema_references::status_schema")]
        status: NativeThreadStatus,
        turn_id: Option<String>,
    },
}
#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeSessionSummary {
    pub target: SessionRef,
    pub title: String,
    pub working_directory: NonEmptyText,
    pub observation: NativeSessionObservation,
}
#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeSessionListResult {
    pub endpoint: EndpointRef,
    pub observed_at: ObservationTimestamp,
    pub generation: Option<CodexGeneration>,
    #[schemars(length(max = 100))]
    pub sessions: Vec<NativeSessionSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 1024))]
    pub next_cursor: Option<String>,
}
