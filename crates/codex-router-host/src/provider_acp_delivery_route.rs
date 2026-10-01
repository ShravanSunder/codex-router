//! ACP provider message delivery, evidence, and reconciliation.
mod provider_active_turn_cancel;
mod provider_content_commands;
mod provider_delivery_request;
mod provider_queue_submission;
mod session_delivery_route;
pub use provider_active_turn_cancel::ProviderCancelActiveTurnError;
pub use provider_content_commands::{
    ProviderPromptContentsError, ProviderQueueAdmissionError, ProviderSteerContentsError,
};
use provider_delivery_request::{ProviderDeliveryContent, ProviderDeliveryRequest};

use crate::external_provider_supervisor::{ProviderPromptContentsRequest, ProviderPromptDispatch};
use crate::provider_acp_message_fifo::{ProviderAcpMessageFifo, ProviderQueuedPrompt};
use crate::provider_acp_route_claim::ProviderAcpRouteClaim;
use crate::provider_acp_session_loading::{
    ProviderSessionLoadOutcome, ProviderSessionLoadRejection, ensure_provider_session_loaded,
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
    CodexGeneration, DeliveryClientReceipt, DeliveryNextAction, DeliveryOutcome, DeliveryReceipt,
    DeliveryRejection, DeliveryRejectionReason, MessageDelivery, OperationId, ProviderIdentity,
    ProviderOperationEffect, SessionReachability, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, DeliveryContractError, DeliveryPrecondition, EndpointDirectory,
    LoadPolicy, NOT_LOADED_REASON, ProviderConversationBackend, ProviderOperationStore, RouteClaim,
};
use std::{
    collections::{HashMap, HashSet},
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
    pub fn queue_list(&self, session: &SessionRef) -> Vec<crate::ProviderQueuedInput> {
        self.supervisor.queued_operation_registry().list(session)
    }

    pub fn queue_cancel(
        &self,
        session: &SessionRef,
        input_id: &session_event_model::InputId,
    ) -> Result<(), crate::ProviderQueueCancellationError> {
        self.supervisor
            .queued_operation_registry()
            .cancel(session, input_id)
    }

    #[must_use]
    pub fn new(
        service_id: UuidIdentity,
        provider_endpoints: HashSet<collaboration_protocol::EndpointRef>,
        directory: EndpointDirectory,
        supervisor: Arc<ExternalProviderSupervisor>,
        store: Arc<Mutex<ProviderOperationStore>>,
        ownership: Arc<dyn LiveSessionOwnershipCheck>,
    ) -> Self {
        let claim = ProviderAcpRouteClaim::new(
            service_id.clone(),
            provider_endpoints,
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
        self.claim.serves(target)
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
                claims: None,
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
        request: &ProviderDeliveryRequest,
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
        request: ProviderDeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        let input_id = request.input_id()?;
        if !self.serves(&request.target) {
            return Ok(Self::rejected(
                DeliveryRejectionReason::NoRoute,
                "provider route does not serve this endpoint",
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
            RouteClaim::Holds => {}
            RouteClaim::CanLoad => {}
            RouteClaim::Unavailable { reason, retryable } => {
                return Ok(Self::not_submitted(reason.reason, retryable));
            }
            RouteClaim::Rejected { .. } | RouteClaim::LiveElsewhere { .. } => {
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
        let provider_session_id = String::from(request.target.session_id.clone());
        if runtime.settings_unresolved(&provider_session_id).await {
            return Ok(Self::rejected(
                DeliveryRejectionReason::SettingsUnresolved,
                "settingsUnresolved: set requested settings or accept current values before prompt, steer or queue",
            ));
        }
        let capabilities = runtime.capability_report(&provider_session_id).await;
        if request.mode == MessageDelivery::Steer && !capabilities.supports_steering {
            return Ok(Self::rejected(
                DeliveryRejectionReason::SteerUnsupported,
                "steer unsupported by provider",
            ));
        }
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
            && request.load_policy == LoadPolicy::LoadedOnly
            && activity == ProviderSessionActivity::NotLoaded
        {
            return Ok(Self::not_submitted(NOT_LOADED_REASON, true));
        }
        if request.mode == MessageDelivery::Queue
            || (!capabilities.supports_steering
                && request.mode == MessageDelivery::Auto
                && activity == ProviderSessionActivity::Running)
        {
            return self
                .queue_delivery(&request, sink, &binding, &operation_id)
                .await;
        }
        let mut effect = Self::effect(&request, &binding, SubmissionEffect::Dispatching)?;
        sink.record(effect.clone()).await?;
        match ensure_provider_session_loaded(
            &self.supervisor,
            &self.store,
            self.ownership.as_ref(),
            &request.target,
            request.load_policy,
        )
        .await
        {
            ProviderSessionLoadOutcome::Ready | ProviderSessionLoadOutcome::AlreadyLoaded => {}
            ProviderSessionLoadOutcome::NotLoaded => {
                return self
                    .finish_known_none(&request, sink, &mut effect, NOT_LOADED_REASON, true)
                    .await;
            }
            ProviderSessionLoadOutcome::UnsupportedLoad => {
                return self
                    .finish_known_none(&request, sink, &mut effect, "unsupported: load", false)
                    .await;
            }
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
            ProviderSessionLoadOutcome::Rejected { reason } => {
                return self.finish_rejected(sink, &mut effect, reason).await;
            }
        }
        let activity = match runtime
            .session_activity(String::from(request.target.session_id.clone()))
            .await
        {
            Ok(activity) => activity,
            Err(_) => return self.finish_unknown(sink, &mut effect).await,
        };
        if !capabilities.supports_steering && activity == ProviderSessionActivity::Running {
            return self
                .queue_delivery(&request, sink, &binding, &operation_id)
                .await;
        }
        if capabilities.supports_steering {
            let prompt_text = match &request.content {
                ProviderDeliveryContent::PreparedPush { line, .. } => line.as_str().to_owned(),
            };
            match runtime
                .steer_session_with_input(
                    String::from(request.target.session_id.clone()),
                    input_id.clone(),
                    prompt_text,
                )
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
                }) => {
                    update_submission(&mut effect, SubmissionEffect::Accepted);
                    if sink.record(effect).await.is_err() {
                        return Ok(Self::receipt(DeliveryOutcome::Unknown, None));
                    }
                    return Ok(Self::receipt(DeliveryOutcome::Steered, None));
                }
                Ok(ProviderSteeringOutcome::StartedNewTurn) => {
                    update_submission(&mut effect, SubmissionEffect::Accepted);
                    if sink.record(effect).await.is_err() {
                        return Ok(Self::receipt(DeliveryOutcome::Unknown, None));
                    }
                    tracing::info!(
                        session_id = %String::from(request.target.session_id.clone()),
                        "agent started a new turn from a steer"
                    );
                    return Ok(Self::receipt(DeliveryOutcome::Started, None));
                }
                Ok(ProviderSteeringOutcome::Failed) => {
                    return self
                        .finish_known_none(&request, sink, &mut effect, "steerFailed", false)
                        .await;
                }
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
        let requested_by = match &request.content {
            ProviderDeliveryContent::PreparedPush { .. } => record.created_by.clone(),
        };
        let dispatch = match &request.content {
            ProviderDeliveryContent::PreparedPush { line, .. } => {
                let contents_request = ProviderPromptContentsRequest::from_prepared_push(
                    operation_id.clone(),
                    input_id,
                    request.target.clone(),
                    requested_by,
                    record.approver,
                    line,
                )?;
                self.supervisor
                    .submit_delivery_prompt_contents(contents_request)
                    .await
            }
        };
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
        _request: &ProviderDeliveryRequest,
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

    async fn finish_rejected(
        &self,
        sink: &dyn AttemptEvidenceSink,
        effect: &mut RouteEffectEvidence<SessionRef, CodexGeneration>,
        provider_reason: ProviderSessionLoadRejection,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        update_submission(effect, SubmissionEffect::NotDispatched);
        if sink.record(effect.clone()).await.is_err() {
            return Ok(Self::receipt(DeliveryOutcome::Unknown, None));
        }
        let client_code = Some(provider_reason.code());
        let detail = Some(provider_reason.safe_detail());
        let (reason, next_action) = match provider_reason {
            ProviderSessionLoadRejection::SessionNotFound { .. } => (
                DeliveryRejectionReason::ProviderSessionNotFound,
                DeliveryNextAction::CorrectRequest,
            ),
            ProviderSessionLoadRejection::ProviderRejected { .. } => (
                DeliveryRejectionReason::ProviderRejected,
                DeliveryNextAction::InspectTarget,
            ),
        };
        Ok(Self::receipt(
            DeliveryOutcome::Rejected(DeliveryRejection {
                reason,
                next_action,
                client_code,
                detail,
                claims: None,
            }),
            None,
        ))
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
