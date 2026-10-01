//! Selects one injected client route for an attempt, then keeps that choice fixed.
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, DeliveryReceipt, DeliveryRequest, RouteClaim,
    RoutePresence, SessionDeliveryRoute, SessionMessageDelivery, TargetPresence,
    TargetPresenceProbe,
};
use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    DeliveryNextAction, DeliveryOutcome, DeliveryRejection, DeliveryRejectionReason,
    SessionReachability, SessionRef,
};
use futures_util::future::join_all;
use std::sync::Arc;

enum RouteDecision {
    Selected(usize),
    Complete(Box<DeliveryReceipt>),
}

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
        match self.select_route(&request.target).await? {
            RouteDecision::Selected(index) => self.deliver_through(index, request, evidence).await,
            RouteDecision::Complete(receipt) => Ok(*receipt),
        }
    }

    async fn deliver_prepared_once(
        &self,
        request: crate::layer_zero::DeliveryRequest,
        evidence: &dyn AttemptEvidenceSink,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        match self.select_route(&request.target).await? {
            RouteDecision::Selected(index) => {
                self.deliver_prepared_through(index, request, evidence)
                    .await
            }
            RouteDecision::Complete(receipt) => Ok(*receipt),
        }
    }

    async fn select_route(
        &self,
        target: &SessionRef,
    ) -> Result<RouteDecision, DeliveryContractError> {
        let claims = join_all(self.routes.iter().map(|route| route.claim(target)))
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(index) = claims
            .iter()
            .position(|claim| matches!(claim, RouteClaim::Holds))
        {
            return Ok(RouteDecision::Selected(index));
        }
        if let Some(detail) = claims.iter().find_map(|claim| match claim {
            RouteClaim::LiveElsewhere { detail, .. } => detail.as_deref(),
            _ => None,
        }) {
            return Ok(RouteDecision::Complete(Box::new(DeliveryReceipt {
                outcome: DeliveryOutcome::Rejected(DeliveryRejection {
                    reason: DeliveryRejectionReason::LiveElsewhere,
                    next_action: DeliveryNextAction::InspectTarget,
                    client_code: None,
                    detail: Some(detail.to_owned()),
                }),
                reachability: None,
                client: None,
            })));
        }
        if claims
            .iter()
            .any(|claim| matches!(claim, RouteClaim::LiveElsewhere { .. }))
        {
            return Ok(RouteDecision::Complete(Box::new(DeliveryReceipt {
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
            })));
        }
        if let Some(index) = claims
            .iter()
            .position(|claim| matches!(claim, RouteClaim::CanLoad))
        {
            return Ok(RouteDecision::Selected(index));
        }
        if let Some(reason) = claims.iter().find_map(|claim| match claim {
            RouteClaim::Unavailable {
                reason,
                retryable: true,
            } => Some(reason),
            _ => None,
        }) {
            return Ok(RouteDecision::Complete(Box::new(DeliveryReceipt {
                outcome: DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    reason: reason.reason.clone(),
                },
                reachability: None,
                client: None,
            })));
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
        Ok(RouteDecision::Complete(Box::new(DeliveryReceipt {
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
        })))
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

    async fn deliver_prepared_through(
        &self,
        index: usize,
        request: crate::layer_zero::DeliveryRequest,
        evidence: &dyn AttemptEvidenceSink,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        let route = self
            .routes
            .get(index)
            .ok_or(DeliveryContractError::InvalidEvidence)?;
        let mut receipt = route.deliver_prepared(request, evidence).await?;
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

    fn deliver_prepared<'a>(
        &'a self,
        request: crate::layer_zero::DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move { self.deliver_prepared_once(request, evidence).await })
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

impl TargetPresenceProbe for SessionDeliveryRouter {
    fn presence(&self, target: &SessionRef) -> DeliveryFuture<'_, TargetPresence> {
        let target = target.clone();
        Box::pin(async move {
            let presences = join_all(self.routes.iter().map(|route| route.presence(&target)))
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()?;
            Ok(aggregate_presence(&presences))
        })
    }
}

fn aggregate_presence(presences: &[RoutePresence]) -> TargetPresence {
    if presences
        .iter()
        .any(|presence| matches!(presence, RoutePresence::Running))
    {
        return TargetPresence::Running;
    }

    let has_live_elsewhere = presences
        .iter()
        .any(|presence| matches!(presence, RoutePresence::LiveElsewhere { .. }));
    let mut unreachable_reasons = presences
        .iter()
        .filter_map(|presence| match presence {
            RoutePresence::Unreachable { reason } => Some(reason.as_str()),
            _ => None,
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if has_live_elsewhere {
        let mut reasons = vec!["live elsewhere".to_owned()];
        for detail in presences.iter().filter_map(|presence| match presence {
            RoutePresence::LiveElsewhere { detail } => detail.as_deref(),
            _ => None,
        }) {
            reasons.push(detail.to_owned());
        }
        reasons.extend(unreachable_reasons);
        return TargetPresence::Unreachable {
            reason: reasons.join("; "),
        };
    }

    if presences
        .iter()
        .any(|presence| matches!(presence, RoutePresence::Wakeable))
    {
        return TargetPresence::Wakeable;
    }

    if unreachable_reasons.is_empty() {
        unreachable_reasons.push("no route holds or can load this session".to_owned());
    }
    TargetPresence::Unreachable {
        reason: unreachable_reasons.join("; "),
    }
}

#[cfg(test)]
mod tests {
    use super::SessionDeliveryRouter;
    use crate::{
        AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
        DeliveryPrecondition, DeliveryReceipt, DeliveryRequest as LegacyDeliveryRequest,
        LoadPolicy, RouteClaim, RoutePresence, SessionDeliveryRoute, SessionMessageDelivery,
    };
    use agent_automation::AttemptId;
    use collaboration_protocol::{
        DeliveryCorrelationId, DeliveryOutcome, MessageContent, MessageDelivery,
        MessageHeaderContext, MessageText, PushId, SessionReachability, SessionRef,
    };
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CapturingLegacyRoute {
        requests: Mutex<Vec<LegacyDeliveryRequest>>,
        prepared_requests: Mutex<Vec<crate::layer_zero::DeliveryRequest>>,
    }

    impl SessionDeliveryRoute for CapturingLegacyRoute {
        fn reachability(&self) -> SessionReachability {
            SessionReachability::ClaudeCodePeer
        }

        fn claim(&self, _: &SessionRef) -> DeliveryFuture<'_, RouteClaim> {
            Box::pin(async { Ok(RouteClaim::Holds) })
        }

        fn presence(&self, _: &SessionRef) -> DeliveryFuture<'_, RoutePresence> {
            Box::pin(async { Ok(RoutePresence::Running) })
        }

        fn deliver<'a>(
            &'a self,
            request: LegacyDeliveryRequest,
            _: &'a dyn AttemptEvidenceSink,
        ) -> DeliveryFuture<'a, DeliveryReceipt> {
            self.requests
                .lock()
                .expect("captured requests")
                .push(request);
            Box::pin(async {
                Ok(DeliveryReceipt {
                    outcome: DeliveryOutcome::Started,
                    reachability: None,
                    client: None,
                })
            })
        }

        fn deliver_prepared<'a>(
            &'a self,
            request: crate::layer_zero::DeliveryRequest,
            evidence: &'a dyn AttemptEvidenceSink,
        ) -> DeliveryFuture<'a, DeliveryReceipt> {
            let legacy_request = request.clone().into_legacy_request();
            self.prepared_requests
                .lock()
                .expect("captured prepared requests")
                .push(request);
            self.deliver(legacy_request, evidence)
        }

        fn reconcile_attempt(
            &self,
            _: AttemptReconciliationContext,
        ) -> DeliveryFuture<'_, AttemptReconciliation> {
            Box::pin(async { Ok(AttemptReconciliation::StillUnknown) })
        }
    }

    #[tokio::test]
    async fn prepared_push_uses_the_legacy_route_stand_in_without_changing_its_line_or_policy() {
        let push_id = PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned())
            .expect("UUIDv7 push id");
        let expected_push_id = push_id.clone();
        let link = format!(
            "router://00000000-0000-4000-8000-000000000001/push/{}",
            push_id.as_str()
        );
        let line =
            MessageText::try_from(format!("✉️ sender · \"preview\" · {link}")).expect("push line");
        let target: SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
            "sessionId":"target"
        }))
        .expect("target session");
        let correlation = DeliveryCorrelationId::try_from(push_id.as_str().to_owned())
            .expect("push id correlation");
        let route = Arc::new(CapturingLegacyRoute::default());
        let router = SessionDeliveryRouter::new(vec![route.clone()]);

        let receipt = router
            .deliver_prepared(
                crate::layer_zero::DeliveryRequest {
                    payload: crate::layer_zero::PreparedPush {
                        push_id,
                        line: line.clone(),
                        load_policy: LoadPolicy::LoadedOnly,
                    },
                    target: target.clone(),
                    mode: MessageDelivery::Queue,
                    precondition: DeliveryPrecondition::Unpinned,
                    correlation: correlation.clone(),
                    attempt: AttemptId::generate(),
                },
                &crate::session_delivery_contract::UnstoredAttemptEvidenceSink,
            )
            .await
            .expect("prepared push delivery");

        assert_eq!(receipt.outcome, DeliveryOutcome::Started);
        assert_eq!(
            receipt.reachability,
            Some(SessionReachability::ClaudeCodePeer)
        );
        let prepared_requests = route
            .prepared_requests
            .lock()
            .expect("captured prepared requests");
        assert_eq!(prepared_requests.len(), 1);
        assert_eq!(prepared_requests[0].payload.push_id, expected_push_id);
        assert_eq!(prepared_requests[0].payload.line.as_str(), line.as_str());
        assert_eq!(
            prepared_requests[0].payload.load_policy,
            LoadPolicy::LoadedOnly
        );
        assert_eq!(prepared_requests[0].target, target);
        assert_eq!(prepared_requests[0].mode, MessageDelivery::Queue);
        assert_eq!(prepared_requests[0].correlation, correlation);
        let requests = route.requests.lock().expect("captured requests");
        assert_eq!(requests.len(), 1);
        let delivered = &requests[0];
        assert_eq!(delivered.target, target);
        assert_eq!(delivered.mode, MessageDelivery::Queue);
        assert_eq!(delivered.load_policy, LoadPolicy::LoadedOnly);
        assert_eq!(delivered.correlation, correlation);
        assert!(matches!(
            &delivered.precondition,
            DeliveryPrecondition::Unpinned
        ));
        assert_eq!(delivered.header_context, MessageHeaderContext::default());
        let MessageContent::Router { text } = &delivered.message else {
            panic!("a prepared push must remain Router-authored through the stand-in");
        };
        assert_eq!(text.as_str(), line.as_str());
    }
}
