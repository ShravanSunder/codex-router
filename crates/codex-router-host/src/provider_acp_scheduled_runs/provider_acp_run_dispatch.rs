//! Provider scheduled prompt admission and exact-operation stop requests.
use super::ProviderAcpScheduledRuns;
use crate::provider_acp_session_loading::{
    ProviderSessionLoadOutcome, ensure_provider_session_loaded,
};
use crate::{ProviderSessionActivity, external_provider_supervisor::ProviderPromptDispatch};
use agent_automation::{RouteEffectEvidence, SubmissionEffect};
use collaboration_protocol::{
    ConversationCancelRequest, ConversationPromptRequest, DeliveryClientReceipt,
    DeliveryNextAction, DeliveryOutcome, DeliveryReceipt, DeliveryRejection,
    DeliveryRejectionReason, MessageContent, OperationId, ProviderOperationEffect,
    ProviderOperationStage, RunExecution, SessionReachability, render_message,
};
use collaboration_service::{
    DeliveryContractError, DeliveryPrecondition, ProviderConversationBackend, RunAcceptance,
    RunEvidenceDisposition, RunEvidenceSink, RunObservationContext, RunSubmission,
    ScheduledRunSubmission, StopRequestOutcome,
};

impl ProviderAcpScheduledRuns {
    pub(super) async fn submit_provider_run(
        &self,
        run: ScheduledRunSubmission,
        sink: &dyn RunEvidenceSink,
    ) -> Result<RunSubmission, DeliveryContractError> {
        let RouteEffectEvidence::ProviderAcp(mut effect) = run.recorded else {
            return Err(DeliveryContractError::InvalidEvidence);
        };
        if effect.target != run.target || run.target.endpoint.service_id != self.service_id {
            return Err(DeliveryContractError::InvalidEvidence);
        }
        let Some(binding) = self.supervisor.binding(&run.target.endpoint) else {
            return Ok(rejected(
                DeliveryRejectionReason::EndpointUnavailable,
                DeliveryNextAction::RetryLater,
                "Provider binding is unavailable",
            ));
        };
        if binding.generation != effect.generation
            || String::from(binding.binding_id) != effect.binding.as_str()
            || matches!(run.precondition, DeliveryPrecondition::EndpointGeneration { ref expected } if expected != &binding.generation)
        {
            return Ok(rejected(
                DeliveryRejectionReason::StaleGeneration,
                DeliveryNextAction::InspectTarget,
                "Provider generation changed before scheduled submission",
            ));
        }
        let Some(runtime) = self.supervisor.runtime_for(&run.target.endpoint) else {
            return Ok(RunSubmission::NotStartedBusy);
        };
        let session_id = String::from(run.target.session_id.clone());
        match runtime.session_activity(session_id.clone()).await {
            Ok(ProviderSessionActivity::Running) => return Ok(RunSubmission::NotStartedBusy),
            Ok(ProviderSessionActivity::NotLoaded) => {
                if matches!(
                    sink.record(RouteEffectEvidence::ProviderAcp(effect.clone()))
                        .await?,
                    RunEvidenceDisposition::AdmissionRefused
                ) {
                    return Ok(RunSubmission::NotStartedBusy);
                }
                if !matches!(
                    ensure_provider_session_loaded(
                        &self.supervisor,
                        &self.store,
                        self.ownership.as_ref(),
                        &run.target,
                    )
                    .await,
                    ProviderSessionLoadOutcome::Ready
                ) {
                    return Ok(RunSubmission::NotStartedBusy);
                }
            }
            Ok(ProviderSessionActivity::Idle) => {}
            Err(_) => return Ok(RunSubmission::NotStartedBusy),
        }
        if !matches!(
            runtime.session_activity(session_id).await,
            Ok(ProviderSessionActivity::Idle)
        ) {
            return Ok(RunSubmission::NotStartedBusy);
        }
        let record = self
            .store
            .lock()
            .await
            .session_record(&run.target)
            .await
            .map_err(|_| DeliveryContractError::ClientOperation)?
            .ok_or(DeliveryContractError::ClientOperation)?;
        let message = MessageContent::Router { text: run.message };
        render_message(&run.target, &message)
            .map_err(|_| DeliveryContractError::ClientOperation)?;
        let operation_id = Self::operation_id(&effect)?;
        effect.submission = SubmissionEffect::Dispatching;
        let timing = match sink
            .record(RouteEffectEvidence::ProviderAcp(effect.clone()))
            .await?
        {
            RunEvidenceDisposition::Recorded {
                timing: Some(timing),
            } => timing,
            RunEvidenceDisposition::Recorded { timing: None } => {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            RunEvidenceDisposition::AdmissionRefused => return Ok(RunSubmission::NotStartedBusy),
        };
        let submitted = self
            .supervisor
            .submit_delivery_prompt(ConversationPromptRequest {
                operation_id: operation_id.clone(),
                target: run.target.clone(),
                generation: Some(binding.generation),
                requested_by: record.created_by,
                approver: record.approver,
                prompt: message,
            })
            .await;
        let result = match submitted {
            Ok(ProviderPromptDispatch::Submitted) => {
                effect.submission = SubmissionEffect::Accepted;
                let started_at = timestamp(timing.dispatch_started_at_ms)?;
                let deadline_at = timestamp(timing.deadline_at_ms)?;
                RunSubmission::Started(Box::new(RunAcceptance {
                    execution: RunExecution::ProviderAcp {
                        target: run.target,
                        operation_id: operation_id.clone(),
                        started_at,
                        deadline_at,
                        effective_timeout_seconds: timing
                            .effective_timeout_seconds
                            .try_into()
                            .map_err(|_| DeliveryContractError::InvalidEvidence)?,
                    },
                    receipt: DeliveryReceipt {
                        outcome: DeliveryOutcome::Started,
                        reachability: Some(SessionReachability::ProviderAcp),
                        client: Some(DeliveryClientReceipt::ProviderAcp { operation_id }),
                    },
                }))
            }
            Ok(ProviderPromptDispatch::NotSubmitted) => {
                effect.submission = SubmissionEffect::NotDispatched;
                rejected(
                    DeliveryRejectionReason::EndpointUnavailable,
                    DeliveryNextAction::RetryLater,
                    "Provider scheduled prompt was not sent",
                )
            }
            Ok(ProviderPromptDispatch::Existing | ProviderPromptDispatch::Uncertain) => {
                effect.submission = SubmissionEffect::Unknown;
                RunSubmission::Unknown
            }
            Err(failure) if failure.effect == ProviderOperationEffect::None => {
                effect.submission = SubmissionEffect::NotDispatched;
                rejected(
                    DeliveryRejectionReason::EndpointUnavailable,
                    DeliveryNextAction::RetryLater,
                    &String::from(failure.message),
                )
            }
            Err(_) => {
                effect.submission = SubmissionEffect::Unknown;
                RunSubmission::Unknown
            }
        };
        if !matches!(
            sink.record(RouteEffectEvidence::ProviderAcp(effect)).await,
            Ok(RunEvidenceDisposition::Recorded { .. })
        ) {
            return Ok(RunSubmission::Unknown);
        }
        Ok(result)
    }

    pub(super) async fn stop_provider_run(
        &self,
        context: &RunObservationContext,
        sink: &dyn RunEvidenceSink,
    ) -> Result<StopRequestOutcome, DeliveryContractError> {
        let provider = self.recorded_provider(context)?;
        let target = &provider.target;
        if provider.submission != SubmissionEffect::Accepted {
            return Ok(StopRequestOutcome::Unsupported);
        }
        let operation_id = Self::operation_id(&provider)?;
        let Some(runtime) = self.supervisor.runtime_for(&target.endpoint) else {
            return Ok(StopRequestOutcome::Unknown);
        };
        if runtime.retirement().is_cancelled() {
            return Ok(StopRequestOutcome::Unknown);
        }
        let operation = self
            .store
            .lock()
            .await
            .inspect(&operation_id)
            .await
            .map_err(|_| DeliveryContractError::ClientOperation)?;
        let Some(operation) = operation else {
            return Ok(StopRequestOutcome::Unknown);
        };
        if operation.stage == ProviderOperationStage::Terminal {
            return Ok(StopRequestOutcome::Unsupported);
        }
        if operation.target.as_ref() != Some(target)
            || !operation
                .binding
                .external_provider()
                .is_some_and(|binding| {
                    binding.generation == provider.generation
                        && String::from(binding.binding_id.clone()) == provider.binding.as_str()
                })
        {
            return Err(DeliveryContractError::InvalidEvidence);
        }
        let record = self
            .store
            .lock()
            .await
            .session_record(target)
            .await
            .map_err(|_| DeliveryContractError::ClientOperation)?;
        let Some(record) = record else {
            return Ok(StopRequestOutcome::Unknown);
        };
        if matches!(
            sink.record_stop_intent().await?,
            RunEvidenceDisposition::AdmissionRefused
        ) {
            return Ok(StopRequestOutcome::Unsupported);
        }
        let cancel = self
            .supervisor
            .cancel(ConversationCancelRequest {
                operation_id: OperationId::generate(),
                target_operation_id: operation_id,
                target: target.clone(),
                generation: Some(provider.generation),
                requested_by: record.created_by,
                approver: record.approver,
            })
            .await;
        Ok(if cancel.is_ok() {
            StopRequestOutcome::Requested
        } else {
            StopRequestOutcome::Unknown
        })
    }
}

fn timestamp(
    milliseconds: i64,
) -> Result<collaboration_protocol::ObservationTimestamp, DeliveryContractError> {
    let value = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(milliseconds)
        .ok_or(DeliveryContractError::InvalidEvidence)?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    collaboration_protocol::ObservationTimestamp::try_from(value)
        .map_err(|_| DeliveryContractError::InvalidEvidence)
}

fn rejected(
    reason: DeliveryRejectionReason,
    next_action: DeliveryNextAction,
    detail: &str,
) -> RunSubmission {
    RunSubmission::Rejected(DeliveryRejection {
        reason,
        next_action,
        client_code: None,
        detail: Some(detail.to_owned()),
    })
}
