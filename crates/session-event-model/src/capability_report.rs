use serde::{Deserialize, Serialize};

/// Features confirmed by the agent and its back door for one Session.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueCapability {
    Native,
    Router,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueSupport {
    pub kind: QueueCapability,
    pub can_cancel: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptContentCapabilities {
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}
