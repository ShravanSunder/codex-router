//! Provider Session inventory is distinct from Codex's native thread catalog.

use crate::{
    EndpointRef, NativeSessionScope, NativeSessionSource, NativeSessionView, ObservationTimestamp,
    ProviderWorkingDirectory, SessionRef,
};
use message_board::Identity;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn all_sources() -> NativeSessionSource {
    NativeSessionSource::All
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionListParams {
    pub endpoint: EndpointRef,
    pub view: NativeSessionView,
    pub scope: NativeSessionScope,
    #[serde(default = "all_sources")]
    pub source: NativeSessionSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 1024))]
    pub cursor: Option<String>,
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderSessionState {
    Unloaded,
    Idle,
    Running,
    RequiresAction,
    AuthenticationRequired,
    Closed,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionSummary {
    pub target: SessionRef,
    pub working_directory: ProviderWorkingDirectory,
    /// Unix seconds; Router has no durable creation timestamp for these rows.
    pub updated_at: i64,
    pub state: ProviderSessionState,
    pub approver: Identity,
    pub created_by: SessionRef,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionListResult {
    pub endpoint: EndpointRef,
    pub observed_at: ObservationTimestamp,
    #[schemars(length(max = 100))]
    pub sessions: Vec<ProviderSessionSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 1024))]
    pub next_cursor: Option<String>,
}
