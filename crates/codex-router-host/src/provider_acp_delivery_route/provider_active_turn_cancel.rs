//! Resolve a running Session turn, then use the operation-bound cancel path.

use super::*;
use collaboration_protocol::{
    ConversationCancelRequest, ConversationOperationFailureKind, ConversationOperationSubmission,
};

#[derive(Debug, thiserror::Error)]
pub enum ProviderCancelActiveTurnError {
    #[error("provider Session not found")]
    SessionNotFound,
    #[error("noActiveTurn")]
    NoActiveTurn,
    #[error("provider Session is unavailable")]
    Unavailable,
    #[error("provider cancel operation failed")]
    Operation(Box<collaboration_protocol::ConversationOperationFailure>),
}

impl ProviderAcpDeliveryRoute {
    pub async fn cancel_active_turn(
        &self,
        target: SessionRef,
        actor: ProviderIdentity,
    ) -> Result<ConversationOperationSubmission, ProviderCancelActiveTurnError> {
        if !self.serves(&target) {
            return Err(ProviderCancelActiveTurnError::SessionNotFound);
        }
        let record = self
            .store
            .lock()
            .await
            .session_record(&target)
            .await
            .map_err(|_| ProviderCancelActiveTurnError::Unavailable)?
            .ok_or(ProviderCancelActiveTurnError::SessionNotFound)?;
        let runtime = self
            .supervisor
            .runtime_for(&target.endpoint)
            .ok_or(ProviderCancelActiveTurnError::Unavailable)?;
        let target_operation_id = runtime
            .active_prompt_operation(String::from(target.session_id.clone()))
            .await
            .map_err(|_| ProviderCancelActiveTurnError::Unavailable)?
            .ok_or(ProviderCancelActiveTurnError::NoActiveTurn)?;
        let binding = self
            .supervisor
            .binding(&target.endpoint)
            .ok_or(ProviderCancelActiveTurnError::Unavailable)?;
        self.supervisor
            .cancel(ConversationCancelRequest {
                operation_id: OperationId::generate(),
                target_operation_id,
                target,
                generation: Some(binding.generation),
                requested_by: actor,
                approver: record.approver,
            })
            .await
            .map_err(|failure| {
                if failure.kind == ConversationOperationFailureKind::NotFound {
                    ProviderCancelActiveTurnError::NoActiveTurn
                } else {
                    ProviderCancelActiveTurnError::Operation(Box::new(failure))
                }
            })
    }
}
