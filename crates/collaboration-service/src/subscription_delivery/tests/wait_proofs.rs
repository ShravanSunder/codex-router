use super::super::{
    SubscriptionClock, SubscriptionWaitFilter, SubscriptionWaitResult,
    subscription_service::OwnerObservation,
};
use super::owner_fixture::*;
use message_board::*;
use sqlx::Connection;

#[tokio::test(start_paused = true)]
async fn concurrent_session_waits_are_fifo_and_never_receive_the_same_range() {
    let fixture = OwnerFixture::new().await;
    fixture
        .policy(&fixture.root, SubscriptionMode::Poll, WhenIdle::Hold, 0, 0)
        .await;
    let runtime = fixture.runtime().await;
    let first_service = runtime.service.clone();
    let first_reader = fixture.reader.clone();
    let topic = fixture.topic.clone();
    let first = tokio::spawn(async move {
        first_service
            .wait(
                first_reader,
                SubscriptionWaitFilter::Topic(topic),
                60,
                usize::MAX,
            )
            .await
            .unwrap()
            .unwrap()
    });
    runtime
        .observe(|event| matches!(event, OwnerObservation::WaitQueued))
        .await;
    let second_service = runtime.service.clone();
    let second_reader = fixture.reader.clone();
    let second = tokio::spawn(async move {
        second_service
            .wait(second_reader, SubscriptionWaitFilter::All, 60, usize::MAX)
            .await
            .unwrap()
            .unwrap()
    });
    runtime
        .observe(|event| matches!(event, OwnerObservation::WaitQueued))
        .await;
    let first_post = fixture.post(&fixture.root, "First range").await;
    let SubscriptionWaitResult::Notice {
        push_id: first_id,
        batch: first_batch,
        line,
    } = first.await.unwrap()
    else {
        panic!("session notice");
    };
    assert!(line.as_str().starts_with("🧵 Router: new thread activity"));
    assert_eq!(
        first_batch.roots[0].through_sequence,
        first_post.message.activity_sequence
    );
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    assert!(!second.is_finished());
    let second_post = fixture.post(&fixture.root, "Second range").await;
    let SubscriptionWaitResult::Notice {
        push_id: second_id,
        batch: second_batch,
        ..
    } = second.await.unwrap()
    else {
        panic!("session notice");
    };
    assert_ne!(first_id, second_id);
    assert_eq!(
        second_batch.roots[0].from_sequence,
        second_post.message.activity_sequence
    );
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn human_poll_returns_bodyless_ranges_without_storing_or_pushing() {
    let fixture = OwnerFixture::new().await;
    fixture
        .policy(&fixture.root, SubscriptionMode::Off, WhenIdle::Hold, 0, 0)
        .await;
    let reader = human("human-poll-reader");
    {
        let mut store = fixture.store.lock().await;
        store
            .join_thread(
                ThreadJoinRequest {
                    mode: None,
                    when_idle: None,
                    root_message_id: fixture.root.clone(),
                    actor: reader.clone(),
                    role: ParticipantRole::Participant,
                    watch: true,
                    replace: None,
                    note: None,
                },
                fixture.clock.now(),
            )
            .await
            .unwrap();
        store
            .subscribe_thread_subscription(
                ThreadSubscriptionSubscribeRequest {
                    reader: reader.clone(),
                    scope: SubscriptionScope::thread(fixture.root.clone()),
                    policy: SubscriptionPolicyPatch {
                        mode: Some(SubscriptionMode::Poll),
                        when_idle: None,
                        timing: SubscriptionTimingPatch {
                            quiet_seconds: Some(0),
                            cap_seconds: Some(0),
                        },
                        lifetime_seconds: None,
                    },
                },
                fixture.clock.now(),
            )
            .await
            .unwrap();
    }
    let runtime = fixture.runtime().await;
    let service = runtime.service.clone();
    let waiter = tokio::spawn(async move {
        service
            .wait(reader, SubscriptionWaitFilter::All, 60, usize::MAX)
            .await
            .unwrap()
            .unwrap()
    });
    runtime
        .observe(|event| matches!(event, OwnerObservation::WaitQueued))
        .await;
    fixture
        .post(&fixture.root, "Human-visible source remains on the board")
        .await;
    let SubscriptionWaitResult::Ranges { batch } = waiter.await.unwrap() else {
        panic!("human gets ranges");
    };
    assert_eq!(batch.roots[0].message_count, 1);
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM router_pushes")
        .fetch_one(&mut observer)
        .await
        .unwrap();
    assert_eq!(stored, 0);
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn poll_wait_timeout_is_empty_and_non_poll_wait_is_rejected() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    assert!(
        runtime
            .service
            .wait(
                fixture.reader.clone(),
                SubscriptionWaitFilter::All,
                1,
                usize::MAX,
            )
            .await
            .is_err()
    );
    fixture
        .policy(&fixture.root, SubscriptionMode::Poll, WhenIdle::Hold, 0, 0)
        .await;
    runtime
        .service
        .reconcile_reader(fixture.reader.clone())
        .await
        .unwrap();
    let service = runtime.service.clone();
    let reader = fixture.reader.clone();
    let waiter = tokio::spawn(async move {
        service
            .wait(reader, SubscriptionWaitFilter::All, 60, usize::MAX)
            .await
            .unwrap()
    });
    runtime
        .observe(|event| matches!(event, OwnerObservation::WaitQueued))
        .await;
    runtime.synchronize(&fixture.reader).await;
    fixture.clock.advance(59).await;
    runtime.synchronize(&fixture.reader).await;
    assert!(!waiter.is_finished());
    fixture.clock.advance(1).await;
    assert!(waiter.await.unwrap().is_none());
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn dropping_the_last_service_handle_cancels_its_owned_tasks() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    runtime.synchronize(&fixture.reader).await;
    let mut events = runtime.service.observe_owners();
    drop(runtime);
    loop {
        if matches!(events.recv().await.unwrap(), OwnerObservation::Stopped) {
            break;
        }
    }
    assert!(
        fixture
            .store
            .lock()
            .await
            .get_thread_subscription_record(
                &fixture.reader,
                &SubscriptionScope::thread(fixture.root.clone())
            )
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn finite_wait_budget_hands_oldest_root_then_preserves_remaining_due_root() {
    use std::time::Duration;

    let fixture = OwnerFixture::new().await;
    let oldest_root = fixture.root.clone();
    let later_root = fixture.add_root().await;
    for root in [&oldest_root, &later_root] {
        fixture
            .policy(root, SubscriptionMode::Poll, WhenIdle::Hold, 0, 0)
            .await;
    }
    let runtime = fixture.runtime().await;
    let oldest_post = fixture.post(&oldest_root, "Oldest pending root").await;
    fixture.post(&later_root, "Later pending root").await;
    let oldest_notice = PendingRootNotice::new(
        oldest_root.clone(),
        fixture.topic.clone(),
        oldest_post.message.activity_sequence,
        oldest_post.message.activity_sequence,
        1,
    )
    .unwrap();
    let maximum_root_notice_bytes = serde_json::to_vec(std::slice::from_ref(&oldest_notice))
        .unwrap()
        .len();

    let first_result = tokio::time::timeout(
        Duration::from_secs(30),
        runtime.service.wait(
            fixture.reader.clone(),
            SubscriptionWaitFilter::All,
            60,
            maximum_root_notice_bytes,
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    let SubscriptionWaitResult::Notice {
        push_id: first_id,
        batch: first_batch,
        ..
    } = first_result
    else {
        panic!("session wait returns a stored notice");
    };
    assert_eq!(first_batch.roots.len(), 1);
    assert_eq!(first_batch.roots[0].root_id, oldest_root);
    assert!(serde_json::to_vec(&first_batch.roots).unwrap().len() <= maximum_root_notice_bytes);
    tokio::time::timeout(
        Duration::from_secs(30),
        runtime.observe(|event| matches!(event, OwnerObservation::Settled)),
    )
    .await
    .unwrap();
    let still_due = fixture
        .store
        .lock()
        .await
        .due_subscription_roots(&fixture.reader, fixture.clock.now())
        .await
        .unwrap();
    assert_eq!(still_due, vec![later_root.clone()]);

    let second_result = tokio::time::timeout(
        Duration::from_secs(30),
        runtime.service.wait(
            fixture.reader.clone(),
            SubscriptionWaitFilter::All,
            60,
            maximum_root_notice_bytes,
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    let SubscriptionWaitResult::Notice {
        push_id: second_id,
        batch: second_batch,
        ..
    } = second_result
    else {
        panic!("session wait returns a stored notice");
    };
    assert_eq!(second_batch.roots.len(), 1);
    assert_eq!(second_batch.roots[0].root_id, later_root);
    assert_ne!(first_id, second_id);
    assert!(serde_json::to_vec(&second_batch.roots).unwrap().len() <= maximum_root_notice_bytes);
    tokio::time::timeout(
        Duration::from_secs(30),
        runtime.observe(|event| matches!(event, OwnerObservation::Settled)),
    )
    .await
    .unwrap();
    assert!(
        fixture
            .store
            .lock()
            .await
            .due_subscription_roots(&fixture.reader, fixture.clock.now())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(runtime.requests.lock().await.try_recv().is_err());
    runtime.close().await;
}
