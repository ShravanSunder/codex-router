//! Stored and runtime discovery are distinct observations of one endpoint-scoped session.
use crate::{
    CodexGeneration, EndpointRef, NativeThreadStatus, NonEmptyText, ObservationTimestamp,
    SessionRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(JsonSchema, Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeSessionView {
    Stored,
    Loaded,
    Active,
}
#[derive(JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NativeSessionScope {
    Any,
    Cwd {
        path: PathBuf,
    },
    Checkout {
        root: PathBuf,
    },
    Repo {
        live_roots: Vec<PathBuf>,
        normalized_origin: Option<String>,
        basename: String,
        fallback_cwd: Option<PathBuf>,
    },
}
#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeSessionSource {
    Interactive,
    Subagents,
    All,
}
#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeSessionListParams {
    pub endpoint: EndpointRef,
    pub view: NativeSessionView,
    pub scope: NativeSessionScope,
    pub source: NativeSessionSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
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
    pub name: Option<String>,
    pub title: String,
    pub source: NativeSessionSource,
    pub working_directory: NonEmptyText,
    pub observation: NativeSessionObservation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub git_branch: Option<String>,
    pub idle_seconds: u64,
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
