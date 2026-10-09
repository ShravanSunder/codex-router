use super::test_support::*;

use std::future::Future;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use codex_router_core::ids::ReservationId;
use codex_router_core::routes::RouteBand;

use crate::account_selection::RouteBandQueueDegradedReason;
use crate::account_selection::RouteBandQueueHealth;
use crate::account_selection::route_band_queue_health_allows_selection;
use crate::provider_error::ProviderErrorClassification;
use codex_router_core::affinity::AffinityKeyHash;
use codex_router_state::affinity_owner::AffinitySourceTransport;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::session_account_affinity::SessionAccountAffinity;

use super::DbWriteActor;
use super::DbWriteCommand;
use super::DbWriteEnqueueResult;

#[tokio::test]
async fn active_client_mirror_commands_are_owned_by_db_write_actor() {
    let repository = Arc::new(RecordingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 4);
    let account_id = account_id("acct_active_mirror");
    let reservation_id = ReservationId::new("reservation_active_mirror");

    let acquired = actor.try_enqueue(DbWriteCommand::active_client_acquired(
        RouteBand::Responses,
        "process-runtime".to_owned(),
        reservation_id.clone(),
        account_id.clone(),
        1_000,
        2,
    ));
    let released = actor.try_enqueue(DbWriteCommand::active_client_released(
        RouteBand::Responses,
        "process-runtime".to_owned(),
        reservation_id.clone(),
        1_100,
    ));

    assert_eq!(acquired, DbWriteEnqueueResult::Enqueued);
    assert_eq!(released, DbWriteEnqueueResult::Enqueued);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if repository.records().len() == 2 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should persist active-client mirror commands"));

    assert_eq!(
        repository.records(),
        vec![
            RecordedDbWrite::ActiveClientAcquired {
                route_band: RouteBand::Responses,
                process_run_id: "process-runtime".to_owned(),
                reservation_id: reservation_id.clone(),
                account_id,
                acquired_unix_seconds: 1_000,
                active_pressure: 2,
            },
            RecordedDbWrite::ActiveClientReleased {
                route_band: RouteBand::Responses,
                process_run_id: "process-runtime".to_owned(),
                reservation_id,
                released_unix_seconds: 1_100,
            },
        ]
    );

    actor.shutdown().await;
}

#[tokio::test]
async fn affinity_owner_persistence_uses_buffered_write_class_with_freshness_label() {
    let repository = Arc::new(RecordingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 4);
    let owner = PreviousResponseAffinityOwnerRecord::new(
        AffinityKeyHash::new("a".repeat(64))
            .unwrap_or_else(|error| panic!("test affinity hash should validate: {error}")),
        account_id("acct_affinity_actor"),
        7,
        RouteBand::Responses,
        AffinitySourceTransport::HttpSse,
        1_000,
    );

    let enqueue_result = actor.try_enqueue(DbWriteCommand::previous_response_affinity_owner(
        owner.clone(),
    ));

    assert_eq!(enqueue_result, DbWriteEnqueueResult::Enqueued);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if repository.records().len() == 1 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should persist affinity owner command"));

    assert_eq!(
        repository.records(),
        vec![RecordedDbWrite::PreviousResponseAffinityOwner(owner)]
    );

    actor.shutdown().await;
}

#[tokio::test]
async fn quota_exhaustion_enqueue_is_non_blocking_for_socket_signal_class() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 1);
    let account_id = account_id("acct_actor_quota");

    let enqueue_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id.clone(),
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_000,
    ));

    assert_eq!(enqueue_result, DbWriteEnqueueResult::Enqueued);
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should receive queued command"));

    let second_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id.clone(),
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_001,
    ));
    let full_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id,
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_002,
    ));

    assert_eq!(second_result, DbWriteEnqueueResult::Enqueued);
    assert_eq!(full_result, DbWriteEnqueueResult::FullDegraded);
    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn active_mirror_queue_pressure_does_not_consume_provider_exhaustion_capacity() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 1);
    let account_id = account_id("acct_actor_quota_reserved_capacity");
    let active_reservation_id = ReservationId::new("reservation_active_pressure");

    let active_in_flight_result = actor.try_enqueue(DbWriteCommand::active_client_acquired(
        RouteBand::Responses,
        "process-runtime".to_owned(),
        active_reservation_id.clone(),
        account_id.clone(),
        1_000,
        2,
    ));
    assert_eq!(active_in_flight_result, DbWriteEnqueueResult::Enqueued);
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should enter blocked active mirror write"));

    let active_queued_result = actor.try_enqueue(DbWriteCommand::active_client_released(
        RouteBand::Responses,
        "process-runtime".to_owned(),
        active_reservation_id,
        1_001,
    ));
    assert_eq!(active_queued_result, DbWriteEnqueueResult::Enqueued);

    let provider_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id,
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_002,
    ));

    assert_eq!(provider_result, DbWriteEnqueueResult::Enqueued);

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn active_mirror_queue_pressure_does_not_consume_affinity_owner_capacity() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let queue_health = RouteBandQueueHealth::default();
    let actor = DbWriteActor::start_on_handle(
        &tokio::runtime::Handle::current(),
        repository.clone(),
        queue_health.clone(),
        1,
    );
    let account_id = account_id("acct_affinity_reserved_capacity");
    let active_reservation_id = ReservationId::new("reservation_active_affinity_pressure");

    let active_in_flight_result = actor.try_enqueue(DbWriteCommand::active_client_acquired(
        RouteBand::Responses,
        "process-runtime".to_owned(),
        active_reservation_id.clone(),
        account_id.clone(),
        1_000,
        2,
    ));
    assert_eq!(active_in_flight_result, DbWriteEnqueueResult::Enqueued);
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should enter blocked active mirror write"));

    let active_queued_result = actor.try_enqueue(DbWriteCommand::active_client_released(
        RouteBand::Responses,
        "process-runtime".to_owned(),
        active_reservation_id,
        1_001,
    ));
    assert_eq!(active_queued_result, DbWriteEnqueueResult::Enqueued);

    let affinity_owner = PreviousResponseAffinityOwnerRecord::new(
        AffinityKeyHash::new("b".repeat(64))
            .unwrap_or_else(|error| panic!("test affinity hash should validate: {error}")),
        account_id,
        7,
        RouteBand::Responses,
        AffinitySourceTransport::HttpSse,
        1_002,
    );
    let affinity_result = actor.try_enqueue(DbWriteCommand::previous_response_affinity_owner(
        affinity_owner,
    ));

    assert_eq!(affinity_result, DbWriteEnqueueResult::Enqueued);
    route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).unwrap_or_else(
        |error| {
            panic!("best-effort active mirror pressure must not degrade affinity routing: {error}")
        },
    );

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn session_affinity_queue_pressure_does_not_consume_affinity_owner_capacity() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 1);

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::session_account_affinity(
            RouteBand::Responses,
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-blocking",
                account_id("acct_session_blocking"),
                1_000,
            ),
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("session affinity write should block the actor"));
    assert_eq!(
        actor.try_enqueue(DbWriteCommand::session_account_affinity(
            RouteBand::Responses,
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-queued",
                account_id("acct_session_queued"),
                1_001,
            ),
        )),
        DbWriteEnqueueResult::Enqueued
    );
    let owner = PreviousResponseAffinityOwnerRecord::new(
        AffinityKeyHash::new("b".repeat(64))
            .unwrap_or_else(|error| panic!("test affinity hash should validate: {error}")),
        account_id("acct_affinity_separate_capacity"),
        1,
        RouteBand::Responses,
        AffinitySourceTransport::HttpSse,
        1_002,
    );

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::previous_response_affinity_owner(owner)),
        DbWriteEnqueueResult::Enqueued,
        "session refresh pressure must not consume critical affinity-owner capacity"
    );

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn closed_session_affinity_queue_does_not_close_critical_affinity_queue() {
    let repository = Arc::new(RecordingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository, 1);
    let session_task = actor
        .session_affinity_task
        .lock()
        .await
        .take()
        .unwrap_or_else(|| panic!("session scheduler task should exist"));
    session_task.abort();
    let _join_result = session_task.await;

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::session_account_affinity(
            RouteBand::Responses,
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-closed-optional-queue",
                account_id("acct_session_closed"),
                1_000,
            ),
        )),
        DbWriteEnqueueResult::ClosedDegraded
    );
    let owner = PreviousResponseAffinityOwnerRecord::new(
        AffinityKeyHash::new("c".repeat(64))
            .unwrap_or_else(|error| panic!("test affinity hash should validate: {error}")),
        account_id("acct_critical_still_open"),
        1,
        RouteBand::Responses,
        AffinitySourceTransport::HttpSse,
        1_001,
    );

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::previous_response_affinity_owner(owner)),
        DbWriteEnqueueResult::Enqueued
    );
    assert!(
        route_band_queue_health_allows_selection(
            &actor.route_band_queue_health,
            RouteBand::Responses,
        )
        .is_ok()
    );

    actor.shutdown().await;
}

#[test]
fn db_write_actor_commands_do_not_accept_raw_provider_body() {
    let account_id = account_id("acct_no_raw_body");
    let command = DbWriteCommand::provider_quota_exhausted(
        account_id,
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_000,
    );
    let command_debug = format!("{command:?}");

    assert!(!command_debug.contains("usage_limit_reached"));
    assert!(!command_debug.contains("raw-provider-body-canary"));
    assert!(command_debug.contains("AccountQuotaExhausted"));
}

#[tokio::test]
async fn enqueue_after_shutdown_returns_closed_degraded() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository, 1);
    actor.shutdown().await;

    let enqueue_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id("acct_closed"),
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_000,
    ));

    assert_eq!(enqueue_result, DbWriteEnqueueResult::ClosedDegraded);
}

#[tokio::test]
async fn request_shutdown_closes_admission_before_joining_actor_tasks() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository, 1);

    actor.request_shutdown();

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id("acct_request_shutdown"),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_000,
        )),
        DbWriteEnqueueResult::ClosedDegraded
    );
    actor.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_shutdown_retains_both_db_write_handles_for_concurrent_retry() {
    let repository = Arc::new(ShutdownBlockedDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 4);
    let initial_account_id = account_id("acct_retained_db_write_shutdown");
    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            initial_account_id.clone(),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_000,
        )),
        DbWriteEnqueueResult::Enqueued
    );
    assert_eq!(
        actor.try_enqueue(DbWriteCommand::session_account_affinity(
            RouteBand::Responses,
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session_retained_db_write_shutdown",
                initial_account_id,
                1_000,
            ),
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        repository.provider_entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("provider write task should enter its repository"));

    let mut interrupted_shutdown = Box::pin(actor.shutdown());
    let interrupted_poll = futures_util::future::poll_fn(|context| {
        Poll::Ready(interrupted_shutdown.as_mut().poll(context))
    })
    .await;
    assert!(
        matches!(interrupted_poll, Poll::Pending),
        "shutdown should wait for the active DB-write and session-affinity tasks"
    );
    drop(interrupted_shutdown);

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id("acct_after_cancelled_db_write_shutdown"),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_001,
        )),
        DbWriteEnqueueResult::ClosedDegraded
    );
    assert_eq!(
        actor.try_enqueue(DbWriteCommand::session_account_affinity(
            RouteBand::Responses,
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session_after_cancelled_db_write_shutdown",
                account_id("acct_after_cancelled_db_write_shutdown"),
                1_001,
            ),
        )),
        DbWriteEnqueueResult::ClosedDegraded
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        repository.affinity_entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("session-affinity task should enter its repository"));

    let mut repeated_shutdown = Box::pin(actor.shutdown());
    let repeated_poll = futures_util::future::poll_fn(|context| {
        Poll::Ready(repeated_shutdown.as_mut().poll(context))
    })
    .await;
    assert!(
        matches!(repeated_poll, Poll::Pending),
        "a repeated shutdown must retain and join both still-running actor tasks"
    );
    let mut concurrent_shutdown = Box::pin(actor.shutdown());
    let concurrent_poll = futures_util::future::poll_fn(|context| {
        Poll::Ready(concurrent_shutdown.as_mut().poll(context))
    })
    .await;
    assert!(
        matches!(concurrent_poll, Poll::Pending),
        "concurrent shutdown must wait behind the stored task joins"
    );

    repository.provider_release.notify_one();
    repository.affinity_release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(repeated_shutdown, concurrent_shutdown);
        actor.shutdown().await;
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("all repeated DB-write shutdown joins should complete"));

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id("acct_after_db_write_shutdown_completion"),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_002,
        )),
        DbWriteEnqueueResult::ClosedDegraded
    );
}

#[tokio::test]
async fn shutdown_cancels_blocked_repository_write_within_threshold() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 1);

    let enqueue_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id("acct_shutdown_blocked"),
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_000,
    ));
    assert_eq!(enqueue_result, DbWriteEnqueueResult::Enqueued);
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should enter blocking repository write"));

    tokio::time::timeout(std::time::Duration::from_millis(300), actor.shutdown())
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("db write actor shutdown must cancel blocked repository writes")
        });

    let enqueue_after_shutdown = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id("acct_after_shutdown"),
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_001,
    ));
    assert_eq!(enqueue_after_shutdown, DbWriteEnqueueResult::ClosedDegraded);
}

#[tokio::test]
async fn shutdown_drains_finite_provider_quota_write_before_closing() {
    let repository = Arc::new(SlowRecordingDbWriteRepository::new(
        std::time::Duration::from_millis(30),
    ));
    let actor = DbWriteActor::start(repository.clone(), 1);
    let account_id = account_id("acct_shutdown_drain");

    let enqueue_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id.clone(),
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_000,
    ));
    assert_eq!(enqueue_result, DbWriteEnqueueResult::Enqueued);

    actor.shutdown().await;

    assert_eq!(
        repository.records(),
        vec![RecordedDbWrite::ProviderQuotaExhausted {
            account_id,
            route_band: RouteBand::Responses,
            classification: ProviderErrorClassification::AccountQuotaExhausted,
            observed_unix_seconds: 1_000,
        }]
    );
}

#[tokio::test]
async fn quota_observation_queue_overflow_records_degraded_without_sensitive_labels() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 1);
    let account_id = account_id("acct_raw_canary");

    let first_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id.clone(),
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_000,
    ));
    assert_eq!(first_result, DbWriteEnqueueResult::Enqueued);
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should receive queued command"));

    let queued_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id.clone(),
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_001,
    ));
    let overflow_result = actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
        account_id,
        RouteBand::Responses,
        ProviderErrorClassification::AccountQuotaExhausted,
        1_002,
    ));

    assert_eq!(queued_result, DbWriteEnqueueResult::Enqueued);
    assert_eq!(overflow_result, DbWriteEnqueueResult::FullDegraded);

    let degraded_event = actor
        .last_degraded_event()
        .unwrap_or_else(|| panic!("queue overflow should emit degraded event"));
    let rendered_event = format!("{degraded_event:?}");
    assert!(rendered_event.contains("provider_quota_exhaustion"));
    assert!(rendered_event.contains("Full"));
    assert!(!rendered_event.contains("acct_raw_canary"));
    assert!(!rendered_event.contains("raw-provider-body-canary"));
    assert!(!rendered_event.contains("prompt-canary"));
    assert!(!rendered_event.contains("sk-live-token-canary"));
    assert!(!rendered_event.contains("reservation_raw_canary"));
    assert!(!rendered_event.contains("/Users/shravansunder"));

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn queued_provider_quota_write_records_nonzero_queue_lag_before_processing() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 2);

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id("acct_queue_lag_first"),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_000,
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should enter first blocking write"));

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id("acct_queue_lag_second"),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_001,
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    repository.release.notify_waiters();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if actor
                .last_queue_lag_event()
                .is_some_and(|event| event.lag_millis() >= 1)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("queued write should emit non-zero queue lag"));

    let event = actor
        .last_queue_lag_event()
        .unwrap_or_else(|| panic!("queue lag event should be recorded"));
    assert_eq!(event.queue_name(), "provider_quota_exhaustion");
    assert_eq!(event.route_band(), "responses");

    actor.shutdown().await;
}

#[tokio::test]
async fn quota_observation_queue_overflow_marks_route_band_queue_degraded() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let queue_health = RouteBandQueueHealth::default();
    let actor = DbWriteActor::start_on_handle(
        &tokio::runtime::Handle::current(),
        repository.clone(),
        queue_health.clone(),
        1,
    );
    let account_id = account_id("acct_queue_health_full");

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id.clone(),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_000,
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should enter blocking repository write"));
    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id.clone(),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_001,
        )),
        DbWriteEnqueueResult::Enqueued
    );
    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id,
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_002,
        )),
        DbWriteEnqueueResult::FullDegraded
    );

    assert!(route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).is_err());

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn successful_write_below_low_water_clears_full_queue_degraded_health() {
    let repository = Arc::new(BlockingDbWriteRepository::default());
    let queue_health = RouteBandQueueHealth::default();
    let actor = DbWriteActor::start_on_handle(
        &tokio::runtime::Handle::current(),
        repository.clone(),
        queue_health.clone(),
        1,
    );
    let account_id = account_id("acct_queue_health_recover");

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id.clone(),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_000,
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("actor should enter blocking repository write"));
    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id.clone(),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_001,
        )),
        DbWriteEnqueueResult::Enqueued
    );
    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id,
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_002,
        )),
        DbWriteEnqueueResult::FullDegraded
    );

    repository.release.notify_waiters();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).is_ok()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| {
        panic!("successful below-low-water write should clear full degraded health")
    });

    actor.shutdown().await;
}

#[test]
fn successful_write_keeps_route_band_degraded_until_queued_depth_below_low_water() {
    let queue_health = RouteBandQueueHealth::default();
    super::mark_route_band_queue_degraded_for_queue(
        &queue_health,
        RouteBand::Responses,
        super::PROVIDER_EXHAUSTION_QUEUE_NAME,
        RouteBandQueueDegradedReason::DbWriteQueueFull,
        1_000,
    )
    .unwrap_or_else(|error| panic!("test should mark queue degraded: {error}"));

    super::apply_db_write_command_result(
        super::DbWriteCommandResult::Succeeded {
            route_band: RouteBand::Responses,
            queue_name: super::PROVIDER_EXHAUSTION_QUEUE_NAME,
        },
        &queue_health,
        32,
        128,
        32,
    );

    assert!(
        route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).is_err(),
        "queue health must stay degraded while 96 of 128 items remain queued"
    );

    super::apply_db_write_command_result(
        super::DbWriteCommandResult::Succeeded {
            route_band: RouteBand::Responses,
            queue_name: super::PROVIDER_EXHAUSTION_QUEUE_NAME,
        },
        &queue_health,
        97,
        128,
        32,
    );

    route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).unwrap_or_else(
        |error| panic!("queue health should recover once queued depth is below low-water: {error}"),
    );
}

#[test]
fn successful_write_from_other_queue_does_not_clear_provider_queue_degraded_health() {
    let queue_health = RouteBandQueueHealth::default();
    super::mark_route_band_queue_degraded_for_queue(
        &queue_health,
        RouteBand::Responses,
        super::PROVIDER_EXHAUSTION_QUEUE_NAME,
        RouteBandQueueDegradedReason::DbWriteQueueFull,
        1_000,
    )
    .unwrap_or_else(|error| panic!("test should mark provider queue degraded: {error}"));

    super::apply_db_write_command_result(
        super::DbWriteCommandResult::Succeeded {
            route_band: RouteBand::Responses,
            queue_name: "affinity_owner",
        },
        &queue_health,
        128,
        128,
        32,
    );

    assert!(
        route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).is_err(),
        "a successful write without matching queue identity must not clear unrelated queue degradation"
    );
}
