//! Reply to the latest accepted Agent message delivered to one exact session.
use crate::{MessageText, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMessageReplyParams {
    /// Session receiving the earlier Agent communication and issuing this reply.
    pub caller: SessionRef,
    pub text: MessageText,
}
