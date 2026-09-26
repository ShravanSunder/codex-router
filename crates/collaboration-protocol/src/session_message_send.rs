//! One public message request carries caller intent without selecting a client.
use crate::{CodexGeneration, DeliveryCorrelationId, MessageContent, MessageDelivery, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMessageSendParams {
    pub target: SessionRef,
    pub message: MessageContent,
    #[serde(default)]
    pub mode: MessageDelivery,
    pub generation_guard: Option<CodexGeneration>,
    pub correlation: Option<DeliveryCorrelationId>,
}
