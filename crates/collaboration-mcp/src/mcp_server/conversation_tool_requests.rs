//! MCP request wrappers add a bounded wait to the common conversation inputs.
use collaboration_client::{
    ConversationCreateInput, ConversationCreatePromptInput, ConversationLoadInput,
    ConversationPromptInput,
};
use collaboration_protocol::PositiveSeconds;
use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(extend("allOf" = [
    {
        "if": {"properties": {"endpoint": {"properties": {"endpointId": {"const": "codex-local"}}}}},
        "then": {"required": ["model", "effort"]}
    },
    {
        "if": {"properties": {"endpoint": {"properties": {"endpointId": {"enum": ["claude-local", "cursor-local"]}}}}},
        "then": {"not": {"anyOf": [{"required": ["model"]}, {"required": ["effort"]}]}}
    }
]))]
pub(super) struct ConversationCreateToolRequest {
    #[serde(flatten)]
    pub(super) create: ConversationCreateInput,
    #[serde(default)]
    pub(super) timeout_seconds: Option<PositiveSeconds>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationLoadToolRequest {
    #[serde(flatten)]
    pub(super) load: ConversationLoadInput,
    #[serde(default)]
    pub(super) timeout_seconds: Option<PositiveSeconds>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationPromptToolRequest {
    #[serde(flatten)]
    pub(super) prompt: ConversationPromptInput,
    #[serde(default)]
    pub(super) timeout_seconds: Option<PositiveSeconds>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(extend("allOf" = [
    {
        "if": {"properties": {"create": {"properties": {"endpoint": {"properties": {"endpointId": {"const": "codex-local"}}}}}}},
        "then": {"properties": {"create": {"required": ["model", "effort"]}}}
    },
    {
        "if": {"properties": {"create": {"properties": {"endpoint": {"properties": {"endpointId": {"enum": ["claude-local", "cursor-local"]}}}}}}},
        "then": {"properties": {"create": {"not": {"anyOf": [{"required": ["model"]}, {"required": ["effort"]}]}}}}
    }
]))]
pub(super) struct ConversationCreatePromptToolRequest {
    #[serde(flatten)]
    pub(super) input: ConversationCreatePromptInput,
    #[serde(default)]
    pub(super) timeout_seconds: Option<PositiveSeconds>,
}
