//! Delivery prompt admission waits for the ACP request to enter the session actor.
use super::*;
use crate::provider_operation_settlement::{ProviderOperationCompletion, optional_message_text};
use crate::provider_session_actor::ProviderPromptDispatchObservation;

const DELIVERY_DISPATCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderPromptDispatch {
    Submitted,
    NotSubmitted,
    Existing,
    Uncertain,
}

impl ExternalProviderSupervisor {
    pub(crate) async fn submit_delivery_prompt(
        &self,
        request: ConversationPromptRequest,
    ) -> Result<ProviderPromptDispatch, Box<ConversationOperationFailure>> {
        let operation_id = request.operation_id.clone();
        let target = request.target.clone();
        let rendered = render_message(&target, &request.prompt).map_err(|_| {
            Box::new(failure(
                ConversationOperationFailureKind::InvalidRequest,
                ConversationOperationFailureStage::Validation,
                ProviderOperationEffect::None,
                "provider delivery prompt could not be rendered",
                operation_id.clone(),
                Some(target.clone()),
            ))
        })?;
        let (binding, runtime) = self
            .runtime_binding(&target.endpoint, &operation_id, Some(target.clone()))
            .map_err(Box::new)?;
        let approval_context = crate::ExternalProviderApprovalContext {
            requester: request.requested_by,
            approver: request.approver,
            target: target.clone(),
            operation_id: operation_id.clone(),
            binding_generation: binding.generation.clone(),
            binding_retirement: runtime.retirement(),
        };
        let prepared = self
            .prepare_operation(
                operation_id.clone(),
                ProviderOperationKind::ConversationPrompt,
                binding,
                Some(&target),
            )
            .await
            .map_err(Box::new)?;
        let PreparedOperation::Admitted { live, .. } = prepared else {
            return Ok(ProviderPromptDispatch::Existing);
        };
        let (dispatched, dispatch_observation) = tokio::sync::oneshot::channel();
        let provider_session_id = String::from(target.session_id.clone());
        let completion_target = target.clone();
        let completion_operation_id = operation_id.clone();
        self.spawn_operation(operation_id, live, async move {
            match runtime
                .prompt_with_approval_dispatch(
                    provider_session_id,
                    rendered.text,
                    approval_context,
                    Some(dispatched),
                )
                .await
            {
                Ok(outcome) => match optional_message_text(outcome.output) {
                    Ok(response) => ProviderOperationCompletion::Success {
                        settlement: ConversationOperationSettlement::PromptCompleted {
                            target: completion_target.clone(),
                            stop_reason: outcome.stop_reason,
                            response,
                        },
                        target: Some(completion_target),
                        session_record: None,
                    },
                    Err(_) => ProviderOperationCompletion::Failure(failure(
                        ConversationOperationFailureKind::ProtocolViolation,
                        ConversationOperationFailureStage::Settlement,
                        ProviderOperationEffect::Applied,
                        "provider returned invalid prompt output",
                        completion_operation_id,
                        Some(completion_target),
                    )),
                },
                Err(error) => ProviderOperationCompletion::Failure(prompt_runtime_failure(
                    completion_operation_id,
                    Some(completion_target),
                    error,
                )),
            }
        });
        Ok(
            match tokio::time::timeout(DELIVERY_DISPATCH_TIMEOUT, dispatch_observation).await {
                Ok(Ok(ProviderPromptDispatchObservation::Submitted)) => {
                    ProviderPromptDispatch::Submitted
                }
                Ok(Ok(ProviderPromptDispatchObservation::NotSubmitted)) => {
                    ProviderPromptDispatch::NotSubmitted
                }
                Ok(Err(_)) | Err(_) => ProviderPromptDispatch::Uncertain,
            },
        )
    }
}
