use super::test_support::*;

use std::sync::Arc;

use codex_router_core::routes::RouteBand;

use crate::account_selection::RouteBandQueueDegradedReason;
use crate::account_selection::RouteBandQueueHealth;
use crate::account_selection::route_band_queue_health_allows_selection;
use crate::provider_error::ProviderErrorClassification;
use codex_router_state::session_account_affinity::SessionAccountAffinity;

use super::DbWriteActor;
use super::DbWriteCommand;
use super::DbWriteEnqueueResult;

#[tokio::test]
async fn successful_session_affinity_write_does_not_clear_critical_affinity_degradation() {
    let queue_health = RouteBandQueueHealth::default();
    super::mark_route_band_queue_degraded_for_queue(
        &queue_health,
        RouteBand::Responses,
        "affinity_owner",
        RouteBandQueueDegradedReason::DbWriteFailed,
        1_000,
    )
    .unwrap_or_else(|error| panic!("test should mark affinity queue degraded: {error}"));
    let repository = RecordingDbWriteRepository::default();
    let result = super::handle_db_write_command(
        &repository,
        DbWriteCommand::session_account_affinity(
            RouteBand::Responses,
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-success",
                account_id("acct_session_success"),
                1_001,
            ),
        ),
    )
    .await;

    assert!(matches!(
        result,
        super::DbWriteCommandResult::SucceededNoRoutingEffect
    ));
    super::apply_db_write_command_result(result, &queue_health, 128, 128, 32);
    assert!(
        route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).is_err(),
        "optional session affinity success must not clear critical affinity degradation"
    );
}

#[tokio::test]
async fn quota_write_failure_marks_route_band_degraded_until_recovery() {
    let repository = Arc::new(FailingOnceDbWriteRepository::default());
    let queue_health = RouteBandQueueHealth::default();
    let actor = DbWriteActor::start_on_handle(
        &tokio::runtime::Handle::current(),
        repository.clone(),
        queue_health.clone(),
        2,
    );

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id("acct_write_failure"),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_000,
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses)
                .is_err()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| {
        panic!("failed durable write should degrade route-band queue health")
    });

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id("acct_write_recovery"),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_001,
        )),
        DbWriteEnqueueResult::Enqueued
    );
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
        panic!("successful durable write should recover failed-write degraded health")
    });

    actor.shutdown().await;
}

#[tokio::test]
async fn quota_write_failure_does_not_recover_after_read_only_probe_without_write_success() {
    let repository = Arc::new(FailingWriteRepository);
    let queue_health = RouteBandQueueHealth::default();
    let actor = DbWriteActor::start_on_handle(
        &tokio::runtime::Handle::current(),
        repository,
        queue_health.clone(),
        2,
    );

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::provider_quota_exhausted(
            account_id("acct_write_probe_recovery"),
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_000,
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses)
                .is_err()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| {
        panic!("failed durable write should first degrade route-band queue health")
    });

    tokio::task::yield_now().await;

    assert!(
        route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).is_err(),
        "DbWriteFailed must not clear without a successful routed write acknowledgement"
    );

    actor.shutdown().await;
}

#[tokio::test]
async fn session_affinity_write_failure_does_not_degrade_request_routing() {
    let repository = Arc::new(FailingSessionAffinityRepository::default());
    let queue_health = RouteBandQueueHealth::default();
    let actor = DbWriteActor::start_on_handle(
        &tokio::runtime::Handle::current(),
        repository.clone(),
        queue_health.clone(),
        2,
    );

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::session_account_affinity(
            RouteBand::Responses,
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-write-failure",
                account_id("acct_session_write_failure"),
                1_000,
            ),
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        repository.write_attempted.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("session affinity write should be attempted"));

    route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).unwrap_or_else(
        |error| panic!("cache-affinity persistence failure must not block routing: {error}"),
    );
    actor.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn repeated_session_affinity_refreshes_are_not_written_immediately() {
    let repository = Arc::new(RecordingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 4);
    let selected_account_id = account_id("acct_debounced_session");

    assert_eq!(
        actor.try_enqueue(DbWriteCommand::session_account_affinity(
            RouteBand::Responses,
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-debounced",
                selected_account_id.clone(),
                1_000,
            ),
        )),
        DbWriteEnqueueResult::Enqueued
    );
    for last_seen_unix_seconds in [1_001, 1_002] {
        assert_eq!(
            actor.try_enqueue(DbWriteCommand::session_account_affinity(
                RouteBand::Responses,
                SessionAccountAffinity::new(
                    codex_router_core::provider::Provider::Openai,
                    "session-debounced",
                    selected_account_id.clone(),
                    last_seen_unix_seconds,
                ),
            )),
            DbWriteEnqueueResult::Enqueued
        );
    }

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if !repository.records().is_empty() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("the first session mapping should persist immediately"));
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    assert_eq!(
        repository.records(),
        vec![RecordedDbWrite::SessionAccountAffinity(
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-debounced",
                selected_account_id,
                1_000,
            )
        )],
        "unchanged-account refreshes should wait for the debounce deadline"
    );

    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if repository.records().len() == 2 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("the latest refresh should flush after the debounce"));
    assert_eq!(
        repository.records()[1],
        RecordedDbWrite::SessionAccountAffinity(SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-debounced",
            account_id("acct_debounced_session"),
            1_002,
        )),
        "the debounced write should contain the latest timestamp"
    );

    actor.shutdown().await;
}

#[tokio::test]
async fn simultaneous_first_session_triggers_enqueue_only_one_immediate_write() {
    let repository = Arc::new(RecordingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 4);
    let selected_account_id = account_id("acct_simultaneous_first");

    for last_seen_unix_seconds in [1_000, 1_001, 1_002] {
        assert_eq!(
            actor.try_enqueue(DbWriteCommand::session_account_affinity(
                RouteBand::Responses,
                SessionAccountAffinity::new(
                    codex_router_core::provider::Provider::Openai,
                    "session-simultaneous-first",
                    selected_account_id.clone(),
                    last_seen_unix_seconds,
                ),
            )),
            DbWriteEnqueueResult::Enqueued
        );
    }

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if !repository.records().is_empty() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("first session mapping should persist immediately"));
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    assert_eq!(
        repository.records(),
        vec![RecordedDbWrite::SessionAccountAffinity(
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-simultaneous-first",
                selected_account_id,
                1_000,
            )
        )],
        "concurrent first triggers should coalesce behind the first immediate write"
    );

    actor.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn account_change_cancels_older_pending_session_refresh() {
    let repository = Arc::new(RecordingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 4);
    let first_account_id = account_id("acct_before_change");
    let changed_account_id = account_id("acct_after_change");

    for (account_id, last_seen_unix_seconds) in [
        (first_account_id.clone(), 1_000),
        (first_account_id, 1_001),
        (changed_account_id.clone(), 1_002),
    ] {
        assert_eq!(
            actor.try_enqueue(DbWriteCommand::session_account_affinity(
                RouteBand::Responses,
                SessionAccountAffinity::new(
                    codex_router_core::provider::Provider::Openai,
                    "session-account-change",
                    account_id,
                    last_seen_unix_seconds,
                ),
            )),
            DbWriteEnqueueResult::Enqueued
        );
    }
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    tokio::task::yield_now().await;

    assert_eq!(
        repository.records(),
        vec![
            RecordedDbWrite::SessionAccountAffinity(SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-account-change",
                account_id("acct_before_change"),
                1_000,
            )),
            RecordedDbWrite::SessionAccountAffinity(SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-account-change",
                changed_account_id,
                1_002,
            )),
        ],
        "an older pending refresh must not overwrite an immediate account change"
    );
    actor.shutdown().await;
}

#[tokio::test]
async fn graceful_shutdown_flushes_latest_pending_session_refresh() {
    let repository = Arc::new(RecordingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 4);
    let selected_account_id = account_id("acct_shutdown_refresh");
    for last_seen_unix_seconds in [1_000, 1_001] {
        assert_eq!(
            actor.try_enqueue(DbWriteCommand::session_account_affinity(
                RouteBand::Responses,
                SessionAccountAffinity::new(
                    codex_router_core::provider::Provider::Openai,
                    "session-shutdown-refresh",
                    selected_account_id.clone(),
                    last_seen_unix_seconds,
                ),
            )),
            DbWriteEnqueueResult::Enqueued
        );
    }

    actor.shutdown().await;

    assert_eq!(
        repository.records(),
        vec![
            RecordedDbWrite::SessionAccountAffinity(SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-shutdown-refresh",
                selected_account_id.clone(),
                1_000,
            )),
            RecordedDbWrite::SessionAccountAffinity(SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-shutdown-refresh",
                selected_account_id,
                1_001,
            )),
        ]
    );
}

#[tokio::test]
async fn session_schedule_capacity_evicts_and_flushes_oldest_pending_refresh() {
    let repository = Arc::new(RecordingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 1);

    for (session_id, account_id_value, last_seen_unix_seconds) in [
        ("session-capacity-one", "acct_capacity_one", 1_000),
        ("session-capacity-one", "acct_capacity_one", 1_001),
        ("session-capacity-two", "acct_capacity_two", 1_002),
    ] {
        loop {
            let result = actor.try_enqueue(DbWriteCommand::session_account_affinity(
                RouteBand::Responses,
                SessionAccountAffinity::new(
                    codex_router_core::provider::Provider::Openai,
                    session_id,
                    account_id(account_id_value),
                    last_seen_unix_seconds,
                ),
            ));
            if result == DbWriteEnqueueResult::Enqueued {
                break;
            }
            tokio::task::yield_now().await;
        }
        tokio::task::yield_now().await;
    }
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if repository.records().len() == 3 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("capacity eviction should flush the pending refresh"));

    assert_eq!(
        repository.records(),
        vec![
            RecordedDbWrite::SessionAccountAffinity(SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-capacity-one",
                account_id("acct_capacity_one"),
                1_000,
            )),
            RecordedDbWrite::SessionAccountAffinity(SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-capacity-one",
                account_id("acct_capacity_one"),
                1_001,
            )),
            RecordedDbWrite::SessionAccountAffinity(SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-capacity-two",
                account_id("acct_capacity_two"),
                1_002,
            )),
        ]
    );
    actor.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn continuous_session_activity_flushes_at_the_maximum_wait() {
    let repository = Arc::new(RecordingDbWriteRepository::default());
    let actor = DbWriteActor::start(repository.clone(), 4);
    let account_id = account_id("acct_maximum_wait");
    assert_eq!(
        actor.try_enqueue(DbWriteCommand::session_account_affinity(
            RouteBand::Responses,
            SessionAccountAffinity::new(
                codex_router_core::provider::Provider::Openai,
                "session-maximum-wait",
                account_id.clone(),
                1_000,
            ),
        )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::task::yield_now().await;
    for last_seen_unix_seconds in [1_029, 1_058, 1_087, 1_116] {
        assert_eq!(
            actor.try_enqueue(DbWriteCommand::session_account_affinity(
                RouteBand::Responses,
                SessionAccountAffinity::new(
                    codex_router_core::provider::Provider::Openai,
                    "session-maximum-wait",
                    account_id.clone(),
                    last_seen_unix_seconds,
                ),
            )),
            DbWriteEnqueueResult::Enqueued
        );
        tokio::time::advance(std::time::Duration::from_secs(29)).await;
    }

    assert_eq!(repository.records().len(), 1);
    tokio::time::advance(std::time::Duration::from_secs(4)).await;
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if repository.records().len() == 2 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("continuous activity should flush at maximum wait"));

    assert_eq!(
        repository.records()[1],
        RecordedDbWrite::SessionAccountAffinity(SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-maximum-wait",
            account_id,
            1_116,
        ))
    );
    actor.shutdown().await;
}
