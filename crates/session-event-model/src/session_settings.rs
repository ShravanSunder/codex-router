use serde::{Deserialize, Serialize};

/// Current settings reported for one provider Session.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSettings {
    pub mode: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub config: Vec<ConfigValue>,
}

/// One current config option value, identified by the provider's option ID.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigValue {
    pub id: String,
    pub value: ConfigValueState,
}

/// ACP v1 config options currently report either a selected ID or a boolean.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValueState {
    Select(String),
    Boolean(bool),
}
