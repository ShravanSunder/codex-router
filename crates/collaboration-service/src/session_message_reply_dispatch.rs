//! Reply delivery resolves the latest accepted sender for the exact caller session.
use crate::{
    DeliveryPrecondition, DeliveryRequest, ServiceIdentity,
    session_delivery_contract::UnstoredAttemptEvidenceSink,
};
use automation_storage::StorageError;
use collaboration_protocol::{
    DeliveryCorrelationId, MessageContent, MessageDelivery, MessageHeaderContext,
    MessageHeaderOrigin, SessionMessageReplyParams,
};
use serde_json::{Value, json};

pub(crate) async fn dispatch(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(params) = serde_json::from_value::<SessionMessageReplyParams>(params) else {
        return failure(
            id,
            -32602,
            "invalidField",
            "Invalid message reply parameters",
        );
    };
    if params.caller.endpoint.service_id != identity.service_id {
        return failure(
            id,
            -32602,
            "wrongService",
            "Reply caller belongs to another Router",
        );
    }
    let Some(store) = identity.automation.as_ref() else {
        return failure(
            id,
            -32050,
            "replyUnavailable",
            "reply unavailable: Router automation store not running",
        );
    };
    if identity
        .latest_sender_unknown
        .lock()
        .await
        .contains(&params.caller)
    {
        return latest_sender_unknown(id);
    }
    let latest_sender = match store
        .lock()
        .await
        .latest_agent_sender_for(&params.caller, chrono::Utc::now())
        .await
    {
        Ok(Some(record)) => record.sender,
        Ok(None) => return latest_sender_unknown(id),
        Err(StorageError::InvalidRecord) => {
            identity
                .latest_sender_unknown
                .lock()
                .await
                .insert(params.caller.clone());
            return latest_sender_unknown(id);
        }
        Err(_) => {
            return failure(
                id,
                -32050,
                "replyUnavailable",
                "reply unavailable: Router automation store could not be read",
            );
        }
    };
    if latest_sender.endpoint.service_id != identity.service_id {
        identity
            .latest_sender_unknown
            .lock()
            .await
            .insert(params.caller.clone());
        return latest_sender_unknown(id);
    }
    let Some(delivery) = identity.session_delivery.as_ref() else {
        return failure(
            id,
            -32050,
            "replyUnavailable",
            "Router session delivery is unavailable",
        );
    };
    let message = MessageContent::Agent {
        sender: params.caller.clone(),
        text: params.text,
    };
    let header_context = MessageHeaderContext::resolve(
        &latest_sender,
        &message,
        &identity.display_names,
        MessageHeaderOrigin::Agent,
    );
    let request = DeliveryRequest {
        target: latest_sender,
        message,
        header_context,
        mode: MessageDelivery::Auto,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::generate(),
        attempt: agent_automation::AttemptId::generate(),
    };
    match delivery
        .deliver(request, &UnstoredAttemptEvidenceSink)
        .await
    {
        Ok(receipt) => json!({"jsonrpc":"2.0","id":id,"result":receipt}),
        Err(_) => failure(
            id,
            -32050,
            "outcomeUnknown",
            "Reply delivery outcome is unknown; inspect the recipient before retrying",
        ),
    }
}

fn latest_sender_unknown(id: Value) -> Value {
    failure(
        id,
        -32050,
        "latestSenderUnknown",
        "latest sender unknown; use message send --to <SessionRef>",
    )
}

fn failure(id: Value, code: i64, kind: &str, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message,"data":{"kind":kind,"stage":"discovery","message":message}}})
}
