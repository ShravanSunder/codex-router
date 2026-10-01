//! Provider route input without collapsing prepared push content into a legacy message.

use collaboration_protocol::{
    DeliveryCorrelationId, MessageContent, MessageDelivery, MessageHeaderContext, MessageText,
    PushId, SessionRef,
};
use collaboration_service::{
    DeliveryContractError, DeliveryPrecondition, DeliveryRequest, LoadPolicy,
};

pub(super) enum ProviderDeliveryContent {
    Message {
        message: MessageContent,
        header_context: MessageHeaderContext,
    },
    PreparedPush {
        push_id: PushId,
        line: MessageText,
    },
}

pub(super) struct ProviderDeliveryRequest {
    pub target: SessionRef,
    pub content: ProviderDeliveryContent,
    pub mode: MessageDelivery,
    pub load_policy: LoadPolicy,
    pub precondition: DeliveryPrecondition,
    pub correlation: DeliveryCorrelationId,
    pub attempt: agent_automation::AttemptId,
}

impl From<DeliveryRequest> for ProviderDeliveryRequest {
    fn from(request: DeliveryRequest) -> Self {
        Self {
            target: request.target,
            content: ProviderDeliveryContent::Message {
                message: request.message,
                header_context: request.header_context,
            },
            mode: request.mode,
            load_policy: request.load_policy,
            precondition: request.precondition,
            correlation: request.correlation,
            attempt: request.attempt,
        }
    }
}

impl From<collaboration_service::layer_zero::DeliveryRequest> for ProviderDeliveryRequest {
    fn from(request: collaboration_service::layer_zero::DeliveryRequest) -> Self {
        Self {
            target: request.target,
            content: ProviderDeliveryContent::PreparedPush {
                push_id: request.payload.push_id,
                line: request.payload.line,
            },
            mode: request.mode,
            load_policy: request.payload.load_policy,
            precondition: request.precondition,
            correlation: request.correlation,
            attempt: request.attempt,
        }
    }
}

impl ProviderDeliveryRequest {
    /// The push contract uses its stored id as the delivery correlation id.
    pub(super) fn validate_push_correlation(&self) -> Result<(), DeliveryContractError> {
        if let ProviderDeliveryContent::PreparedPush { push_id, .. } = &self.content
            && self.correlation.as_str() != push_id.as_str()
        {
            return Err(DeliveryContractError::InvalidEvidence);
        }
        Ok(())
    }

    pub(super) fn input_id(&self) -> Result<session_event_model::InputId, DeliveryContractError> {
        match &self.content {
            ProviderDeliveryContent::Message { .. } => Ok(session_event_model::InputId::generate()),
            ProviderDeliveryContent::PreparedPush { .. } => {
                self.validate_push_correlation()?;
                // ACP tracks accepted provider input with its Host-assigned InputId.
                session_event_model::InputId::try_from(self.correlation.as_str().to_owned())
                    .map_err(|_| DeliveryContractError::InvalidEvidence)
            }
        }
    }
}
