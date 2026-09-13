use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionDiagnostic {
    pub kind: PermissionDiagnosticKind,
    pub stage: PermissionDiagnosticStage,
    pub message: String,
    pub next_action: PermissionDiagnosticNextAction,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionDiagnosticKind {
    PermissionDenied,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionDiagnosticStage {
    ManifestRead,
    DirectoryResolve,
    SocketResolve,
    SocketConnect,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionDiagnosticNextAction {
    RequestApproval,
}
