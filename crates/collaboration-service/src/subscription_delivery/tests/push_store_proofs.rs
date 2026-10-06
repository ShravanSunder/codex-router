use super::super::subscription_push::{
    SubscriptionPushStore, SubscriptionPushStoreProps, target_session,
};
use super::super::{SubscriptionClock, subscription_service::OwnerObservation};
use super::owner_fixture::*;
use collaboration_protocol::{DeliveryOutcome, PushDeliveryState, RouterOriginRef, UuidIdentity};
use message_board::{SubscriptionDeliveryOutcome, SubscriptionScope};
use sqlx::Connection;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

const OWNER_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct OwnerObservationTrace {
    last: Option<OwnerObservation>,
    last_rows_loaded: Option<usize>,
}

impl OwnerObservationTrace {
    fn record(&mut self, cycle: &str, observation: OwnerObservation) {
        eprintln!("{cycle}: owner observation: {observation:?}");
        if let OwnerObservation::RowsLoaded(rows) = &observation {
            self.last_rows_loaded = Some(*rows);
        }
        self.last = Some(observation);
    }

    fn timeout_message(&self, cycle: &str, stage: &str) -> String {
        format!(
            "{cycle}: timed out while {stage}; last owner observation: {:?}",
            self.last
        )
    }
}

fn test_subscription_push_store(fixture: &OwnerFixture) -> SubscriptionPushStore {
    let (observations, _) = broadcast::channel(1);
    SubscriptionPushStore::new(SubscriptionPushStoreProps {
        store: Arc::clone(&fixture.push_store),
        delivery: Arc::new(crate::SessionDeliveryRouter::new(Vec::new())),
        machine: crate::MachineIdentity::new(
            UuidIdentity::try_from(SERVICE_ID.to_owned()).unwrap(),
            Some("Owner-test-machine"),
        )
        .unwrap(),
        clock: fixture.clock.clone(),
        shutdown: CancellationToken::new(),
        observations,
    })
}

fn stored_router_origin_ref(record: &collaboration_protocol::PushRecord) -> RouterOriginRef {
    let encoded = record
        .origin_router_ref
        .as_deref()
        .expect("stored Router push has a typed origin reference");
    let origin_ref = RouterOriginRef::parse_canonical(encoded).unwrap();
    assert_eq!(origin_ref.canonical_string().unwrap(), encoded);
    origin_ref
}

async fn wait_for_owner_start_boundary(
    runtime: &OwnerRuntime,
    trace: &mut OwnerObservationTrace,
    cycle: &str,
) -> usize {
    let rows_loaded = wait_for_owner_observation(
        runtime,
        trace,
        cycle,
        "waiting for restored subscription rows",
        |observation| matches!(observation, OwnerObservation::RowsLoaded(_)),
    )
    .await;
    let OwnerObservation::RowsLoaded(row_count) = rows_loaded else {
        panic!("owner row-load observation has an unexpected shape");
    };
    let sleeping = wait_for_owner_observation(
        runtime,
        trace,
        cycle,
        "waiting for the owner's deadline boundary",
        |observation| matches!(observation, OwnerObservation::Sleeping(Some(_))),
    )
    .await;
    assert!(matches!(sleeping, OwnerObservation::Sleeping(Some(_))));
    row_count
}

async fn receive_expiry_request_with_owner_observations(
    runtime: &OwnerRuntime,
    trace: &mut OwnerObservationTrace,
    cycle: &str,
) -> crate::layer_zero::DeliveryRequest {
    let wait_for_request = async {
        loop {
            tokio::select! {
                request = async { runtime.requests.lock().await.recv().await } => {
                    eprintln!("{cycle}: scripted expiry delivery request received");
                    return request.expect("expiry owner sends a scripted delivery request");
                }
                observation = async { runtime.observations.lock().await.recv().await } => {
                    trace.record(
                        cycle,
                        observation.expect("owner observation stream remains open"),
                    );
                }
            }
        }
    };
    tokio::time::timeout(OWNER_EVENT_TIMEOUT, wait_for_request)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "{}",
                trace.timeout_message(cycle, "waiting for the expiry delivery request")
            )
        })
}

async fn wait_for_owner_observation(
    runtime: &OwnerRuntime,
    trace: &mut OwnerObservationTrace,
    cycle: &str,
    stage: &str,
    matches: impl Fn(&OwnerObservation) -> bool,
) -> OwnerObservation {
    let wait_for_event = async {
        loop {
            let observation = runtime
                .observations
                .lock()
                .await
                .recv()
                .await
                .expect("owner observation stream remains open");
            let matched = matches(&observation);
            trace.record(cycle, observation.clone());
            if matched {
                return observation;
            }
        }
    };
    tokio::time::timeout(OWNER_EVENT_TIMEOUT, wait_for_event)
        .await
        .unwrap_or_else(|_| panic!("{}", trace.timeout_message(cycle, stage)))
}

async fn close_owner_with_timeout(
    runtime: OwnerRuntime,
    trace: &OwnerObservationTrace,
    cycle: &str,
) {
    tokio::time::timeout(OWNER_EVENT_TIMEOUT, runtime.close())
        .await
        .unwrap_or_else(|_| panic!("{}", trace.timeout_message(cycle, "closing the owner")));
}

#[tokio::test(start_paused = true)]
async fn successive_selected_activity_batches_store_distinct_canonical_origin_refs() {
    let fixture = OwnerFixture::new().await;
    let push = test_subscription_push_store(&fixture);
    let target = target_session(&fixture.reader).unwrap();
    let scope = SubscriptionScope::thread(fixture.root.clone());
    let mut stored_refs = Vec::with_capacity(2);
    let mut selected_batch_ids = Vec::with_capacity(2);

    for body in ["First batch", "Second batch"] {
        fixture.post(&fixture.root, body).await;
        let (batch, settlement) = fixture
            .store
            .lock()
            .await
            .select_subscription_notice(
                &fixture.reader,
                std::slice::from_ref(&fixture.root),
                fixture.clock.now(),
                usize::MAX,
            )
            .await
            .unwrap();
        let selected_batch_id = batch.batch_id.clone();
        assert!(
            settlement
                .roots
                .iter()
                .all(|root| root.subscription_scope == scope),
            "both selected batches use the same subscription scope"
        );

        let prepared = push
            .activity(&target, &batch, crate::LoadPolicy::LoadedOnly)
            .await
            .unwrap();
        let stored = fixture
            .push_store
            .lock()
            .await
            .get_push_record(&prepared.push_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.kind,
            collaboration_protocol::PushKind::SubscriptionActivity
        );
        let origin_ref = stored_router_origin_ref(&stored);
        let RouterOriginRef::SubscriptionActivity {
            target: stored_target,
            batch_id: stored_batch_id,
        } = &origin_ref
        else {
            panic!("activity push stores a SubscriptionActivity origin reference");
        };
        assert_eq!(stored_target, &target);
        assert_eq!(stored_batch_id, &selected_batch_id);

        selected_batch_ids.push(selected_batch_id);
        stored_refs.push(origin_ref);
        fixture
            .store
            .lock()
            .await
            .settle_subscription_batch(
                &fixture.reader,
                &settlement,
                SubscriptionDeliveryOutcome::Accepted,
            )
            .await
            .unwrap();
    }

    assert_ne!(selected_batch_ids[0], selected_batch_ids[1]);
    assert_ne!(stored_refs[0], stored_refs[1]);
}

#[tokio::test]
async fn expiry_after_resubscribe_stores_a_reference_for_the_new_generation() {
    let fixture = OwnerFixture::new().await;
    let scope = SubscriptionScope::thread(fixture.root.clone());
    let target = target_session(&fixture.reader).unwrap();

    let mut first_trace = OwnerObservationTrace::default();
    let first_runtime = fixture.runtime().await;
    eprintln!("first expiry cycle: fresh owner started from the stored subscription");
    let first_rows_loaded =
        wait_for_owner_start_boundary(&first_runtime, &mut first_trace, "first expiry cycle").await;
    assert_eq!(first_rows_loaded, 1);
    eprintln!("first expiry cycle: advancing the injected clock by 24 hours");
    fixture.clock.advance_without_tokio_time(24 * 60 * 60);
    let first_request = receive_expiry_request_with_owner_observations(
        &first_runtime,
        &mut first_trace,
        "first expiry cycle",
    )
    .await;
    let first_push_id = first_request.payload.push_id.clone();
    let first_record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&first_request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    let first_origin_ref = stored_router_origin_ref(&first_record);
    let first_expired_subscription = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(&fixture.reader, &scope)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first_expired_subscription.state().end_reason(),
        Some(message_board::EndReason::Expired)
    );
    let first_generation = first_expired_subscription.generation();
    let RouterOriginRef::SubscriptionExpiry {
        target: first_target,
        scope: first_scope,
        subscription_generation: first_origin_generation,
    } = &first_origin_ref
    else {
        panic!("expiry push stores a SubscriptionExpiry origin reference");
    };
    assert_eq!(first_target, &target);
    assert_eq!(first_scope, &scope);
    assert_eq!(*first_origin_generation, first_generation);

    first_runtime
        .completions
        .send(DeliveryOutcome::Started)
        .unwrap();
    wait_for_owner_observation(
        &first_runtime,
        &mut first_trace,
        "first expiry cycle",
        "waiting for expiry owner shutdown",
        |observation| matches!(observation, OwnerObservation::Stopped),
    )
    .await;
    let first_delivered_record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&first_push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first_delivered_record.delivery_state,
        PushDeliveryState::Delivered
    );
    close_owner_with_timeout(first_runtime, &first_trace, "first expiry cycle").await;

    fixture
        .policy(
            &fixture.root,
            message_board::SubscriptionMode::Deliver,
            message_board::WhenIdle::Hold,
            0,
            0,
        )
        .await;
    let resubscribed = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(&fixture.reader, &scope)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        resubscribed.state(),
        message_board::SubscriptionState::Active
    );
    assert!(resubscribed.generation().get() > first_generation.get());

    let mut second_trace = OwnerObservationTrace::default();
    let second_runtime = fixture.runtime().await;
    eprintln!("second expiry cycle: fresh owner started after resubscription");
    let second_rows_loaded =
        wait_for_owner_start_boundary(&second_runtime, &mut second_trace, "second expiry cycle")
            .await;
    assert_eq!(second_rows_loaded, 1);
    assert_eq!(second_trace.last_rows_loaded, Some(1));
    eprintln!("second expiry cycle: fresh owner reloaded the reactivated subscription");
    eprintln!("second expiry cycle: advancing the injected clock by 24 hours");
    fixture.clock.advance_without_tokio_time(24 * 60 * 60);
    let second_request = receive_expiry_request_with_owner_observations(
        &second_runtime,
        &mut second_trace,
        "second expiry cycle",
    )
    .await;
    let second_push_id = second_request.payload.push_id.clone();
    let second_record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&second_request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    let second_origin_ref = stored_router_origin_ref(&second_record);
    let second_expired_subscription = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(&fixture.reader, &scope)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        second_expired_subscription.state().end_reason(),
        Some(message_board::EndReason::Expired)
    );
    let second_generation = second_expired_subscription.generation();
    let RouterOriginRef::SubscriptionExpiry {
        target: second_target,
        scope: second_scope,
        subscription_generation: second_origin_generation,
    } = &second_origin_ref
    else {
        panic!("expiry push stores a SubscriptionExpiry origin reference");
    };
    assert_eq!(second_target, &target);
    assert_eq!(second_scope, &scope);
    assert_eq!(*second_origin_generation, second_generation);
    assert_ne!(first_generation, second_generation);
    assert_ne!(first_origin_ref, second_origin_ref);

    second_runtime
        .completions
        .send(DeliveryOutcome::Started)
        .unwrap();
    wait_for_owner_observation(
        &second_runtime,
        &mut second_trace,
        "second expiry cycle",
        "waiting for expiry owner shutdown",
        |observation| matches!(observation, OwnerObservation::Stopped),
    )
    .await;
    let second_delivered_record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&second_push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        second_delivered_record.delivery_state,
        PushDeliveryState::Delivered
    );
    close_owner_with_timeout(second_runtime, &second_trace, "second expiry cycle").await;
}

#[tokio::test(start_paused = true)]
async fn failed_wait_attempt_mark_releases_selection_and_returns_explicit_error() {
    let fixture = OwnerFixture::new().await;
    fixture
        .policy(
            &fixture.root,
            message_board::SubscriptionMode::Poll,
            message_board::WhenIdle::Hold,
            0,
            0,
        )
        .await;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_wait_attempt BEFORE UPDATE OF delivery_state ON router_pushes WHEN NEW.delivery_state='attempted' BEGIN SELECT RAISE(FAIL, 'injected wait attempt failure'); END").execute(&mut observer).await.unwrap();
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Pending poll range").await;
    let result = runtime
        .service
        .wait(
            fixture.reader.clone(),
            super::super::SubscriptionWaitFilter::All,
            60,
            usize::MAX,
        )
        .await;
    assert!(
        result.is_err(),
        "a failed attempted-state write must be reported to the waiter"
    );
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
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
    let due = fixture
        .store
        .lock()
        .await
        .due_subscription_roots(&fixture.reader, fixture.clock.now())
        .await
        .unwrap();
    assert_eq!(
        due,
        vec![fixture.root.clone()],
        "unhanded selection must be released and remain retryable"
    );
    sqlx::query("DROP TRIGGER fail_wait_attempt")
        .execute(&mut observer)
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .service
            .wait(
                fixture.reader.clone(),
                super::super::SubscriptionWaitFilter::All,
                60,
                usize::MAX
            )
            .await
            .unwrap(),
        Some(super::super::SubscriptionWaitResult::Notice { .. })
    ));
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn accepted_push_record_settlement_retries_before_board_settlement_without_redelivery() {
    let fixture = OwnerFixture::new().await;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_push_settle BEFORE UPDATE OF delivery_state ON router_pushes WHEN NEW.delivery_state='delivered' BEGIN SELECT RAISE(FAIL, 'injected push settlement failure'); END").execute(&mut observer).await.unwrap();
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Accepted").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    runtime.completions.send(DeliveryOutcome::Started).unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::PushSettlementRetry))
        .await;
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
    assert!(runtime.requests.lock().await.try_recv().is_err());
    sqlx::query("DROP TRIGGER fail_push_settle")
        .execute(&mut observer)
        .await
        .unwrap();
    fixture.clock.advance(1).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    runtime.synchronize(&fixture.reader).await;
    let stored = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.delivery_state, PushDeliveryState::Delivered);
    assert!(runtime.requests.lock().await.try_recv().is_err());
    observer.close().await.unwrap();
    runtime.close().await;
}

#[path = "record_insert_failure_proof.rs"]
mod record_insert_failure_proof;
