use serde::{Deserialize, Serialize};

/// Features confirmed by the agent and its back door for one Session.
#[derive(schemars::JsonSchema, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityReport {
    pub load: bool,
    pub resume: bool,
    pub close: bool,
    pub list: bool,
    pub steer: bool,
    pub queue: Option<QueueSupport>,
    pub modes: bool,
    pub config_options: bool,
    pub elicitation: bool,
    pub usage: bool,
    pub prompt_content: PromptContentCapabilities,
    pub auth_status: ProviderAuthStatus,
}

impl CapabilityReport {
    /// ACP v1 always accepts text and resource links.
    #[must_use]
    pub const fn accepts_text(&self) -> bool {
        true
    }

    #[must_use]
    pub const fn accepts_resource_link(&self) -> bool {
        true
    }
}

#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueCapability {
    Native,
    Router,
}

#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueSupport {
    pub kind: QueueCapability,
    pub can_cancel: bool,
}

#[derive(schemars::JsonSchema, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptContentCapabilities {
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}

/// Connection-scoped auth status shared by Sessions on that provider connection.
/// It contains no account, organization, plan, or vendor details.
#[derive(schemars::JsonSchema, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ProviderAuthStatus {
    #[default]
    NotReported,
    LoggedOut,
    Account {
        label: String,
    },
    ApiKey {
        label: String,
    },
    Gateway {
        label: String,
    },
    External {
        label: String,
    },
}
