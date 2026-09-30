//! Reply to the latest accepted Agent message delivered to one exact session.
use crate::{DeliveryReceipt, MessageText, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMessageReplyParams {
    /// Session receiving the earlier Agent communication and issuing this reply.
    pub caller: SessionRef,
    /// Refuses to send if the latest accepted Agent sender differs from this session.
    #[serde(default)]
    pub expect_sender: Option<SessionRef>,
    pub text: MessageText,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMessageReplyResult {
    /// Resolved recipient selected from the caller's latest accepted Agent delivery.
    pub target: SessionRef,
    /// Emoji-prefixed display identity for the resolved recipient.
    pub target_identity: String,
    pub receipt: DeliveryReceipt,
}
