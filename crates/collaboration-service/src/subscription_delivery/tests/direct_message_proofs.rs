use super::super::{
    SubscriptionClock, subscription_push::target_session, subscription_service::OwnerObservation,
};
use super::owner_fixture::*;
use crate::{DeliveryPrecondition, LoadPolicy, TargetPresence};
use collaboration_protocol::{
    CodexGeneration, DeliveryOutcome, MessageDelivery, PushDeliveryState, PushHeaderFacts, PushId,
    PushKind, PushOrigin, PushRecord, PushRecordDraft,
};
use sqlx::Connection;
use std::time::Duration;

async fn draft(
    fixture: &OwnerFixture,
    mode: MessageDelivery,
    guard: Option<CodexGeneration>,
) -> PushRecordDraft {
    PushRecordDraft {
        push_id: PushId::try_from(uuid::Uuid::now_v7().to_string()).unwrap(),
        kind: PushKind::DirectMessage,
        origin: PushOrigin::Session(target_session(&fixture.reader).unwrap()),
        origin_router_ref: None,
        target: target_session(&fixture.reader).unwrap(),
        mode: Some(mode),
        guard,
        reply_to_push_id: None,
        header_facts: PushHeaderFacts::DirectMessage {
            sender_display_name: None,
        },
        body: Some("DM full content".to_owned()),
        activity: None,
        created_at: fixture.clock.now(),
    }
}

async fn record(fixture: &OwnerFixture, push_id: &PushId) -> PushRecord {
    fixture
        .push_store
        .lock()
        .await
        .get_push_record(push_id)
        .await
        .unwrap()
        .unwrap()
}

async fn request(runtime: &OwnerRuntime) -> crate::layer_zero::DeliveryRequest {
    tokio::time::timeout(Duration::from_secs(30), async {
        runtime.requests.lock().await.recv().await.unwrap()
    })
    .await
    .unwrap()
}

async fn bounded_observe(runtime: &OwnerRuntime, matches: impl Fn(&OwnerObservation) -> bool) {
    tokio::time::timeout(Duration::from_secs(30), runtime.observe(matches))
        .await
        .unwrap();
}

async fn insert(
    fixture: &OwnerFixture,
    mode: MessageDelivery,
    guard: Option<CodexGeneration>,
) -> PushId {
    let draft = draft(fixture, mode, guard).await;
    let push_id = draft.push_id.clone();
    fixture
        .push_store
        .lock()
        .await
        .insert_push_record(draft)
        .await
        .unwrap();
    push_id
}

fn guard() -> CodexGeneration {
    serde_json::from_value(serde_json::json!({"serviceEpoch":SERVICE_ID,"generation":3})).unwrap()
}

#[tokio::test]
async fn auto_and_queue_hold_without_attempt_and_recover_their_exact_mode() {
    for mode in [MessageDelivery::Auto, MessageDelivery::Queue] {
        let fixture = OwnerFixture::new().await;
        *fixture.presence.0.lock().unwrap() = TargetPresence::Wakeable;
        let push_id = insert(&fixture, mode, None).await;
        let runtime = fixture.runtime_without_board().await;
        bounded_observe(&runtime, |event| {
            matches!(event, OwnerObservation::DirectMessageHeld)
        })
        .await;
        let held = record(&fixture, &push_id).await;
        assert_eq!(held.delivery_state, PushDeliveryState::Held);
        assert_eq!(held.mode, Some(mode));
        assert!(runtime.requests.lock().await.try_recv().is_err());
        runtime.close().await;

        *fixture.presence.0.lock().unwrap() = TargetPresence::Running;
        let restored = fixture.runtime_without_board().await;
        let submitted = request(&restored).await;
        assert_eq!(submitted.payload.push_id, push_id);
        assert_eq!(submitted.mode, mode);
        assert!(matches!(
            submitted.precondition,
            DeliveryPrecondition::Unpinned
        ));
        assert_eq!(submitted.payload.load_policy, LoadPolicy::LoadedOnly);
        restored.completions.send(DeliveryOutcome::Started).unwrap();
        bounded_observe(&restored, |event| {
            matches!(event, OwnerObservation::DirectMessageSettled)
        })
        .await;
        assert_eq!(
            record(&fixture, &push_id).await.delivery_state,
            PushDeliveryState::Delivered
        );
        restored.close().await;
    }
}

#[tokio::test]
async fn unavailable_board_holds_dm_and_rechecks_presence_at_thirty_seconds() {
    let fixture = OwnerFixture::new().await;
    *fixture.presence.0.lock().unwrap() = TargetPresence::Wakeable;
    let runtime = fixture.runtime_without_board().await;
    assert!(
        runtime
            .service
            .reconcile_reader(fixture.reader.clone())
            .await
            .is_err()
    );
    let push_id = insert(&fixture, MessageDelivery::Queue, None).await;
    let held = runtime
        .service
        .deliver_direct_message(target_session(&fixture.reader).unwrap(), push_id.clone())
        .await
        .unwrap();
    assert_eq!(held.delivery_state, PushDeliveryState::Held);
    runtime.synchronize(&fixture.reader).await;
    *fixture.presence.0.lock().unwrap() = TargetPresence::Running;
    fixture.clock.advance_without_tokio_time(29);
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    fixture.clock.advance_without_tokio_time(1);
    assert_eq!(request(&runtime).await.mode, MessageDelivery::Queue);
    runtime.completions.send(DeliveryOutcome::Started).unwrap();
    bounded_observe(&runtime, |event| {
        matches!(event, OwnerObservation::DirectMessageSettled)
    })
    .await;
    runtime.close().await;
}

#[tokio::test]
async fn steer_and_guarded_send_to_nonrunning_reject_without_any_hold() {
    for (mode, pinned) in [
        (MessageDelivery::Steer, None),
        (MessageDelivery::Auto, Some(guard())),
    ] {
        let fixture = OwnerFixture::new().await;
        *fixture.presence.0.lock().unwrap() = TargetPresence::Wakeable;
        let runtime = fixture.runtime_without_board().await;
        let push_id = insert(&fixture, mode, pinned).await;
        let rejected = runtime
            .service
            .deliver_direct_message(target_session(&fixture.reader).unwrap(), push_id)
            .await
            .unwrap();
        assert_eq!(rejected.delivery_state, PushDeliveryState::Rejected);
        assert!(matches!(
            rejected.last_outcome.unwrap().outcome,
            DeliveryOutcome::Rejected(_)
        ));
        assert!(runtime.requests.lock().await.try_recv().is_err());
        assert!(
            fixture
                .push_store
                .lock()
                .await
                .direct_message_recovery_targets()
                .await
                .unwrap()
                .is_empty()
        );
        runtime.close().await;
    }
}

#[tokio::test]
async fn restart_gap_preserves_steer_guard_and_race_rejection_never_holds() {
    let fixture = OwnerFixture::new().await;
    let pinned = guard();
    let push_id = insert(&fixture, MessageDelivery::Steer, Some(pinned.clone())).await;
    let runtime = fixture.runtime_without_board().await;
    let submitted = request(&runtime).await;
    assert_eq!(submitted.mode, MessageDelivery::Steer);
    assert!(
        matches!(submitted.precondition, DeliveryPrecondition::EndpointGeneration { expected } if expected == pinned)
    );
    runtime
        .completions
        .send(DeliveryOutcome::NotSubmitted {
            retryable: true,
            reason: "target unloaded at the effect boundary".to_owned(),
        })
        .unwrap();
    bounded_observe(&runtime, |event| {
        matches!(event, OwnerObservation::DirectMessageSettled)
    })
    .await;
    assert_eq!(
        record(&fixture, &push_id).await.delivery_state,
        PushDeliveryState::Rejected
    );
    runtime.close().await;
}

#[tokio::test]
async fn restarted_nonrunning_steer_and_guarded_pending_records_are_rejected() {
    let fixture = OwnerFixture::new().await;
    *fixture.presence.0.lock().unwrap() = TargetPresence::Wakeable;
    let steer = insert(&fixture, MessageDelivery::Steer, None).await;
    let guarded = insert(&fixture, MessageDelivery::Queue, Some(guard())).await;
    let runtime = fixture.runtime_without_board().await;
    bounded_observe(&runtime, |event| matches!(event, OwnerObservation::Stopped)).await;
    for push_id in [steer, guarded] {
        assert_eq!(
            record(&fixture, &push_id).await.delivery_state,
            PushDeliveryState::Rejected
        );
    }
    assert!(runtime.requests.lock().await.try_recv().is_err());
    runtime.close().await;
}

#[tokio::test]
async fn same_target_dm_and_subscription_have_one_inflight_delivery() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "subscription body").await;
    let subscription = request(&runtime).await;
    let dm_id = insert(&fixture, MessageDelivery::Auto, None).await;
    let service = runtime.service.clone();
    let target = target_session(&fixture.reader).unwrap();
    let dm_id_for_send = dm_id.clone();
    let dm =
        tokio::spawn(async move { service.deliver_direct_message(target, dm_id_for_send).await });
    bounded_observe(&runtime, |event| {
        matches!(event, OwnerObservation::DirectMessageQueued)
    })
    .await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    assert_eq!(subscription.payload.load_policy, LoadPolicy::LoadedOnly);
    runtime.await_settled().await;
    assert_eq!(request(&runtime).await.payload.push_id, dm_id);
    runtime.completions.send(DeliveryOutcome::Started).unwrap();
    assert_eq!(
        dm.await.unwrap().unwrap().delivery_state,
        PushDeliveryState::Delivered
    );
    assert!(runtime.requests.lock().await.try_recv().is_err());
    runtime.close().await;
}

#[tokio::test]
async fn dm_settlement_failure_retries_the_write_without_duplicate_delivery() {
    let fixture = OwnerFixture::new().await;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_dm_settle BEFORE UPDATE OF delivery_state ON router_pushes WHEN NEW.delivery_state='delivered' BEGIN SELECT RAISE(FAIL, 'injected settlement failure'); END").execute(&mut observer).await.unwrap();
    let push_id = insert(&fixture, MessageDelivery::Auto, None).await;
    let runtime = fixture.runtime_without_board().await;
    assert_eq!(request(&runtime).await.payload.push_id, push_id);
    runtime.completions.send(DeliveryOutcome::Started).unwrap();
    bounded_observe(&runtime, |event| {
        matches!(event, OwnerObservation::PushSettlementRetry)
    })
    .await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    sqlx::query("DROP TRIGGER fail_dm_settle")
        .execute(&mut observer)
        .await
        .unwrap();
    fixture.clock.advance_without_tokio_time(1);
    bounded_observe(&runtime, |event| {
        matches!(event, OwnerObservation::DirectMessageSettled)
    })
    .await;
    assert_eq!(
        record(&fixture, &push_id).await.delivery_state,
        PushDeliveryState::Delivered
    );
    assert!(runtime.requests.lock().await.try_recv().is_err());
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test]
async fn restored_guarded_generation_mismatch_is_terminal_without_any_broadened_attempt() {
    let fixture = OwnerFixture::new().await;
    let pinned = guard();
    let push_id = insert(&fixture, MessageDelivery::Queue, Some(pinned.clone())).await;
    let runtime = fixture.runtime_without_board().await;
    let submitted = request(&runtime).await;
    assert_eq!(submitted.mode, MessageDelivery::Queue);
    assert!(
        matches!(submitted.precondition, DeliveryPrecondition::EndpointGeneration { expected } if expected == pinned)
    );
    runtime
        .completions
        .send(DeliveryOutcome::Rejected(
            collaboration_protocol::DeliveryRejection {
                reason: collaboration_protocol::DeliveryRejectionReason::StaleGeneration,
                next_action: collaboration_protocol::DeliveryNextAction::InspectTarget,
                client_code: None,
                detail: Some("pinned generation no longer exists".to_owned()),
                claims: None,
            },
        ))
        .unwrap();
    bounded_observe(&runtime, |event| {
        matches!(event, OwnerObservation::DirectMessageSettled)
    })
    .await;
    assert_eq!(
        record(&fixture, &push_id).await.delivery_state,
        PushDeliveryState::Rejected
    );
    runtime.close().await;
    let restored = fixture.runtime_without_board().await;
    assert!(
        fixture
            .push_store
            .lock()
            .await
            .direct_message_recovery_targets()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(restored.requests.lock().await.try_recv().is_err());
    restored.close().await;
}

#[tokio::test]
async fn attempted_dm_on_restart_becomes_unknown_and_never_reenters_layer_zero() {
    let fixture = OwnerFixture::new().await;
    let push_id = insert(&fixture, MessageDelivery::Auto, None).await;
    fixture
        .push_store
        .lock()
        .await
        .mark_push_attempted(&push_id)
        .await
        .unwrap();
    let runtime = fixture.runtime_without_board().await;
    let recovered = record(&fixture, &push_id).await;
    assert_eq!(recovered.delivery_state, PushDeliveryState::OutcomeUnknown);
    assert_eq!(
        recovered.last_outcome.unwrap().outcome,
        DeliveryOutcome::Unknown
    );
    assert!(runtime.requests.lock().await.try_recv().is_err());
    runtime.close().await;
}

struct LiveElsewhereRoute {
    claims: std::sync::atomic::AtomicUsize,
    effects: std::sync::atomic::AtomicUsize,
}

impl crate::SessionDeliveryRoute for LiveElsewhereRoute {
    fn reachability(&self) -> collaboration_protocol::SessionReachability {
        collaboration_protocol::SessionReachability::ClaudeCodePeer
    }
    fn claim(
        &self,
        _: &collaboration_protocol::SessionRef,
    ) -> crate::DeliveryFuture<'_, crate::RouteClaim> {
        self.claims
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async {
            Ok(crate::RouteClaim::LiveElsewhere {
                writable: false,
                detail: Some("live peer cannot receive Router input".to_owned()),
            })
        })
    }
    fn presence(
        &self,
        _: &collaboration_protocol::SessionRef,
    ) -> crate::DeliveryFuture<'_, crate::RoutePresence> {
        Box::pin(async {
            Ok(crate::RoutePresence::LiveElsewhere {
                detail: Some("live peer cannot receive Router input".to_owned()),
            })
        })
    }
    fn deliver<'a>(
        &'a self,
        _: crate::layer_zero::DeliveryRequest,
        _: &'a dyn crate::AttemptEvidenceSink,
    ) -> crate::DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt> {
        self.effects
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Err(crate::DeliveryContractError::InvalidEvidence) })
    }
    fn reconcile_attempt(
        &self,
        _: crate::AttemptReconciliationContext,
    ) -> crate::DeliveryFuture<'_, crate::AttemptReconciliation> {
        Box::pin(async { Err(crate::DeliveryContractError::InvalidEvidence) })
    }
}

fn live_elsewhere_service(
    fixture: &OwnerFixture,
    router: std::sync::Arc<crate::SessionDeliveryRouter>,
) -> super::super::SubscriptionDeliveryService {
    super::super::SubscriptionDeliveryService::new(super::super::SubscriptionDeliveryServiceProps {
        board_availability: super::super::BoardAvailability::Unavailable,
        push_store: fixture.push_store.clone(),
        delivery: router.clone(),
        presence: router,
        machine_identity: crate::MachineIdentity::new(
            collaboration_protocol::UuidIdentity::try_from(SERVICE_ID.to_owned()).unwrap(),
            Some("live-elsewhere-proof"),
        )
        .unwrap(),
        clock: fixture.clock.clone(),
    })
}

#[tokio::test]
async fn live_elsewhere_presence_reaches_authoritative_router_rejection_and_is_never_held_or_replayed()
 {
    use std::sync::{Arc, atomic::Ordering};
    for mode in [MessageDelivery::Auto, MessageDelivery::Queue] {
        let fixture = OwnerFixture::new().await;
        let route = Arc::new(LiveElsewhereRoute {
            claims: Default::default(),
            effects: Default::default(),
        });
        let router = Arc::new(crate::SessionDeliveryRouter::new(vec![route.clone()]));
        let service = live_elsewhere_service(&fixture, router.clone());
        service.start().await.unwrap();
        let mut pending = draft(&fixture, mode, None).await;
        pending.target.endpoint.endpoint_id =
            collaboration_protocol::EndpointId::try_from("claude-local".to_owned()).unwrap();
        let target = pending.target.clone();
        let push_id = pending.push_id.clone();
        fixture
            .push_store
            .lock()
            .await
            .insert_push_record(pending)
            .await
            .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            service.deliver_direct_message(target, push_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.delivery_state, PushDeliveryState::Rejected);
        assert!(
            matches!(result.last_outcome.unwrap().outcome, DeliveryOutcome::Rejected(rejection) if rejection.reason == collaboration_protocol::DeliveryRejectionReason::LiveElsewhere)
        );
        assert_eq!(route.claims.load(Ordering::SeqCst), 1);
        assert_eq!(route.effects.load(Ordering::SeqCst), 0);
        assert!(
            fixture
                .push_store
                .lock()
                .await
                .direct_message_recovery_targets()
                .await
                .unwrap()
                .is_empty()
        );
        service.shutdown().await;
        let restored = live_elsewhere_service(&fixture, router);
        restored.start().await.unwrap();
        assert_eq!(
            record(&fixture, &push_id).await.delivery_state,
            PushDeliveryState::Rejected
        );
        assert_eq!(route.claims.load(Ordering::SeqCst), 1);
        assert_eq!(route.effects.load(Ordering::SeqCst), 0);
        restored.shutdown().await;
    }
}
