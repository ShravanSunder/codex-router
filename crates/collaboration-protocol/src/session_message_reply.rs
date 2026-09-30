//! Reply to one stored direct message using its push id or Router link.
use crate::{DeliveryReceipt, MessageText, PushId, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMessageReplyParams {
    /// Session receiving the earlier Agent communication and issuing this reply.
    pub caller: SessionRef,
    /// Local push id or router:// machine/push/id link being answered.
    pub reference: String,
    pub text: MessageText,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMessageReplyResult {
    /// Sender of the selected direct message.
    pub target: SessionRef,
    pub target_identity: String,
    pub push_id: PushId,
    pub link: String,
    pub receipt: DeliveryReceipt,
}
