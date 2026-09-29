//! Selects one injected client route for an attempt, then keeps that choice fixed.
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, DeliveryReceipt, DeliveryRequest, LoadPolicy,
    RouteClaim, RoutePresence, SessionDeliveryRoute, SessionMessageDelivery, TargetPresence,
};
use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    DeliveryNextAction, DeliveryOutcome, DeliveryRejection, DeliveryRejectionReason,
    SessionReachability, SessionRef,
};
use futures_util::future::join_all;
use std::sync::Arc;

pub struct SessionDeliveryRouter {
    pub(crate) routes: Vec<Arc<dyn SessionDeliveryRoute>>,
}

impl SessionDeliveryRouter {
    #[must_use]
    pub fn new(routes: Vec<Arc<dyn SessionDeliveryRoute>>) -> Self {
        Self { routes }
    }

    async fn deliver_once(
        &self,
        request: DeliveryRequest,
        evidence: &dyn AttemptEvidenceSink,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        let claims = join_all(self.routes.iter().map(|route| route.claim(&request.target)))
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(index) = claims
            .iter()
            .position(|claim| matches!(claim, RouteClaim::Holds))
        {
            return self.deliver_through(index, request, evidence).await;
        }
        if let Some(detail) = claims.iter().find_map(|claim| match claim {
            RouteClaim::LiveElsewhere { detail, .. } => detail.as_deref(),
            _ => None,
        }) {
            return Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::Rejected(DeliveryRejection {
                    reason: DeliveryRejectionReason::LiveElsewhere,
                    next_action: DeliveryNextAction::InspectTarget,
                    client_code: None,
                    detail: Some(detail.to_owned()),
                }),
                reachability: None,
                client: None,
            });
        }
        if claims
            .iter()
            .any(|claim| matches!(claim, RouteClaim::LiveElsewhere { .. }))
        {
            return Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::Rejected(DeliveryRejection {
                    reason: DeliveryRejectionReason::LiveElsewhere,
                    next_action: DeliveryNextAction::InspectTarget,
                    client_code: None,
                    detail: Some(
                        "session is live in a Claude Code process Router cannot message".into(),
                    ),
                }),
                reachability: None,
                client: None,
            });
        }
        let loadable_index = claims
            .iter()
            .position(|claim| matches!(claim, RouteClaim::CanLoad));
        if request.load_policy == LoadPolicy::MayLoad
            && let Some(index) = loadable_index
        {
            return self.deliver_through(index, request, evidence).await;
        }
        if let Some(reason) = claims.iter().find_map(|claim| match claim {
            RouteClaim::Unavailable {
                reason,
                retryable: true,
            } => Some(reason),
            _ => None,
        }) {
            return Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    reason: reason.reason.clone(),
                },
                reachability: None,
                client: None,
            });
        }
        if request.load_policy == LoadPolicy::LoadedOnly && loadable_index.is_some() {
            return Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    reason: "notLoaded".into(),
                },
                reachability: None,
                client: None,
            });
        }
        let reasons: Vec<_> = claims
            .iter()
            .filter_map(|claim| match claim {
                RouteClaim::Unavailable { reason, .. } => {
                    Some(format!("{}; {}", reason.reason, reason.fix))
                }
                _ => None,
            })
            .collect();
        Ok(DeliveryReceipt {
            outcome: DeliveryOutcome::Rejected(DeliveryRejection {
                reason: if reasons.is_empty() {
                    DeliveryRejectionReason::NoRoute
                } else {
                    DeliveryRejectionReason::EndpointUnavailable
                },
                next_action: if reasons.is_empty() {
                    DeliveryNextAction::CorrectRequest
                } else {
                    DeliveryNextAction::RetryLater
                },
                client_code: None,
                detail: Some(if reasons.is_empty() {
                    "no route serves this endpoint".into()
                } else {
                    reasons.join("; ")
                }),
            }),
            reachability: None,
            client: None,
        })
    }

    async fn deliver_through(
        &self,
        index: usize,
        request: DeliveryRequest,
        evidence: &dyn AttemptEvidenceSink,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        let route = self
            .routes
            .get(index)
            .ok_or(DeliveryContractError::InvalidEvidence)?;
        let mut receipt = route.deliver(request, evidence).await?;
        receipt.reachability = Some(route.reachability());
        Ok(receipt)
    }
}

impl SessionMessageDelivery for SessionDeliveryRouter {
    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move { self.deliver_once(request, evidence).await })
    }

    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async move {
            let reachability = match &context.recorded {
                RouteEffectEvidence::CodexAppServer(_) => SessionReachability::CodexAppServer,
                RouteEffectEvidence::ProviderAcp(_) => SessionReachability::ProviderAcp,
                RouteEffectEvidence::ClaudeCodePeer(_) => SessionReachability::ClaudeCodePeer,
            };
            let Some(route) = self
                .routes
                .iter()
                .find(|route| route.reachability() == reachability)
            else {
                return Ok(AttemptReconciliation::StillUnknown);
            };
            route.reconcile_attempt(context).await
        })
    }
}

impl SessionDeliveryRouter {
    pub async fn presence(
        &self,
        target: &SessionRef,
    ) -> Result<TargetPresence, DeliveryContractError> {
        let presences = join_all(self.routes.iter().map(|route| route.presence(target)))
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        Ok(aggregate_presence(&presences))
    }
}

fn aggregate_presence(presences: &[RoutePresence]) -> TargetPresence {
    if presences
        .iter()
        .any(|presence| matches!(presence, RoutePresence::Running))
    {
        return TargetPresence::Running;
    }

    let live_elsewhere = presences
        .iter()
        .filter_map(|presence| match presence {
            RoutePresence::LiveElsewhere { detail } => Some(detail),
            _ => None,
        })
        .count();
    let mut unreachable_reasons = presences
        .iter()
        .filter_map(|presence| match presence {
            RoutePresence::Unreachable { reason } => Some(reason.as_str()),
            _ => None,
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if live_elsewhere > 0 {
        unreachable_reasons.push("live elsewhere".to_owned());
        for detail in presences.iter().filter_map(|presence| match presence {
            RoutePresence::LiveElsewhere { detail } => detail.as_deref(),
            _ => None,
        }) {
            unreachable_reasons.push(detail.to_owned());
        }
        return TargetPresence::Unreachable {
            reason: unreachable_reasons.join("; "),
        };
    }

    if presences
        .iter()
        .any(|presence| matches!(presence, RoutePresence::Wakeable))
    {
        return TargetPresence::Wakeable;
    }

    if unreachable_reasons.is_empty() {
        unreachable_reasons.push("no delivery route serves this endpoint".to_owned());
    }
    TargetPresence::Unreachable {
        reason: unreachable_reasons.join("; "),
    }
}
