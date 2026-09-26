//! Scheduled runs select a client once, then dispatch by their recorded route evidence.
use crate::{
    DeliveryContractError, DeliveryFuture, FreshSessionRequest, PreparationEvidenceSink,
    PreparedTarget, RouteClaim, RunEvidenceSink, RunObservationContext, RunReconciliation,
    RunSettlement, RunSubmission, ScheduleDestination, SchedulePreparationOutcome,
    SchedulePreparationRequest, ScheduleSupport, ScheduledRunExecution, ScheduledRunRoute,
    ScheduledRunSubmission, SessionDeliveryRouter, StopRequestOutcome,
};
use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{EndpointRef, SessionReachability, SessionRef};
use futures_util::future::join_all;
use std::sync::Arc;

impl SessionDeliveryRouter {
    async fn scheduled_for_existing(
        &self,
        target: &SessionRef,
    ) -> Result<Arc<dyn ScheduledRunRoute>, DeliveryContractError> {
        let claims = join_all(self.routes.iter().map(|route| route.claim(target)))
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let selected = claims
            .iter()
            .position(|claim| matches!(claim, RouteClaim::Holds))
            .or_else(|| {
                if claims
                    .iter()
                    .any(|claim| matches!(claim, RouteClaim::LiveElsewhere { .. }))
                {
                    None
                } else {
                    claims
                        .iter()
                        .position(|claim| matches!(claim, RouteClaim::CanLoad))
                }
            })
            .ok_or(DeliveryContractError::ClientOperation)?;
        self.routes
            .get(selected)
            .ok_or(DeliveryContractError::InvalidEvidence)?
            .scheduled_runs()
            .ok_or(DeliveryContractError::ClientOperation)
    }

    async fn scheduled_for_fresh(
        &self,
        endpoint: &EndpointRef,
    ) -> Result<Arc<dyn ScheduledRunRoute>, DeliveryContractError> {
        for route in &self.routes {
            let Some(scheduled) = route.scheduled_runs() else {
                continue;
            };
            if scheduled.supports_endpoint(endpoint) {
                return Ok(scheduled);
            }
        }
        Err(DeliveryContractError::ClientOperation)
    }

    fn scheduled_for_recorded(
        &self,
        evidence: &RouteEffectEvidence<SessionRef, collaboration_protocol::CodexGeneration>,
    ) -> Result<Arc<dyn ScheduledRunRoute>, DeliveryContractError> {
        let reachability = match evidence {
            RouteEffectEvidence::CodexAppServer(_) => SessionReachability::CodexAppServer,
            RouteEffectEvidence::ProviderAcp(_) => SessionReachability::ProviderAcp,
            RouteEffectEvidence::ClaudeCodePeer(_) => SessionReachability::ClaudeCodePeer,
        };
        self.routes
            .iter()
            .find(|route| route.reachability() == reachability)
            .and_then(|route| route.scheduled_runs())
            .ok_or(DeliveryContractError::ClientOperation)
    }
}

impl ScheduledRunExecution for SessionDeliveryRouter {
    fn prepare_destination<'a>(
        &'a self,
        request: SchedulePreparationRequest,
        sink: &'a dyn PreparationEvidenceSink,
    ) -> DeliveryFuture<'a, SchedulePreparationOutcome> {
        Box::pin(async move {
            let route = match &request.destination {
                collaboration_protocol::DestinationPreparation::Existing { target, .. } => {
                    self.scheduled_for_existing(target).await?
                }
                collaboration_protocol::DestinationPreparation::Fresh { endpoint, .. } => {
                    self.scheduled_for_fresh(endpoint).await?
                }
                collaboration_protocol::DestinationPreparation::Fork { source, .. } => {
                    self.scheduled_for_existing(source).await?
                }
            };
            route.prepare_destination(request, sink).await
        })
    }

    fn initial_evidence(
        &self,
        destination: &ScheduleDestination,
    ) -> DeliveryFuture<'_, RouteEffectEvidence<SessionRef, collaboration_protocol::CodexGeneration>>
    {
        let destination = destination.clone();
        Box::pin(async move {
            let route = match &destination {
                ScheduleDestination::Existing { target } => {
                    self.scheduled_for_existing(target).await?
                }
                ScheduleDestination::Fresh { endpoint } => {
                    self.scheduled_for_fresh(endpoint).await?
                }
                ScheduleDestination::Fork { source, .. } => {
                    self.scheduled_for_existing(source).await?
                }
            };
            route.initial_evidence(&destination).await
        })
    }

    fn support(&self, destination: &ScheduleDestination) -> DeliveryFuture<'_, ScheduleSupport> {
        let destination = destination.clone();
        Box::pin(async move {
            let route = match &destination {
                ScheduleDestination::Existing { target } => {
                    self.scheduled_for_existing(target).await?
                }
                ScheduleDestination::Fresh { endpoint } => {
                    self.scheduled_for_fresh(endpoint).await?
                }
                ScheduleDestination::Fork { source, .. } => {
                    self.scheduled_for_existing(source).await?
                }
            };
            route.support(&destination).await
        })
    }

    fn prepare_existing_target<'a>(
        &'a self,
        target: &SessionRef,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        let target = target.clone();
        Box::pin(async move {
            let route = self.scheduled_for_existing(&target).await?;
            route.prepare_existing_target(&target, sink).await
        })
    }

    fn prepare_fresh_session<'a>(
        &'a self,
        request: FreshSessionRequest,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        Box::pin(async move {
            let route = self.scheduled_for_fresh(&request.endpoint).await?;
            route.prepare_fresh_session(request, sink).await
        })
    }

    fn submit_run<'a>(
        &'a self,
        run: ScheduledRunSubmission,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, RunSubmission> {
        Box::pin(async move {
            let route = self.scheduled_for_recorded(&run.recorded)?;
            route.submit_run(run, sink).await
        })
    }

    fn observe_settlement(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunSettlement> {
        Box::pin(async move {
            let route = self.scheduled_for_recorded(&context.recorded)?;
            route.observe_settlement(context).await
        })
    }

    fn summary_source(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, crate::RunSummarySource> {
        Box::pin(async move {
            let route = self.scheduled_for_recorded(&context.recorded)?;
            route.summary_source(context).await
        })
    }

    fn request_stop<'a>(
        &'a self,
        context: RunObservationContext,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, StopRequestOutcome> {
        Box::pin(async move {
            let route = self.scheduled_for_recorded(&context.recorded)?;
            route.request_stop(context, sink).await
        })
    }

    fn reconcile_run(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunReconciliation> {
        Box::pin(async move {
            let route = self.scheduled_for_recorded(&context.recorded)?;
            route.reconcile_run(context).await
        })
    }
}
