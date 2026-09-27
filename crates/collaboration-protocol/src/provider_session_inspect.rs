//! One provider Session's state and last advertised capabilities/settings.

use crate::{ProviderSessionState, ProviderSettingName, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use session_event_model::CapabilityReport;

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionInspectRequest {
    pub target: SessionRef,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderHistoryAvailability {
    Available,
    HistoryUnavailable,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionInspectResult {
    pub target: SessionRef,
    pub state: ProviderSessionState,
    pub capabilities: CapabilityReport,
    pub history: ProviderHistoryAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_catalog: Option<ProviderSettingsCatalogView>,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingsCatalogView {
    pub current_mode: Option<String>,
    pub modes: Vec<ProviderSettingChoiceView>,
    pub config_options: Vec<ProviderConfigOptionView>,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingChoiceView {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderConfigOptionView {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<ProviderSettingName>,
    pub current_value: ProviderConfigValueView,
    pub choices: Vec<ProviderSettingChoiceView>,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ProviderConfigValueView {
    Select { value: String },
    Boolean { value: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderInspectFailureKind {
    NotFound,
    Unavailable,
    Overloaded,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderInspectFailure {
    pub kind: ProviderInspectFailureKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<crate::ConversationOperationFailureStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<SessionRef>,
    pub message: String,
}
