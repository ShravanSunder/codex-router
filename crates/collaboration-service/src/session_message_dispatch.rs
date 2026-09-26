//! Public message Control call delegates one attempt to the injected delivery seam.
use crate::{
    DeliveryPrecondition, DeliveryRequest, ServiceIdentity, SessionMessageDelivery,
    session_delivery_contract::UnstoredAttemptEvidenceSink,
};
use collaboration_protocol::{MessageContent, SessionMessageSendParams};
use serde_json::{Value, json};

pub(crate) async fn dispatch(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(params) = serde_json::from_value::<SessionMessageSendParams>(params) else {
        return failure(id, -32602, "invalidField");
    };
    if params.target.endpoint.service_id != identity.service_id
        || matches!(params.message, MessageContent::Router { .. })
    {
        return failure(id, -32602, "invalidField");
    }
    let Some(delivery) = identity.session_delivery.as_ref() else {
        return failure(id, -32050, "unavailable");
    };
    deliver(id, params, delivery.as_ref()).await
}

async fn deliver(
    id: Value,
    params: SessionMessageSendParams,
    delivery: &dyn SessionMessageDelivery,
) -> Value {
    let request = DeliveryRequest {
        target: params.target,
        message: params.message,
        mode: params.mode,
        precondition: params
            .generation_guard
            .map_or(DeliveryPrecondition::Unpinned, |expected| {
                DeliveryPrecondition::EndpointGeneration { expected }
            }),
        correlation: params
            .correlation
            .unwrap_or_else(collaboration_protocol::DeliveryCorrelationId::generate),
        attempt: agent_automation::AttemptId::generate(),
    };
    match delivery
        .deliver(request, &UnstoredAttemptEvidenceSink)
        .await
    {
        Ok(receipt) => json!({"jsonrpc":"2.0","id":id,"result":receipt}),
        Err(_) => failure(id, -32050, "outcomeUnknown"),
    }
}

fn failure(id: Value, code: i64, kind: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":"Message request failed","data":{"kind":kind,"stage":"discovery","message":"Message request failed"}}})
}
