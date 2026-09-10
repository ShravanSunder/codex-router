//! Service defaults are distinct from per-schedule overrides and captured active-attempt budgets.
use crate::{OperationId, PositiveSeconds};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationConfiguration {
    pub execution_timeout_seconds: PositiveSeconds,
    pub summary_timeout_seconds: PositiveSeconds,
}
impl Default for AutomationConfiguration {
    fn default() -> Self {
        Self {
            execution_timeout_seconds: PositiveSeconds::DEFAULT_EXECUTION_TIMEOUT,
            summary_timeout_seconds: PositiveSeconds::DEFAULT_SUMMARY_TIMEOUT,
        }
    }
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationConfigureRequest {
    pub operation_id: OperationId,
    pub execution_timeout_seconds: PositiveSeconds,
    pub summary_timeout_seconds: PositiveSeconds,
}
impl AutomationConfigureRequest {
    pub fn configuration(&self) -> AutomationConfiguration {
        AutomationConfiguration {
            execution_timeout_seconds: self.execution_timeout_seconds,
            summary_timeout_seconds: self.summary_timeout_seconds,
        }
    }
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationStatus {
    pub storage_available: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub configuration: Option<AutomationConfiguration>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub earliest_retained_event_cursor: Option<String>,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationFileState {
    NotReplaced,
    Replaced,
    Unknown,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationFailure {
    pub kind: ConfigurationFailureKind,
    pub message: String,
    pub operation_id: Option<OperationId>,
    pub file_state: ConfigurationFileState,
    pub next_action: ConfigurationNextAction,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationFailureKind {
    InvalidField,
    OperationConflict,
    AutomationUnavailable,
    OutcomeUnknown,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationNextAction {
    CorrectRequest,
    InspectOperation,
    RetryLater,
}
