//! Provider Session inventory is distinct from Codex's native thread catalog.

use crate::{
    EndpointRef, NativeSessionScope, NativeSessionSource, NativeSessionView, ObservationTimestamp,
    ProviderIdentity, ProviderWorkingDirectory, SessionRef,
};
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
#[serde(untagged, rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum ProviderSessionSummary {
    HostedProvider {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<HostedProviderOrigin>,
        target: SessionRef,
        working_directory: ProviderWorkingDirectory,
        /// Unix seconds; Router has no durable creation timestamp for these rows.
        updated_at: i64,
        state: ProviderSessionState,
        approver: ProviderIdentity,
        created_by: ProviderIdentity,
    },
    ClaudeCodeInteractive {
        origin: ClaudeCodeInteractiveOrigin,
        target: SessionRef,
        name: Option<String>,
        working_directory: ProviderWorkingDirectory,
        status: ClaudeCodeInteractiveStatus,
        started_at: i64,
        updated_at: i64,
        status_updated_at: Option<i64>,
        kind: String,
        entrypoint: String,
    },
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostedProviderOrigin {
    HostedProvider,
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaudeCodeInteractiveOrigin {
    ClaudeCodeInteractive,
}

impl ProviderSessionSummary {
    pub fn target(&self) -> &SessionRef {
        match self {
            Self::HostedProvider { target, .. } | Self::ClaudeCodeInteractive { target, .. } => {
                target
            }
        }
    }
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaudeCodeInteractiveStatus {
    Busy,
    Idle,
    Waiting,
    Shell,
    Unreported,
    Other,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionListResult {
    pub endpoint: EndpointRef,
    pub observed_at: ObservationTimestamp,
    #[schemars(length(max = 100))]
    pub sessions: Vec<ProviderSessionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_records: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 1024))]
    pub next_cursor: Option<String>,
}
