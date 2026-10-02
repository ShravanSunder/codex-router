//! Queue ACP delivery when the Session is already running.

use super::*;

impl ProviderAcpDeliveryRoute {
    pub(super) async fn queue_delivery(
        &self,
        request: &ProviderDeliveryRequest,
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
        let (input_id, queued_prompt) = match &request.content {
            ProviderDeliveryContent::PreparedPush { line, .. } => {
                let input_id = request.input_id()?;
                let prompt_request = ProviderPromptContentsRequest::from_prepared_push(
                    operation_id.clone(),
                    input_id.clone(),
                    request.target.clone(),
                    record.created_by.clone(),
                    record.approver.clone(),
                    line,
                )?;
                (
                    input_id,
                    ProviderQueuedPrompt::Contents {
                        request: prompt_request,
                        load_policy: request.load_policy,
                    },
                )
            }
        };
        let permit = match self.queue.reserve(&request.target) {
            Ok(permit) => permit,
            Err(reason) => return Ok(Self::rejected(DeliveryRejectionReason::Busy, reason)),
        };
        let effect = Self::effect(request, binding, SubmissionEffect::RouterQueued)?;
        sink.record(effect).await?;
        match &queued_prompt {
            ProviderQueuedPrompt::Contents {
                request: prompt, ..
            } => {
                self.supervisor
                    .queued_operation_registry()
                    .record_queued_contents(
                        operation_id.clone(),
                        request.target.clone(),
                        binding.clone(),
                        input_id,
                        &prompt.contents,
                    );
            }
        }
        permit.send(queued_prompt);
        Ok(Self::receipt(
            DeliveryOutcome::Queued,
            Some(operation_id.clone()),
        ))
    }
}
