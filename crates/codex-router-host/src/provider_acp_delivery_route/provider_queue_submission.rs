//! Queue ACP delivery when the Session is already running.

use super::*;

impl ProviderAcpDeliveryRoute {
    pub(super) async fn queue_delivery(
        &self,
        request: &DeliveryRequest,
        sink: &dyn AttemptEvidenceSink,
        binding: &collaboration_protocol::ProviderBindingIdentity,
        operation_id: &OperationId,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        let record = self
            .store
            .lock()
            .await
            .session_record(&request.target)
            .await
            .map_err(|_| DeliveryContractError::ClientOperation)?;
        let Some(record) = record else {
            return Ok(Self::not_submitted(
                "provider session record is missing",
                false,
            ));
        };
        let requested_by = match &request.message {
            MessageContent::Agent { sender, .. } => sender.clone(),
            MessageContent::HumanUser { .. } | MessageContent::Router { .. } => record.created_by,
        };
        let permit = match self.queue.reserve(&request.target) {
            Ok(permit) => permit,
            Err(reason) => return Ok(Self::rejected(DeliveryRejectionReason::Busy, reason)),
        };
        let effect = Self::effect(request, binding, SubmissionEffect::RouterQueued)?;
        sink.record(effect).await?;
        self.supervisor.queued_operation_registry().record_queued(
            operation_id.clone(),
            request.target.clone(),
            binding.clone(),
        );
        permit.send(ConversationPromptRequest {
            operation_id: operation_id.clone(),
            target: request.target.clone(),
            generation: Some(binding.generation.clone()),
            requested_by,
            approver: record.approver,
            prompt: request.message.clone(),
        });
        Ok(Self::receipt(
            DeliveryOutcome::Queued,
            Some(operation_id.clone()),
        ))
    }
}
