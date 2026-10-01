use super::ProviderAcpDeliveryRoute;
use crate::provider_acp_session_loading::{
    ProviderSessionLoadability, inspect_provider_session_loadability,
};
use agent_automation::{RouteEffectEvidence, SubmissionEffect};
use collaboration_protocol::{
    DeliveryOutcome, DeliveryReceipt, OperationId, ProviderOperationEffect, ProviderOperationStage,
    SessionReachability, SessionRef,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, RouteClaim, RoutePresence, SessionDeliveryRoute,
};
use std::sync::Arc;

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

    fn presence(&self, target: &SessionRef) -> DeliveryFuture<'_, RoutePresence> {
        let target = target.clone();
        Box::pin(async move {
            if !self.serves(&target) {
                return Ok(RoutePresence::NotMine);
            }
            let presence = match self.claim.claim(&target).await {
                RouteClaim::NotMine => RoutePresence::NotMine,
                RouteClaim::Holds => RoutePresence::Running,
                RouteClaim::Rejected { rejection } => RoutePresence::LiveElsewhere {
                    detail: rejection.detail,
                },
                RouteClaim::LiveElsewhere { detail, .. } => RoutePresence::LiveElsewhere { detail },
                RouteClaim::Unavailable { reason, .. } => RoutePresence::Unreachable {
                    reason: reason.reason,
                },
                RouteClaim::CanLoad => {
                    let Some(runtime) = self.supervisor.runtime_for(&target.endpoint) else {
                        return Ok(RoutePresence::Unreachable {
                            reason: "provider runtime is unavailable".to_owned(),
                        });
                    };
                    let loadability = inspect_provider_session_loadability(
                        runtime.as_ref(),
                        &self.store,
                        self.ownership.as_ref(),
                        &target,
                    )
                    .await;
                    match loadability {
                        ProviderSessionLoadability::AlreadyLoaded => RoutePresence::Running,
                        ProviderSessionLoadability::Loadable { .. } => RoutePresence::Wakeable,
                        ProviderSessionLoadability::UnsupportedLoad => RoutePresence::Unreachable {
                            reason: "provider does not support loading this session".to_owned(),
                        },
                        ProviderSessionLoadability::MissingRecord => RoutePresence::Unreachable {
                            reason: "provider session record is missing".to_owned(),
                        },
                        ProviderSessionLoadability::LiveElsewhere => RoutePresence::LiveElsewhere {
                            detail: Some("provider session is live elsewhere".to_owned()),
                        },
                        ProviderSessionLoadability::Unavailable { reason } => {
                            RoutePresence::Unreachable { reason }
                        }
                    }
                }
            };
            Ok(presence)
        })
    }

    fn deliver<'a>(
        &'a self,
        request: collaboration_service::layer_zero::DeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move { self.deliver_provider(request.into(), sink).await })
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
            let matching_binding = record.binding.external_provider().is_some_and(|binding| {
                binding.generation == provider.generation
                    && String::from(binding.binding_id.clone()) == provider.binding.as_str()
            });
            if !matching_binding {
                return Ok(AttemptReconciliation::KnownNotSubmitted);
            }
            Ok(match (record.stage, record.effect) {
                (ProviderOperationStage::Terminal, ProviderOperationEffect::Applied) => {
                    let outcome = if provider.submission == SubmissionEffect::RouterQueued {
                        DeliveryOutcome::Queued
                    } else {
                        DeliveryOutcome::Started
                    };
                    AttemptReconciliation::Accepted(Box::new(ProviderAcpDeliveryRoute::receipt(
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
