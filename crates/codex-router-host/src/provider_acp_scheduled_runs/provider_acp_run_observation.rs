//! Provider operation settlement and summary source from live ACP output.
use super::ProviderAcpScheduledRuns;
use agent_automation::{ProviderAcpEffectEvidence, RouteEffectEvidence, SubmissionEffect};
use collaboration_protocol::{
    CodexGeneration, ConversationOperationSettlement, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, OperationId, PositiveSeconds, ProviderOperationEffect,
    ProviderOperationStage, ProviderPromptStopReason, SessionRef,
};
use collaboration_service::{
    DeliveryContractError, ProviderConversationBackend, RunObservationContext, RunReconciliation,
    RunSettlement,
};

impl ProviderAcpScheduledRuns {
    pub(super) fn recorded_provider(
        &self,
        context: &RunObservationContext,
    ) -> Result<ProviderAcpEffectEvidence<SessionRef, CodexGeneration>, DeliveryContractError> {
        let RouteEffectEvidence::ProviderAcp(provider) = &context.recorded else {
            return Err(DeliveryContractError::InvalidEvidence);
        };
        if provider.target.endpoint.service_id != self.service_id {
            return Err(DeliveryContractError::InvalidEvidence);
        }
        Ok(provider.clone())
    }

    pub(super) fn operation_id(
        provider: &ProviderAcpEffectEvidence<SessionRef, CodexGeneration>,
    ) -> Result<OperationId, DeliveryContractError> {
        OperationId::try_from(provider.attempt_id.as_str().to_owned())
            .map_err(|_| DeliveryContractError::InvalidEvidence)
    }

    pub(super) async fn observe_provider_run(
        &self,
        context: &RunObservationContext,
    ) -> Result<RunSettlement, DeliveryContractError> {
        let provider = self.recorded_provider(context)?;
        let operation_id = Self::operation_id(&provider)?;
        let timeout_seconds =
            PositiveSeconds::try_from(1).map_err(|_| DeliveryContractError::InvalidEvidence)?;
        let result = self
            .supervisor
            .wait(ConversationOperationWaitRequest {
                operation_id,
                timeout_seconds,
            })
            .await;
        match result {
            Ok(result)
                if result
                    .operation
                    .binding
                    .external_provider()
                    .is_some_and(|binding| {
                        binding.generation == provider.generation
                            && String::from(binding.binding_id.clone()) == provider.binding.as_str()
                    })
                    && result.operation.target.as_ref() == Some(&provider.target) =>
            {
                match result.output {
                    ConversationOperationWaitOutput::Pending => Ok(RunSettlement::Pending),
                    ConversationOperationWaitOutput::Available {
                        settlement:
                            ConversationOperationSettlement::PromptCompleted {
                                target,
                                stop_reason,
                                ..
                            },
                    } if provider.target.eq(&target) => match stop_reason {
                        ProviderPromptStopReason::Cancelled => Ok(RunSettlement::Interrupted),
                        ProviderPromptStopReason::Refusal => Ok(RunSettlement::Failed {
                            reason: "Provider refused the scheduled prompt".into(),
                        }),
                        ProviderPromptStopReason::EndTurn
                        | ProviderPromptStopReason::MaxTokens
                        | ProviderPromptStopReason::MaxTurnRequests => {
                            Ok(RunSettlement::Completed {
                                summary_source: None,
                            })
                        }
                    },
                    ConversationOperationWaitOutput::Available { .. } => {
                        Err(DeliveryContractError::InvalidEvidence)
                    }
                    ConversationOperationWaitOutput::OutputUnavailable { .. }
                        if result.operation.stage == ProviderOperationStage::Terminal
                            && result.operation.effect == ProviderOperationEffect::Applied =>
                    {
                        Ok(result.operation.terminal_stop_reason.map_or_else(
                            || RunSettlement::Failed {
                                reason: "Provider terminal outcome is unavailable".into(),
                            },
                            settlement_from_stop_reason,
                        ))
                    }
                    ConversationOperationWaitOutput::OutputUnavailable { .. }
                        if result.operation.stage == ProviderOperationStage::Terminal
                            && result.operation.effect == ProviderOperationEffect::None =>
                    {
                        Ok(RunSettlement::Failed {
                            reason: "Provider prompt was not submitted".into(),
                        })
                    }
                    ConversationOperationWaitOutput::OutputUnavailable { .. } => {
                        Ok(RunSettlement::Pending)
                    }
                }
            }
            Ok(_) => Err(DeliveryContractError::InvalidEvidence),
            Err(failure) if failure.effect == ProviderOperationEffect::Unknown => {
                Ok(RunSettlement::Pending)
            }
            Err(failure) => Ok(RunSettlement::Failed {
                reason: String::from(failure.message),
            }),
        }
    }

    pub(super) async fn reconcile_provider_run(
        &self,
        context: &RunObservationContext,
    ) -> Result<RunReconciliation, DeliveryContractError> {
        let provider = self.recorded_provider(context)?;
        let operation_id = Self::operation_id(&provider)?;
        let operation = self
            .store
            .lock()
            .await
            .inspect(&operation_id)
            .await
            .map_err(|_| DeliveryContractError::ClientOperation)?;
        let Some(operation) = operation else {
            return Ok(
                if matches!(
                    provider.submission,
                    SubmissionEffect::NotDispatched | SubmissionEffect::Dispatching
                ) {
                    RunReconciliation::KnownNotSubmitted
                } else {
                    RunReconciliation::StillUnknown
                },
            );
        };
        if !operation
            .binding
            .external_provider()
            .is_some_and(|binding| {
                binding.generation == provider.generation
                    && String::from(binding.binding_id.clone()) == provider.binding.as_str()
            })
            || operation.target.as_ref() != Some(&provider.target)
        {
            return Err(DeliveryContractError::InvalidEvidence);
        }
        match (operation.stage, operation.effect) {
            (ProviderOperationStage::Terminal, ProviderOperationEffect::None) => {
                Ok(RunReconciliation::KnownNotSubmitted)
            }
            (ProviderOperationStage::Terminal, ProviderOperationEffect::Applied) => {
                Ok(RunReconciliation::Settled {
                    settlement: self.observe_provider_run(context).await?,
                })
            }
            _ => Ok(RunReconciliation::StillUnknown),
        }
    }
}

fn settlement_from_stop_reason(reason: ProviderPromptStopReason) -> RunSettlement {
    match reason {
        ProviderPromptStopReason::Cancelled => RunSettlement::Interrupted,
        ProviderPromptStopReason::Refusal => RunSettlement::Failed {
            reason: "Provider refused the scheduled prompt".into(),
        },
        ProviderPromptStopReason::EndTurn
        | ProviderPromptStopReason::MaxTokens
        | ProviderPromptStopReason::MaxTurnRequests => RunSettlement::Completed {
            summary_source: None,
        },
    }
}
