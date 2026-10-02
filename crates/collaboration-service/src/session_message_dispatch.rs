//! Public message Control call is stored before the one-line delivery attempt.
use crate::{ServiceIdentity, push_record_delivery};
use collaboration_protocol::SessionMessageSendParams;
use serde_json::Value;

pub(crate) async fn dispatch(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(params) = serde_json::from_value::<SessionMessageSendParams>(params) else {
        return crate::push_record_resolver::failure(
            id,
            -32602,
            "invalidField",
            "discovery",
            "Invalid message request",
        );
    };
    push_record_delivery::dispatch_message(id, params, identity).await
}
