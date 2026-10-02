use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, MessageDelivery, MessageText, PushId,
    SessionReachability, SessionRef,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
    DeliveryPrecondition, DeliveryReceipt, LoadPolicy, RouteClaim, RoutePresence,
    RouteUnavailableReason, SessionDeliveryRoute, SessionDeliveryRouter, SessionMessageDelivery,
    TargetPresence, TargetPresenceProbe, layer_zero,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct FakeRoute {
    reachability: SessionReachability,
    claim: RouteClaim,
    presence: RoutePresence,
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
    fn presence(&self, _: &SessionRef) -> DeliveryFuture<'_, RoutePresence> {
        let presence = self.presence.clone();
        Box::pin(async move { Ok(presence) })
    }
    fn deliver(
        &self,
        _: layer_zero::DeliveryRequest,
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

fn request() -> Result<layer_zero::DeliveryRequest, Box<dyn std::error::Error>> {
    request_with_load_policy(LoadPolicy::MayLoad)
}

fn request_with_load_policy(
    load_policy: LoadPolicy,
) -> Result<layer_zero::DeliveryRequest, Box<dyn std::error::Error>> {
    let target = target()?;
    let push_id = PushId::try_from(agent_automation::AttemptId::generate().as_str().to_owned())?;
    let correlation = DeliveryCorrelationId::try_from(push_id.as_str().to_owned())?;
    let line = MessageText::try_from(format!(
        "✉️ sender · \"hello\" · router://{}/push/{}",
        String::from(target.endpoint.service_id.clone()),
        push_id.as_str()
    ))?;
    Ok(layer_zero::DeliveryRequest {
        payload: layer_zero::PreparedPush {
            push_id,
            line,
            load_policy,
        },
        target,
        mode: MessageDelivery::Auto,
        precondition: DeliveryPrecondition::Unpinned,
        correlation,
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
    let presence = match &claim {
        RouteClaim::NotMine => RoutePresence::NotMine,
        RouteClaim::Holds => RoutePresence::Running,
        RouteClaim::CanLoad => RoutePresence::Wakeable,
        RouteClaim::Rejected { rejection } => RoutePresence::LiveElsewhere {
            detail: rejection.detail.clone(),
        },
        RouteClaim::LiveElsewhere { detail, .. } => RoutePresence::LiveElsewhere {
            detail: detail.clone(),
        },
        RouteClaim::Unavailable { reason, .. } => RoutePresence::Unreachable {
            reason: reason.reason.clone(),
        },
    };
    fake_with_claim_and_presence(claim, presence, reachability, outcome)
}

fn fake_with_claim_and_presence(
    claim: RouteClaim,
    presence: RoutePresence,
    reachability: SessionReachability,
    outcome: DeliveryOutcome,
) -> (Arc<dyn SessionDeliveryRoute>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    (
        Arc::new(FakeRoute {
            reachability,
            claim,
            presence,
            outcome,
            calls: calls.clone(),
        }),
        calls,
    )
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn presence_aggregation_uses_running_then_live_elsewhere_then_wakeable()
-> Result<(), Box<dyn std::error::Error>> {
    let (running, _) = fake_with_claim_and_presence(
        RouteClaim::Holds,
        RoutePresence::Running,
        SessionReachability::CodexAppServer,
        DeliveryOutcome::Queued,
    );
    let (live_elsewhere, _) = fake_with_claim_and_presence(
        RouteClaim::LiveElsewhere {
            writable: false,
            detail: Some("peer is live but unsupported".into()),
        },
        RoutePresence::LiveElsewhere {
            detail: Some("peer is live but unsupported".into()),
        },
        SessionReachability::ClaudeCodePeer,
        DeliveryOutcome::Queued,
    );
    let (wakeable, _) = fake_with_claim_and_presence(
        RouteClaim::CanLoad,
        RoutePresence::Wakeable,
        SessionReachability::ProviderAcp,
        DeliveryOutcome::Queued,
    );

    let router = SessionDeliveryRouter::new(vec![wakeable, live_elsewhere, running]);
    assert!(matches!(
        router.presence(&target()?).await?,
        TargetPresence::Running
    ));

    let (live_elsewhere, _) = fake_with_claim_and_presence(
        RouteClaim::LiveElsewhere {
            writable: false,
            detail: Some("peer is live but unsupported".into()),
        },
        RoutePresence::LiveElsewhere {
            detail: Some("peer is live but unsupported".into()),
        },
        SessionReachability::ClaudeCodePeer,
        DeliveryOutcome::Queued,
    );
    let (wakeable, _) = fake_with_claim_and_presence(
        RouteClaim::CanLoad,
        RoutePresence::Wakeable,
        SessionReachability::ProviderAcp,
        DeliveryOutcome::Queued,
    );
    let router = SessionDeliveryRouter::new(vec![wakeable, live_elsewhere]);
    assert!(matches!(
        router.presence(&target()?).await?,
        TargetPresence::Unreachable { ref reason }
            if reason.starts_with("live elsewhere")
                && reason.contains("peer is live but unsupported")
    ));

    let (unreachable, _) = fake_with_claim_and_presence(
        RouteClaim::Unavailable {
            reason: RouteUnavailableReason {
                reason: "Codex backend is down".into(),
                fix: "retry after it starts".into(),
            },
            retryable: true,
        },
        RoutePresence::Unreachable {
            reason: "Codex backend is down".into(),
        },
        SessionReachability::CodexAppServer,
        DeliveryOutcome::Queued,
    );
    let (wakeable, _) = fake_with_claim_and_presence(
        RouteClaim::CanLoad,
        RoutePresence::Wakeable,
        SessionReachability::ProviderAcp,
        DeliveryOutcome::Queued,
    );
    let router = SessionDeliveryRouter::new(vec![unreachable, wakeable]);
    assert!(matches!(
        router.presence(&target()?).await?,
        TargetPresence::Wakeable
    ));

    let (codex_unreachable, _) = fake_with_claim_and_presence(
        RouteClaim::Unavailable {
            reason: RouteUnavailableReason {
                reason: "Codex backend is down".into(),
                fix: "retry after it starts".into(),
            },
            retryable: true,
        },
        RoutePresence::Unreachable {
            reason: "Codex backend is down".into(),
        },
        SessionReachability::CodexAppServer,
        DeliveryOutcome::Queued,
    );
    let (provider_unreachable, _) = fake_with_claim_and_presence(
        RouteClaim::Unavailable {
            reason: RouteUnavailableReason {
                reason: "provider runtime is unavailable".into(),
                fix: "retry after provider starts".into(),
            },
            retryable: true,
        },
        RoutePresence::Unreachable {
            reason: "provider runtime is unavailable".into(),
        },
        SessionReachability::ProviderAcp,
        DeliveryOutcome::Queued,
    );
    let router = SessionDeliveryRouter::new(vec![codex_unreachable, provider_unreachable]);
    assert!(matches!(
        router.presence(&target()?).await?,
        TargetPresence::Unreachable { reason }
            if reason.contains("Codex backend is down")
                && reason.contains("provider runtime is unavailable")
    ));
    let router = SessionDeliveryRouter::new(Vec::new());
    assert!(matches!(
        router.presence(&target()?).await?,
        TargetPresence::Unreachable { reason }
            if reason == "no route holds or can load this session"
    ));
    Ok(())
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn loaded_only_delegates_loadability_inspection_to_the_route()
-> Result<(), Box<dyn std::error::Error>> {
    let (loadable, route_calls) = fake_with_outcome(
        RouteClaim::CanLoad,
        SessionReachability::ProviderAcp,
        DeliveryOutcome::NotSubmitted {
            retryable: false,
            reason: "unsupported: load".into(),
        },
    );
    let router = SessionDeliveryRouter::new(vec![loadable]);

    let receipt = router
        .deliver(
            request_with_load_policy(LoadPolicy::LoadedOnly)?,
            &NoopEvidenceSink,
        )
        .await?;

    assert!(matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: false,
            ref reason,
        } if reason == "unsupported: load"
    ));
    assert_eq!(route_calls.load(Ordering::SeqCst), 1);
    Ok(())
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
        RouteClaim::LiveElsewhere {
            writable: false,
            detail: Some("registry protocol 2 is unsupported".into()),
        },
        SessionReachability::ClaudeCodePeer,
    );
    let router = SessionDeliveryRouter::new(vec![provider, peer]);
    let receipt = router.deliver(request()?, &NoopEvidenceSink).await?;
    if !matches!(
        receipt.outcome,
        DeliveryOutcome::Rejected(rejection)
            if rejection.detail.as_deref() == Some("registry protocol 2 is unsupported")
    ) || provider_calls.load(Ordering::SeqCst) != 0
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
