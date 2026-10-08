//! Replies resolve one retained DM id/link instead of guessing the latest sender.
use crate::ServiceIdentity;
use crate::collaboration_application::MessageOperations;
use collaboration_protocol::SessionMessageReplyParams;
use serde_json::Value;

pub(crate) async fn dispatch(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(params) = serde_json::from_value::<SessionMessageReplyParams>(params) else {
        return crate::push_record_resolver::failure(
            id,
            -32602,
            "invalidField",
            "discovery",
            "Reply requires a DM push id or Router link and text",
        );
    };
    crate::push_record_resolver::message_response(
        id,
        MessageOperations::new(identity).message_reply(params).await,
    )
}
