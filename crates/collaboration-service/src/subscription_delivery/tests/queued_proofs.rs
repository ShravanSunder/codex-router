use super::super::{SubscriptionClock, subscription_service::OwnerObservation};
use super::owner_fixture::*;
use crate::AttemptReconciliation;
use collaboration_protocol::{
    DeliveryOutcome, DeliveryReceipt, PushDeliveryState, SessionReachability,
};
use message_board::{SubscriptionDeliveryOutcome, SubscriptionScope};

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
