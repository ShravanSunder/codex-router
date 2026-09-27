//! Provider ACP scheduled-run policy over recorded provider operations.
use crate::{ExternalProviderSupervisor, LiveSessionOwnershipCheck};
use collaboration_protocol::UuidIdentity;
use collaboration_service::ProviderOperationStore;
use std::sync::Arc;
use tokio::sync::Mutex;

mod provider_acp_run_observation;

pub struct ProviderAcpScheduledRuns {
    service_id: UuidIdentity,
    supervisor: Arc<ExternalProviderSupervisor>,
    store: Arc<Mutex<ProviderOperationStore>>,
    ownership: Arc<dyn LiveSessionOwnershipCheck>,
}

impl ProviderAcpScheduledRuns {
    pub(crate) fn new(
        service_id: UuidIdentity,
        supervisor: Arc<ExternalProviderSupervisor>,
        store: Arc<Mutex<ProviderOperationStore>>,
        ownership: Arc<dyn LiveSessionOwnershipCheck>,
    ) -> Self {
        Self {
            service_id,
            supervisor,
            store,
            ownership,
        }
    }
}

mod provider_acp_run_dispatch;

use crate::provider_acp_session_loading::{
    ProviderSessionLoadOutcome, ensure_provider_session_loaded,
};
use agent_automation::{
    AttemptId, ProviderAcpEffectEvidence, ProviderBindingReference, ProviderSettlementEffect,
    RouteEffectEvidence, SubmissionEffect,
};
use collaboration_protocol::{
    CodexGeneration, DestinationPreparation, EndpointRef, ProviderCapabilityName,
    ProviderCapabilityStatus, ScheduleFailureKind, SessionRef,
};
use collaboration_service::{
    DeliveryContractError, DeliveryFuture, FreshSessionRequest, PreparationEvidenceSink,
    PreparedTarget, ProviderConversationBackend, RunEvidenceDisposition, RunEvidenceSink,
    RunObservationContext, RunReconciliation, RunSettlement, RunSubmission, RunSummarySource,
    ScheduleCapability, ScheduleDestination, SchedulePreparationFailure,
    SchedulePreparationOutcome, SchedulePreparationRequest, ScheduleSupport, ScheduledRunExecution,
    ScheduledRunRoute, ScheduledRunSubmission, SettlementEvidence, StopRequestOutcome,
};

impl ProviderAcpScheduledRuns {
    fn serves(&self, endpoint: &EndpointRef) -> bool {
        endpoint.service_id == self.service_id && self.supervisor.binding(endpoint).is_some()
    }

    fn evidence_for(
        &self,
        target: &SessionRef,
        attempt_id: AttemptId,
    ) -> Result<RouteEffectEvidence<SessionRef, CodexGeneration>, DeliveryContractError> {
        if !self.serves(&target.endpoint) {
            return Err(DeliveryContractError::ClientOperation);
        }
        let binding = self
            .supervisor
            .binding(&target.endpoint)
            .ok_or(DeliveryContractError::ClientOperation)?;
        let reference = ProviderBindingReference::try_from(String::from(binding.binding_id))
            .map_err(|_| DeliveryContractError::InvalidEvidence)?;
        Ok(RouteEffectEvidence::ProviderAcp(
            ProviderAcpEffectEvidence {
                target: target.clone(),
                generation: binding.generation,
                binding: reference,
                attempt_id,
                submission: SubmissionEffect::NotDispatched,
                settlement: ProviderSettlementEffect::NotObserved,
            },
        ))
    }

    fn has_capability(&self, endpoint: &EndpointRef, name: ProviderCapabilityName) -> bool {
        self.supervisor.binding(endpoint).is_some_and(|binding| {
            binding.capabilities.as_slice().iter().any(|capability| {
                capability.name == name && capability.status == ProviderCapabilityStatus::Supported
            })
        })
    }
}

impl ScheduledRunRoute for ProviderAcpScheduledRuns {
    fn supports_endpoint(&self, endpoint: &EndpointRef) -> bool {
        self.serves(endpoint)
    }
}

impl ScheduledRunExecution for ProviderAcpScheduledRuns {
    fn support(&self, destination: &ScheduleDestination) -> DeliveryFuture<'_, ScheduleSupport> {
        let destination = destination.clone();
        Box::pin(async move {
            let endpoint = match &destination {
                ScheduleDestination::Existing { target } => &target.endpoint,
                ScheduleDestination::Fresh { endpoint } => endpoint,
                ScheduleDestination::Fork { source, .. } => &source.endpoint,
            };
            let mut missing = Vec::new();
            if !self.serves(endpoint) {
                missing.push(ScheduleCapability::ReadTarget);
                missing.push(ScheduleCapability::StartRun);
            } else {
                if let ScheduleDestination::Existing { target } = &destination
                    && !self.has_capability(endpoint, ProviderCapabilityName::Load)
                {
                    let loaded = if let Some(runtime) = self.supervisor.runtime_for(endpoint) {
                        matches!(
                            runtime
                                .session_activity(String::from(target.session_id.clone()))
                                .await,
                            Ok(crate::ProviderSessionActivity::Idle
                                | crate::ProviderSessionActivity::Running)
                        )
                    } else {
                        false
                    };
                    if !loaded {
                        missing.push(ScheduleCapability::ReadTarget);
                    }
                }
                if !self.has_capability(endpoint, ProviderCapabilityName::Prompt) {
                    missing.push(ScheduleCapability::StartRun);
                }
                if !self.has_capability(endpoint, ProviderCapabilityName::Cancel) {
                    missing.push(ScheduleCapability::StopRun);
                }
            }
            if matches!(destination, ScheduleDestination::Fresh { .. }) {
                missing.push(ScheduleCapability::CreateSession);
            }
            if matches!(destination, ScheduleDestination::Fork { .. }) {
                missing.push(ScheduleCapability::ForkSession);
            }
            Ok(if missing.is_empty() {
                ScheduleSupport::Supported {
                    settlement: SettlementEvidence::OperationSettlement,
                }
            } else {
                ScheduleSupport::Unsupported { missing }
            })
        })
    }

    fn initial_evidence(
        &self,
        destination: &ScheduleDestination,
    ) -> DeliveryFuture<'_, RouteEffectEvidence<SessionRef, CodexGeneration>> {
        let destination = destination.clone();
        Box::pin(async move {
            match destination {
                ScheduleDestination::Existing { target } => {
                    self.evidence_for(&target, AttemptId::generate())
                }
                ScheduleDestination::Fork { source, .. } => {
                    self.evidence_for(&source, AttemptId::generate())
                }
                ScheduleDestination::Fresh { .. } => Err(DeliveryContractError::ClientOperation),
            }
        })
    }

    fn prepare_destination<'a>(
        &'a self,
        request: SchedulePreparationRequest,
        sink: &'a dyn PreparationEvidenceSink,
    ) -> DeliveryFuture<'a, SchedulePreparationOutcome> {
        Box::pin(async move {
            let target = match request.destination {
                DestinationPreparation::Existing { target, .. } => target,
                DestinationPreparation::Fork { source, .. } => {
                    let evidence = self.evidence_for(&source, AttemptId::generate())?;
                    return Ok(SchedulePreparationOutcome::Failed(
                        SchedulePreparationFailure {
                            kind: ScheduleFailureKind::UnsupportedCapability,
                            explanation: "Create the session first (conversation create), then schedule it as an existing target; provider ACP sessions cannot be forked".into(),
                            evidence,
                            uncertain: false,
                        },
                    ));
                }
                DestinationPreparation::Fresh { .. } => {
                    return Err(DeliveryContractError::ClientOperation);
                }
            };
            let attempt_id = AttemptId::try_from(request.operation_id.as_str().to_owned())
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
            let evidence = self.evidence_for(&target, attempt_id)?;
            if !sink.record_intent(evidence.clone()).await? {
                return Ok(SchedulePreparationOutcome::Failed(
                    SchedulePreparationFailure {
                        kind: ScheduleFailureKind::OutcomeUnknown,
                        explanation: "Provider preparation intent could not be established".into(),
                        evidence,
                        uncertain: true,
                    },
                ));
            }
            Ok(
                match ensure_provider_session_loaded(
                    &self.supervisor,
                    &self.store,
                    self.ownership.as_ref(),
                    &target,
                )
                .await
                {
                    ProviderSessionLoadOutcome::Ready => {
                        SchedulePreparationOutcome::Prepared(PreparedTarget { target, evidence })
                    }
                    ProviderSessionLoadOutcome::UnsupportedLoad => {
                        SchedulePreparationOutcome::Failed(SchedulePreparationFailure {
                            kind: ScheduleFailureKind::UnsupportedCapability,
                            explanation: "unsupported: load".into(),
                            evidence,
                            uncertain: false,
                        })
                    }
                    ProviderSessionLoadOutcome::MissingRecord => {
                        SchedulePreparationOutcome::Failed(SchedulePreparationFailure {
                            kind: ScheduleFailureKind::ResourceNotFound,
                            explanation: "Provider session record is missing".into(),
                            evidence,
                            uncertain: false,
                        })
                    }
                    ProviderSessionLoadOutcome::LiveElsewhere => {
                        SchedulePreparationOutcome::Failed(SchedulePreparationFailure {
                            kind: ScheduleFailureKind::OwnershipConflict,
                            explanation: "Provider session became live in Claude Code".into(),
                            evidence,
                            uncertain: false,
                        })
                    }
                    ProviderSessionLoadOutcome::Unavailable { reason } => {
                        SchedulePreparationOutcome::Failed(SchedulePreparationFailure {
                            kind: ScheduleFailureKind::OutcomeUnknown,
                            explanation: reason,
                            evidence,
                            uncertain: true,
                        })
                    }
                    ProviderSessionLoadOutcome::Rejected { reason } => {
                        SchedulePreparationOutcome::Failed(SchedulePreparationFailure {
                            kind: if matches!(reason, crate::provider_acp_session_loading::ProviderSessionLoadRejection::SessionNotFound { .. }) {
                                ScheduleFailureKind::ResourceNotFound
                            } else {
                                ScheduleFailureKind::OutcomeUnknown
                            },
                            explanation: reason.safe_detail(),
                            evidence,
                            uncertain: false,
                        })
                    }
                },
            )
        })
    }

    fn prepare_existing_target<'a>(
        &'a self,
        target: &SessionRef,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        let target = target.clone();
        Box::pin(async move {
            let evidence = self.evidence_for(&target, AttemptId::generate())?;
            if matches!(
                sink.record(evidence.clone()).await?,
                RunEvidenceDisposition::AdmissionRefused
            ) {
                return Err(DeliveryContractError::ClientOperation);
            }
            if !matches!(
                ensure_provider_session_loaded(
                    &self.supervisor,
                    &self.store,
                    self.ownership.as_ref(),
                    &target,
                )
                .await,
                ProviderSessionLoadOutcome::Ready
            ) {
                return Err(DeliveryContractError::ClientOperation);
            }
            Ok(PreparedTarget { target, evidence })
        })
    }

    fn prepare_fresh_session<'a>(
        &'a self,
        _request: FreshSessionRequest,
        _sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        Box::pin(async { Err(DeliveryContractError::ClientOperation) })
    }

    fn submit_run<'a>(
        &'a self,
        run: ScheduledRunSubmission,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, RunSubmission> {
        Box::pin(async move { self.submit_provider_run(run, sink).await })
    }

    fn observe_settlement(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunSettlement> {
        Box::pin(async move { self.observe_provider_run(&context).await })
    }

    fn summary_source(
        &self,
        _context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunSummarySource> {
        Box::pin(async { Err(DeliveryContractError::InvalidEvidence) })
    }

    fn request_stop<'a>(
        &'a self,
        context: RunObservationContext,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, StopRequestOutcome> {
        Box::pin(async move { self.stop_provider_run(&context, sink).await })
    }

    fn reconcile_run(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunReconciliation> {
        Box::pin(async move { self.reconcile_provider_run(&context).await })
    }
}
