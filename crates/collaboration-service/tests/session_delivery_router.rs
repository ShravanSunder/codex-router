use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, MessageContent, MessageDelivery,
    SessionReachability, SessionRef,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
    DeliveryPrecondition, DeliveryReceipt, DeliveryRequest, RouteClaim, RouteUnavailableReason,
    SessionDeliveryRoute, SessionDeliveryRouter, SessionMessageDelivery,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct FakeRoute {
    reachability: SessionReachability,
    claim: RouteClaim,
    outcome: DeliveryOutcome,
    calls: Arc<AtomicUsize>,
}

impl SessionDeliveryRoute for FakeRoute {
    fn reachability(&self) -> SessionReachability {
        self.reachability
    }
    fn claim(&self, _: &SessionRef) -> DeliveryFuture<'_, RouteClaim> {
        Box::pin(async { Ok(self.claim.clone()) })
    }
    fn deliver(
        &self,
        _: DeliveryRequest,
        _: &dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'_, DeliveryReceipt> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(DeliveryReceipt {
                outcome: self.outcome.clone(),
                reachability: Some(self.reachability),
                client: None,
            })
        })
    }
    fn reconcile_attempt(
        &self,
        _: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async { Ok(AttemptReconciliation::KnownNotSubmitted) })
    }
}

struct NoopEvidenceSink;
impl AttemptEvidenceSink for NoopEvidenceSink {
    fn record(
        &self,
        _: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn target() -> Result<SessionRef, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
        "sessionId":"session-one"
    }))?)
}

fn request() -> Result<DeliveryRequest, Box<dyn std::error::Error>> {
    Ok(DeliveryRequest {
        target: target()?,
        message: MessageContent::Router {
            text: "hello".to_owned().try_into()?,
        },
        mode: MessageDelivery::Auto,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::generate(),
        attempt: agent_automation::AttemptId::generate(),
    })
}

fn fake(
    claim: RouteClaim,
    reachability: SessionReachability,
) -> (Arc<dyn SessionDeliveryRoute>, Arc<AtomicUsize>) {
    fake_with_outcome(claim, reachability, DeliveryOutcome::Queued)
}

fn fake_with_outcome(
    claim: RouteClaim,
    reachability: SessionReachability,
    outcome: DeliveryOutcome,
) -> (Arc<dyn SessionDeliveryRoute>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    (
        Arc::new(FakeRoute {
            reachability,
            claim,
            outcome,
            calls: calls.clone(),
        }),
        calls,
    )
}

#[tokio::test]
async fn selected_route_not_submitted_never_falls_through_to_another_route()
-> Result<(), Box<dyn std::error::Error>> {
    let (selected, selected_calls) = fake_with_outcome(
        RouteClaim::Holds,
        SessionReachability::ProviderAcp,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            reason: "binding retired".into(),
        },
    );
    let (fallback, fallback_calls) = fake(RouteClaim::Holds, SessionReachability::ClaudeCodePeer);
    let router = SessionDeliveryRouter::new(vec![selected, fallback]);
    let receipt = router.deliver(request()?, &NoopEvidenceSink).await?;
    if !matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ..
        }
    ) || selected_calls.load(Ordering::SeqCst) != 1
        || fallback_calls.load(Ordering::SeqCst) != 0
    {
        return Err("selected route fall-through would risk a second submission".into());
    }
    Ok(())
}

#[tokio::test]
async fn held_route_wins_before_peer_and_loadable_provider()
-> Result<(), Box<dyn std::error::Error>> {
    let (provider, provider_calls) = fake(RouteClaim::Holds, SessionReachability::ProviderAcp);
    let (peer, peer_calls) = fake(RouteClaim::Holds, SessionReachability::ClaudeCodePeer);
    let (loadable, load_calls) = fake(RouteClaim::CanLoad, SessionReachability::ProviderAcp);
    let router = SessionDeliveryRouter::new(vec![provider, peer, loadable]);
    let receipt = router.deliver(request()?, &NoopEvidenceSink).await?;
    if receipt.reachability != Some(SessionReachability::ProviderAcp)
        || !matches!(receipt.outcome, DeliveryOutcome::Queued)
        || provider_calls.load(Ordering::SeqCst) != 1
        || peer_calls.load(Ordering::SeqCst) != 0
        || load_calls.load(Ordering::SeqCst) != 0
    {
        return Err("the first held route did not win without another delivery".into());
    }
    Ok(())
}

#[tokio::test]
async fn live_elsewhere_vetoes_provider_load() -> Result<(), Box<dyn std::error::Error>> {
    let (provider, provider_calls) = fake(RouteClaim::CanLoad, SessionReachability::ProviderAcp);
    let (peer, peer_calls) = fake(
        RouteClaim::LiveElsewhere { writable: false },
        SessionReachability::ClaudeCodePeer,
    );
    let router = SessionDeliveryRouter::new(vec![provider, peer]);
    let receipt = router.deliver(request()?, &NoopEvidenceSink).await?;
    if !matches!(receipt.outcome, DeliveryOutcome::Rejected(_))
        || provider_calls.load(Ordering::SeqCst) != 0
        || peer_calls.load(Ordering::SeqCst) != 0
    {
        return Err("live external owner did not veto provider load".into());
    }
    Ok(())
}

#[tokio::test]
async fn loadable_route_delivers_without_a_live_owner() -> Result<(), Box<dyn std::error::Error>> {
    let (provider, provider_calls) = fake(RouteClaim::CanLoad, SessionReachability::ProviderAcp);
    let (peer, _) = fake(RouteClaim::NotMine, SessionReachability::ClaudeCodePeer);
    let router = SessionDeliveryRouter::new(vec![provider, peer]);
    let receipt = router.deliver(request()?, &NoopEvidenceSink).await?;
    if receipt.reachability != Some(SessionReachability::ProviderAcp)
        || provider_calls.load(Ordering::SeqCst) != 1
    {
        return Err("loadable provider was not selected".into());
    }
    Ok(())
}

#[tokio::test]
async fn retryable_unavailable_and_all_not_mine_keep_distinct_outcomes()
-> Result<(), Box<dyn std::error::Error>> {
    let reason = RouteUnavailableReason {
        reason: "provider starting".into(),
        fix: "retry shortly".into(),
    };
    let (unavailable, calls) = fake(
        RouteClaim::Unavailable {
            reason,
            retryable: true,
        },
        SessionReachability::ProviderAcp,
    );
    let router = SessionDeliveryRouter::new(vec![unavailable]);
    let receipt = router.deliver(request()?, &NoopEvidenceSink).await?;
    if !matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ..
        }
    ) || calls.load(Ordering::SeqCst) != 0
    {
        return Err("retryable unavailability became a dispatch or rejection".into());
    }

    let (not_mine, _) = fake(RouteClaim::NotMine, SessionReachability::CodexAppServer);
    let router = SessionDeliveryRouter::new(vec![not_mine]);
    let receipt = router.deliver(request()?, &NoopEvidenceSink).await?;
    if !matches!(receipt.outcome, DeliveryOutcome::Rejected(_)) || receipt.reachability.is_some() {
        return Err("unserved endpoint did not reject without a client route".into());
    }
    Ok(())
}
