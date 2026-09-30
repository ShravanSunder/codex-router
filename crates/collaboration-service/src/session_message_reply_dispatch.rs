//! Replies resolve one retained DM id/link instead of guessing the latest sender.
use crate::{
    ServiceIdentity, push_record_delivery, push_record_resolver,
};
use automation_storage::{StorageError};
use collaboration_protocol::SessionMessageReplyParams;
use serde_json::Value;

pub(crate) async fn dispatch(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let params = match serde_json::from_value::<SessionMessageReplyParams>(params) {
        Ok(params) => params,
        Err(_) => {
            return push_record_resolver::failure(
                id,
                -32602,
                "invalidField",
                "discovery",
                "Reply requires a DM push id or Router link and text",
            );
        }
    };
    if params.caller.endpoint.service_id != identity.service_id {
        return push_record_resolver::failure(
            id,
            -32602,
            "wrongService",
            "discovery",
            "Reply caller belongs to another Router",
        );
    }
    let push_id = match push_record_resolver::resolve_reference(&params.reference, identity) {
        Ok(push_id) => push_id,
        Err(push_record_resolver::ReferenceError::Invalid) => {
            return push_record_resolver::failure(
                id,
                -32602,
                "invalidField",
                "discovery",
                "Invalid push id or Router link",
            );
        }
        Err(push_record_resolver::ReferenceError::Foreign(machine_id)) => {
            return push_record_resolver::failure(
                id,
                -32050,
                "foreignMachine",
                "discovery",
                &format!("lives on {machine_id}; cross-machine fetch not available yet"),
            );
        }
    };
    let Some(store) = identity.automation.as_ref() else {
        return push_record_resolver::failure(
            id,
            -32050,
            "unavailable",
            "discovery",
            "Push storage is unavailable",
        );
    };
    let record = match store.lock().await.get_push_record(&push_id).await {
        Ok(Some(record)) => record,
        Ok(None) | Err(StorageError::PushNotFound) => {
            return push_record_resolver::not_found(id, "discovery");
        }
        Err(_) => {
            return push_record_resolver::failure(
                id,
                -32050,
                "unavailable",
                "discovery",
                "Push storage could not be read",
            );
        }
    };
    if !push_record_resolver::can_read(&record, &params.caller) {
        return push_record_resolver::failure(
            id,
            -32050,
            "notPermitted",
            "discovery",
            "Not permitted to reply to this push",
        );
    }
    push_record_delivery::dispatch_reply(id, params, record, identity).await
}
