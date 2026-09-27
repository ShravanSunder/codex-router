//! Immediate, actor-bound provider Session setting changes.

use crate::{EffectiveProviderSettings, ProviderIdentity, ProviderSettingName, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingsSetRequest {
    pub target: SessionRef,
    pub actor: ProviderIdentity,
    pub setting: ProviderSettingName,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingsAcceptRequest {
    pub target: SessionRef,
    pub actor: ProviderIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingsResult {
    pub target: SessionRef,
    pub effective_settings: EffectiveProviderSettings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderSettingsFailureKind {
    WrongActor,
    NotFound,
    Busy,
    InvalidSetting,
    ProviderRejected,
    OutcomeUnknown,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingsFailure {
    pub kind: ProviderSettingsFailureKind,
    pub target: SessionRef,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<ProviderSettingName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advertised: Vec<String>,
}
