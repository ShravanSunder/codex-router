//! Router access selection and truthful native-settings evidence.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouterAccess {
    WriteRestricted,
    WorkspaceWrite,
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsObservationSource {
    ThreadStart,
    ThreadFork,
    ThreadResume,
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsUnavailableReason {
    NativeResponseOmittedSettings,
    /// The broker holds no access route for this thread, so the app-server
    /// default governs its access. Threads created before routes were recorded
    /// still resume; their access is inherited rather than Router-selected.
    NoRecordedAccessRoute,
    NotObservedBeforeResume,
    ThreadReadOmitsSettings,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SettingsObservation {
    Observed {
        source: SettingsObservationSource,
        observed_at: String,
        router_access: Option<RouterAccess>,
        native_sandbox: Option<serde_json::Value>,
        permission_profile: Option<serde_json::Value>,
        approval_policy: Box<Option<serde_json::Value>>,
        approvals_reviewer: Box<Option<serde_json::Value>>,
    },
    Unavailable {
        reason: SettingsUnavailableReason,
    },
}
