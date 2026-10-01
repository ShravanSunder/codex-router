//! Public message Control call delegates one attempt to the injected delivery seam.
use crate::{
    DeliveryPrecondition, DeliveryRequest, LoadPolicy, ServiceIdentity,
    latest_agent_sender_tracking, session_delivery_contract::UnstoredAttemptEvidenceSink,
};
use collaboration_protocol::{
    MessageContent, MessageHeaderContext, MessageHeaderOrigin, SessionMessageSendParams,
};
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
    deliver(id, params, identity, delivery.as_ref()).await
}

async fn deliver(
    id: Value,
    params: SessionMessageSendParams,
    identity: &ServiceIdentity,
    delivery: &dyn crate::SessionMessageDelivery,
) -> Value {
    let latest_sender = match &params.message {
        MessageContent::Agent { sender, .. } => Some((params.target.clone(), sender.clone())),
        MessageContent::HumanUser { .. } | MessageContent::Router { .. } => None,
    };
    let header_context = MessageHeaderContext::resolve(
        &params.target,
        &params.message,
        &identity.display_names,
        MessageHeaderOrigin::Agent,
    );
    let request = DeliveryRequest {
        target: params.target,
        message: params.message,
        header_context,
        mode: params.mode,
        load_policy: LoadPolicy::MayLoad,
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
    let result = delivery
        .deliver(request, &UnstoredAttemptEvidenceSink)
        .await;
    match result {
        Ok(receipt) => {
            match latest_sender {
                Some((recipient, sender))
                    if latest_agent_sender_tracking::is_accepted(&receipt.outcome) =>
                {
                    latest_agent_sender_tracking::record_after_acceptance(
                        identity.automation.as_ref(),
                        &identity.latest_sender_unknown,
                        &recipient,
                        &sender,
                    )
                    .await;
                }
                Some((recipient, _))
                    if latest_agent_sender_tracking::is_unknown(&receipt.outcome) =>
                {
                    latest_agent_sender_tracking::invalidate_after_unknown(
                        identity.automation.as_ref(),
                        &identity.latest_sender_unknown,
                        &recipient,
                    )
                    .await;
                }
                _ => {}
            }
            json!({"jsonrpc":"2.0","id":id,"result":receipt})
        }
        Err(_) => {
            if let Some((recipient, _)) = latest_sender {
                latest_agent_sender_tracking::invalidate_after_unknown(
                    identity.automation.as_ref(),
                    &identity.latest_sender_unknown,
                    &recipient,
                )
                .await;
            }
            failure(id, -32050, "outcomeUnknown")
        }
    }
}

fn failure(id: Value, code: i64, kind: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":"Message request failed","data":{"kind":kind,"stage":"discovery","message":"Message request failed"}}})
}
