use super::super::{
    SubscriptionClock, SubscriptionWaitFilter, subscription_service::OwnerObservation,
};
use super::owner_fixture::*;
use crate::AttemptReconciliation;
use collaboration_protocol::{
    DeliveryOutcome, DeliveryReceipt, PushDeliveryState, SessionReachability,
};
use message_board::{SubscriptionDeliveryOutcome, SubscriptionMode, SubscriptionScope, WhenIdle};

#[tokio::test(start_paused = true)]
async fn covering_wait_renews_only_covered_poll_subscription_timestamps() {
    let fixture = OwnerFixture::new().await;
    let uncovered_root = fixture.add_root().await;
    for root in [&fixture.root, &uncovered_root] {
        fixture
            .policy(root, SubscriptionMode::Poll, WhenIdle::Hold, 0, 0)
            .await;
    }
    let covered_scope = SubscriptionScope::thread(fixture.root.clone());
    let uncovered_scope = SubscriptionScope::thread(uncovered_root);
    let (covered_before, uncovered_before) = {
        let mut store = fixture.store.lock().await;
        (
            store
                .get_thread_subscription_record(&fixture.reader, &covered_scope)
                .await
                .unwrap()
                .unwrap(),
            store
                .get_thread_subscription_record(&fixture.reader, &uncovered_scope)
                .await
                .unwrap()
                .unwrap(),
        )
    };
    let runtime = fixture.runtime().await;
    fixture.clock.advance(3600).await;
    runtime.synchronize(&fixture.reader).await;
    let renewal_time = fixture.clock.now();
    let non_covering = runtime
        .service
        .wait(
            fixture.reader.clone(),
            SubscriptionWaitFilter::Roots(vec![message_board::MessageId::generate()]),
            0,
            usize::MAX,
        )
        .await
        .unwrap_err();
    assert_eq!(
        non_covering.kind,
        message_board::BoardFailureKind::InvalidField
    );
    for (scope, before) in [
        (&covered_scope, &covered_before),
        (&uncovered_scope, &uncovered_before),
    ] {
        let unchanged = fixture
            .store
            .lock()
            .await
            .get_thread_subscription_record(&fixture.reader, scope)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.renewed_at(), before.renewed_at());
        assert_eq!(unchanged.expires_at(), before.expires_at());
    }
    let service = runtime.service.clone();
    let reader = fixture.reader.clone();
    let covered_root = fixture.root.clone();
    let waiter = tokio::spawn(async move {
        service
            .wait(
                reader,
                SubscriptionWaitFilter::Roots(vec![covered_root]),
                60,
                usize::MAX,
            )
            .await
            .unwrap()
    });
    runtime
        .observe(|event| matches!(event, OwnerObservation::WaitQueued))
        .await;
    let (covered_after, uncovered_after) = {
        let mut store = fixture.store.lock().await;
        (
            store
                .get_thread_subscription_record(&fixture.reader, &covered_scope)
                .await
                .unwrap()
                .unwrap(),
            store
                .get_thread_subscription_record(&fixture.reader, &uncovered_scope)
                .await
                .unwrap()
                .unwrap(),
        )
    };
    assert_eq!(covered_after.renewed_at(), renewal_time);
    assert_eq!(
        covered_after.expires_at(),
        renewal_time + (covered_before.expires_at() - covered_before.renewed_at())
    );
    assert!(covered_after.renewed_at() > covered_before.renewed_at());
    assert!(covered_after.expires_at() > covered_before.expires_at());
    assert_eq!(uncovered_after.renewed_at(), uncovered_before.renewed_at());
    assert_eq!(uncovered_after.expires_at(), uncovered_before.expires_at());
    fixture.clock.advance(60).await;
    assert!(waiter.await.unwrap().is_none());
    assert!(runtime.requests.lock().await.try_recv().is_err());
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn loaded_only_acp_queue_is_reconciled_before_any_settlement() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.queued_runtime().await;
    fixture.post(&fixture.root, "Queued activity").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    runtime.completions.send(DeliveryOutcome::Queued).unwrap();
    let (context, reply) = runtime.reconciliations.lock().await.recv().await.unwrap();
    assert_eq!(context.target, request.target);
    let agent_automation::RouteEffectEvidence::ProviderAcp(evidence) = context.recorded else {
        panic!("captured ACP evidence");
    };
    assert_eq!(evidence.attempt_id, request.attempt);
    assert_eq!(
        evidence.submission,
        agent_automation::SubmissionEffect::RouterQueued
    );
    let record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.delivery_state, PushDeliveryState::Attempted);
    let board = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(board.roots()[0].pending_count(), 1);
    reply
        .send(AttemptReconciliation::Accepted(Box::new(DeliveryReceipt {
            outcome: DeliveryOutcome::Started,
            reachability: Some(SessionReachability::ProviderAcp),
            client: None,
        })))
        .ok()
        .unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    let record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record.last_outcome.unwrap().outcome,
        DeliveryOutcome::Started
    );
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn acp_queue_known_not_submitted_returns_to_hold_without_advancing() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.queued_runtime().await;
    fixture.post(&fixture.root, "Not submitted").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    runtime.completions.send(DeliveryOutcome::Queued).unwrap();
    let (_, reply) = runtime.reconciliations.lock().await.recv().await.unwrap();
    reply
        .send(AttemptReconciliation::KnownNotSubmitted)
        .ok()
        .unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::Held))
        .await;
    let record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.delivery_state, PushDeliveryState::Held);
    let board = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(board.roots()[0].pending_count(), 1);
    assert_eq!(board.roots()[0].held_since(), Some(fixture.clock.now()));
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn acp_queue_still_unknown_settles_unknown_with_route_evidence() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.queued_runtime().await;
    fixture.post(&fixture.root, "Unknown queue").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    runtime.completions.send(DeliveryOutcome::Queued).unwrap();
    let (_, reply) = runtime.reconciliations.lock().await.recv().await.unwrap();
    reply
        .send(AttemptReconciliation::StillUnknown)
        .ok()
        .unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    let record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.delivery_state, PushDeliveryState::OutcomeUnknown);
    let board = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let Some(SubscriptionDeliveryOutcome::Unknown { evidence }) = board.last_outcome() else {
        panic!("unknown board evidence");
    };
    assert_eq!(evidence["routeEvidence"]["submission"], "routerQueued");
    runtime.close().await;
}
