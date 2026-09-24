//! ACP provider message delivery, evidence, and reconciliation.
use crate::external_provider_supervisor::ProviderPromptDispatch;
use crate::provider_acp_message_fifo::ProviderAcpMessageFifo;
use crate::provider_acp_route_claim::ProviderAcpRouteClaim;
use crate::provider_acp_session_loading::{
    ProviderSessionLoadOutcome, ensure_provider_session_loaded,
};
use crate::{
    ExternalProviderSupervisor, LiveSessionOwnershipCheck, ProviderSessionActivity,
    ProviderSteeringOutcome,
};
use agent_automation::{
    ProviderAcpEffectEvidence, ProviderBindingReference, ProviderSettlementEffect,
    RouteEffectEvidence, SubmissionEffect,
};
use collaboration_protocol::{
    CodexGeneration, ConversationPromptRequest, DeliveryClientReceipt, DeliveryNextAction,
    DeliveryOutcome, DeliveryReceipt, DeliveryRejection, DeliveryRejectionReason, MessageContent,
    MessageDelivery, OperationId, ProviderOperationEffect, ProviderOperationStage,
    SessionReachability, SessionRef, UuidIdentity, render_message,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, DeliveryPrecondition, DeliveryRequest,
    EndpointDirectory, ProviderConversationBackend, ProviderOperationStore, RouteClaim,
    SessionDeliveryRoute,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::Mutex;

pub struct ProviderAcpDeliveryRoute {
    service_id: UuidIdentity,
    claim: ProviderAcpRouteClaim,
    supervisor: Arc<ExternalProviderSupervisor>,
    store: Arc<Mutex<ProviderOperationStore>>,
    ownership: Arc<dyn LiveSessionOwnershipCheck>,
    queue: ProviderAcpMessageFifo,
    session_locks: StdMutex<HashMap<SessionRef, Arc<Mutex<()>>>>,
}

impl ProviderAcpDeliveryRoute {
    #[must_use]
    pub fn new(
        service_id: UuidIdentity,
        directory: EndpointDirectory,
        supervisor: Arc<ExternalProviderSupervisor>,
        store: Arc<Mutex<ProviderOperationStore>>,
        ownership: Arc<dyn LiveSessionOwnershipCheck>,
    ) -> Self {
        let claim = ProviderAcpRouteClaim::new(
            service_id.clone(),
            directory,
            Arc::clone(&supervisor),
            Arc::clone(&store),
        );
        let queue = ProviderAcpMessageFifo::new(
            Arc::clone(&supervisor),
            Arc::clone(&store),
            Arc::clone(&ownership),
        );
        Self {
            service_id,
            claim,
            supervisor,
            store,
            ownership,
            queue,
            session_locks: StdMutex::new(HashMap::new()),
        }
    }

    fn session_lock(&self, target: &SessionRef) -> Result<Arc<Mutex<()>>, DeliveryContractError> {
        let mut locks = self
            .session_locks
            .lock()
            .map_err(|_| DeliveryContractError::ClientOperation)?;
        Ok(Arc::clone(
            locks
                .entry(target.clone())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        ))
    }

    pub async fn shutdown_queue(&self) {
        self.queue.shutdown().await;
    }

    fn serves(&self, target: &SessionRef) -> bool {
        target.endpoint.service_id == self.service_id
            && matches!(
                String::from(target.endpoint.endpoint_id.clone()).as_str(),
                "claude-local" | "cursor-local"
            )
    }

    fn is_cursor(target: &SessionRef) -> bool {
        String::from(target.endpoint.endpoint_id.clone()) == "cursor-local"
    }

    fn receipt(outcome: DeliveryOutcome, operation_id: Option<OperationId>) -> DeliveryReceipt {
        DeliveryReceipt {
            outcome,
            reachability: Some(SessionReachability::ProviderAcp),
            client: operation_id
                .map(|operation_id| DeliveryClientReceipt::ProviderAcp { operation_id }),
        }
    }

    fn rejected(reason: DeliveryRejectionReason, detail: &str) -> DeliveryReceipt {
        let next_action = match reason {
            DeliveryRejectionReason::Busy | DeliveryRejectionReason::EndpointUnavailable => {
                DeliveryNextAction::RetryLater
            }
            DeliveryRejectionReason::LiveElsewhere => DeliveryNextAction::InspectTarget,
            _ => DeliveryNextAction::CorrectRequest,
        };
        Self::receipt(
            DeliveryOutcome::Rejected(DeliveryRejection {
                reason,
                next_action,
                client_code: None,
                detail: Some(detail.to_owned()),
            }),
            None,
        )
    }

    fn not_submitted(reason: impl Into<String>, retryable: bool) -> DeliveryReceipt {
        Self::receipt(
            DeliveryOutcome::NotSubmitted {
                reason: reason.into(),
                retryable,
            },
            None,
        )
    }

    fn effect(
        request: &DeliveryRequest,
        binding: &collaboration_protocol::ProviderBindingIdentity,
        submission: SubmissionEffect,
    ) -> Result<RouteEffectEvidence<SessionRef, CodexGeneration>, DeliveryContractError> {
        let reference =
            ProviderBindingReference::try_from(String::from(binding.binding_id.clone()))
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
        Ok(RouteEffectEvidence::ProviderAcp(
            ProviderAcpEffectEvidence {
                target: request.target.clone(),
                generation: binding.generation.clone(),
                binding: reference,
                attempt_id: request.attempt.clone(),
                submission,
                settlement: ProviderSettlementEffect::NotObserved,
            },
        ))
    }

    async fn deliver_provider<'a>(
        &'a self,
        request: DeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        if !self.serves(&request.target) {
            return Ok(Self::rejected(
                DeliveryRejectionReason::NoRoute,
                "provider route does not serve this endpoint",
            ));
        }
        if Self::is_cursor(&request.target) && request.mode == MessageDelivery::Steer {
            return Ok(Self::rejected(
                DeliveryRejectionReason::SteerUnsupported,
                "steer unsupported by Cursor",
            ));
        }
        let Some(binding) = self.supervisor.binding(&request.target.endpoint) else {
            return Ok(Self::not_submitted(
                "provider runtime is unavailable",
                false,
            ));
        };
        if let DeliveryPrecondition::EndpointGeneration { expected } = &request.precondition
            && expected != &binding.generation
        {
            return Ok(Self::rejected(
                DeliveryRejectionReason::StaleGeneration,
                "provider binding generation changed",
            ));
        }
        let session_lock = self.session_lock(&request.target)?;
        let _session_guard = session_lock.lock().await;
        let claim = self.claim.claim(&request.target).await;
        match claim {
            RouteClaim::Holds | RouteClaim::CanLoad => {}
            RouteClaim::Unavailable { reason, retryable } => {
                return Ok(Self::not_submitted(reason.reason, retryable));
            }
            RouteClaim::LiveElsewhere { .. } => {
                return Ok(Self::not_submitted("session is live elsewhere", true));
            }
            RouteClaim::NotMine => {
                return Ok(Self::rejected(
                    DeliveryRejectionReason::NoRoute,
                    "provider has no recorded session",
                ));
            }
        }
        let operation_id = OperationId::try_from(request.attempt.as_str().to_owned())
            .map_err(|_| DeliveryContractError::InvalidEvidence)?;
        let Some(runtime) = self.supervisor.runtime_for(&request.target.endpoint) else {
            return Ok(Self::not_submitted("provider runtime is unavailable", true));
        };
        let activity = match runtime
            .session_activity(String::from(request.target.session_id.clone()))
            .await
        {
            Ok(activity) => activity,
            Err(_) => {
                return Ok(Self::not_submitted(
                    "provider activity is unavailable",
                    true,
                ));
            }
        };
        if request.mode == MessageDelivery::Queue
            || (Self::is_cursor(&request.target)
                && request.mode == MessageDelivery::Auto
                && activity == ProviderSessionActivity::Running)
        {
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
                MessageContent::HumanUser { .. } | MessageContent::Router { .. } => {
                    record.created_by
                }
            };
            let permit = match self.queue.reserve(&request.target) {
                Ok(permit) => permit,
                Err(reason) => return Ok(Self::rejected(DeliveryRejectionReason::Busy, reason)),
            };
            let effect = Self::effect(&request, &binding, SubmissionEffect::RouterQueued)?;
            sink.record(effect).await?;
            permit.send(ConversationPromptRequest {
                operation_id: operation_id.clone(),
                target: request.target.clone(),
                generation: binding.generation,
                requested_by,
                approver: record.approver,
                prompt: request.message,
            });
            return Ok(Self::receipt(DeliveryOutcome::Queued, Some(operation_id)));
        }
        let mut effect = Self::effect(&request, &binding, SubmissionEffect::Dispatching)?;
        sink.record(effect.clone()).await?;
        match ensure_provider_session_loaded(
            &self.supervisor,
            &self.store,
            self.ownership.as_ref(),
            &request.target,
        )
        .await
        {
            ProviderSessionLoadOutcome::Ready => {}
            ProviderSessionLoadOutcome::MissingRecord => {
                return self
                    .finish_known_none(
                        &request,
                        sink,
                        &mut effect,
                        "provider session record is missing",
                        false,
                    )
                    .await;
            }
            ProviderSessionLoadOutcome::LiveElsewhere => {
                return self
                    .finish_known_none(
                        &request,
                        sink,
                        &mut effect,
                        "session became live elsewhere",
                        true,
                    )
                    .await;
            }
            ProviderSessionLoadOutcome::Unavailable { reason } => {
                return self
                    .finish_known_none(&request, sink, &mut effect, reason, true)
                    .await;
            }
        }
        let activity = match runtime
            .session_activity(String::from(request.target.session_id.clone()))
            .await
        {
            Ok(activity) => activity,
            Err(_) => return self.finish_unknown(sink, &mut effect).await,
        };
        if Self::is_cursor(&request.target) && activity == ProviderSessionActivity::Running {
            return self
                .finish_known_none(
                    &request,
                    sink,
                    &mut effect,
                    "provider session became busy",
                    true,
                )
                .await;
        }
        if !Self::is_cursor(&request.target) {
            let prompt = match render_message(&request.target, &request.message) {
                Ok(prompt) => prompt,
                Err(_) => {
                    return self
                        .finish_known_none(
                            &request,
                            sink,
                            &mut effect,
                            "provider prompt exceeds the frame limit",
                            false,
                        )
                        .await;
                }
            };
            match runtime
                .steer_session(String::from(request.target.session_id.clone()), prompt.text)
                .await
            {
                Ok(ProviderSteeringOutcome::Injected {
                    running_operation_id: Some(running),
                }) => {
                    update_submission(&mut effect, SubmissionEffect::Accepted);
                    if sink.record(effect).await.is_err() {
                        return Ok(Self::receipt(DeliveryOutcome::Unknown, None));
                    }
                    return Ok(Self::receipt(DeliveryOutcome::Steered, Some(running)));
                }
                Ok(ProviderSteeringOutcome::Injected {
                    running_operation_id: None,
                }) => return self.finish_unknown(sink, &mut effect).await,
                Ok(ProviderSteeringOutcome::PromptRequired)
                    if request.mode == MessageDelivery::Steer =>
                {
                    return self
                        .finish_known_none(&request, sink, &mut effect, "no running turn", false)
                        .await;
                }
                Ok(ProviderSteeringOutcome::PromptRequired) => {}
                Err(_) => return self.finish_unknown(sink, &mut effect).await,
            }
        }
        let record = match self
            .store
            .lock()
            .await
            .session_record(&request.target)
            .await
        {
            Ok(Some(record)) => record,
            Ok(None) => {
                return self
                    .finish_known_none(
                        &request,
                        sink,
                        &mut effect,
                        "provider session record is missing",
                        false,
                    )
                    .await;
            }
            Err(_) => return self.finish_unknown(sink, &mut effect).await,
        };
        let requested_by = match &request.message {
            MessageContent::Agent { sender, .. } => sender.clone(),
            MessageContent::HumanUser { .. } | MessageContent::Router { .. } => record.created_by,
        };
        let dispatch = self
            .supervisor
            .submit_delivery_prompt(ConversationPromptRequest {
                operation_id: operation_id.clone(),
                target: request.target.clone(),
                generation: binding.generation,
                requested_by,
                approver: record.approver,
                prompt: request.message.clone(),
            })
            .await;
        match dispatch {
            Ok(ProviderPromptDispatch::Submitted) => {
                update_submission(&mut effect, SubmissionEffect::Accepted);
                if sink.record(effect).await.is_err() {
                    return Ok(Self::receipt(DeliveryOutcome::Unknown, None));
                }
                Ok(Self::receipt(DeliveryOutcome::Started, Some(operation_id)))
            }
            Ok(ProviderPromptDispatch::NotSubmitted) => {
                self.finish_known_none(
                    &request,
                    sink,
                    &mut effect,
                    "provider prompt was not sent",
                    true,
                )
                .await
            }
            Ok(ProviderPromptDispatch::Existing | ProviderPromptDispatch::Uncertain) => {
                self.finish_unknown(sink, &mut effect).await
            }
            Err(failure) if failure.effect == ProviderOperationEffect::None => {
                self.finish_known_none(
                    &request,
                    sink,
                    &mut effect,
                    String::from(failure.message),
                    true,
                )
                .await
            }
            Err(_) => self.finish_unknown(sink, &mut effect).await,
        }
    }

    async fn finish_known_none(
        &self,
        _request: &DeliveryRequest,
        sink: &dyn AttemptEvidenceSink,
        effect: &mut RouteEffectEvidence<SessionRef, CodexGeneration>,
        reason: impl Into<String>,
        retryable: bool,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        update_submission(effect, SubmissionEffect::NotDispatched);
        if sink.record(effect.clone()).await.is_err() {
            return Ok(Self::receipt(DeliveryOutcome::Unknown, None));
        }
        Ok(Self::not_submitted(reason, retryable))
    }

    async fn finish_unknown(
        &self,
        sink: &dyn AttemptEvidenceSink,
        effect: &mut RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        update_submission(effect, SubmissionEffect::Unknown);
        let _result = sink.record(effect.clone()).await;
        Ok(Self::receipt(DeliveryOutcome::Unknown, None))
    }
}

fn update_submission(
    effect: &mut RouteEffectEvidence<SessionRef, CodexGeneration>,
    submission: SubmissionEffect,
) {
    if let RouteEffectEvidence::ProviderAcp(provider) = effect {
        provider.submission = submission;
    }
}

impl SessionDeliveryRoute for ProviderAcpDeliveryRoute {
    fn scheduled_runs(&self) -> Option<Arc<dyn collaboration_service::ScheduledRunRoute>> {
        Some(Arc::new(
            crate::provider_acp_scheduled_runs::ProviderAcpScheduledRuns::new(
                self.service_id.clone(),
                Arc::clone(&self.supervisor),
                Arc::clone(&self.store),
                Arc::clone(&self.ownership),
            ),
        ))
    }

    fn reachability(&self) -> SessionReachability {
        SessionReachability::ProviderAcp
    }

    fn claim(&self, target: &SessionRef) -> DeliveryFuture<'_, RouteClaim> {
        let target = target.clone();
        Box::pin(async move { Ok(self.claim.claim(&target).await) })
    }

    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move { self.deliver_provider(request, sink).await })
    }

    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async move {
            let RouteEffectEvidence::ProviderAcp(provider) = context.recorded else {
                return Err(DeliveryContractError::InvalidEvidence);
            };
            if provider.target != context.target {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            let operation_id = OperationId::try_from(provider.attempt_id.as_str().to_owned())
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
            let record = self
                .store
                .lock()
                .await
                .inspect(&operation_id)
                .await
                .map_err(|_| DeliveryContractError::ClientOperation)?;
            let Some(record) = record else {
                return Ok(if provider.submission == SubmissionEffect::RouterQueued {
                    AttemptReconciliation::KnownNotSubmitted
                } else {
                    AttemptReconciliation::StillUnknown
                });
            };
            if record.binding.generation != provider.generation
                || String::from(record.binding.binding_id) != provider.binding.as_str()
            {
                return Ok(AttemptReconciliation::KnownNotSubmitted);
            }
            Ok(match (record.stage, record.effect) {
                (ProviderOperationStage::Terminal, ProviderOperationEffect::Applied) => {
                    let outcome = if provider.submission == SubmissionEffect::RouterQueued {
                        DeliveryOutcome::Queued
                    } else {
                        DeliveryOutcome::Started
                    };
                    AttemptReconciliation::Accepted(Box::new(Self::receipt(
                        outcome,
                        Some(operation_id),
                    )))
                }
                (ProviderOperationStage::Terminal, ProviderOperationEffect::None) => {
                    AttemptReconciliation::KnownNotSubmitted
                }
                _ => AttemptReconciliation::StillUnknown,
            })
        })
    }
}
