use std::env;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::task::Poll;
use std::time::Duration;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::ReservationId;
use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;
use codex_router_state::session_account_affinity::SessionAccountAffinity;
use codex_router_state::sqlite::AsyncSessionAccountAffinityRepository;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use futures_util::future::BoxFuture;
use tokio::sync::Notify;

use super::MaintenanceActor;
use super::MaintenanceEnqueueResult;
use super::MaintenanceHint;
use super::MaintenanceRepository;
use super::MaintenanceRepositoryError;
use crate::test_log_capture::capture_log_output;
use crate::test_log_capture::capture_log_output_async;

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[tokio::test]
async fn maintenance_hints_coalesce_without_request_admission_waiting() {
    let repository = Arc::new(BlockingMaintenanceRepository::default());
    let actor = MaintenanceActor::start(repository.clone(), 8);
    let hint = refresh_rollups_hint();

    let enqueue_result = tokio::time::timeout(Duration::from_millis(25), async {
        actor.try_enqueue(hint.clone())
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("maintenance hint enqueue must not await maintenance"));
    assert_eq!(enqueue_result, MaintenanceEnqueueResult::Enqueued);

    tokio::time::timeout(Duration::from_secs(1), repository.entered.notified())
        .await
        .unwrap_or_else(|_elapsed| panic!("actor should begin processing first hint"));

    let duplicate_result = tokio::time::timeout(Duration::from_millis(25), async {
        actor.try_enqueue(hint.clone())
    })
    .await
    .unwrap_or_else(|_elapsed| {
        panic!("duplicate maintenance hint coalescing must not wait for active maintenance")
    });
    assert_eq!(duplicate_result, MaintenanceEnqueueResult::Coalesced);

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn maintenance_hint_coalescing_does_not_emit_a_warning_log() {
    let repository = Arc::new(BlockingMaintenanceRepository::default());
    let actor = MaintenanceActor::start(repository.clone(), 8);
    let hint = refresh_rollups_hint();

    assert_eq!(
        actor.try_enqueue(hint.clone()),
        MaintenanceEnqueueResult::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(1), repository.entered.notified())
        .await
        .unwrap_or_else(|_elapsed| panic!("actor should begin processing first hint"));

    let rendered_log = capture_log_output(|| {
        assert_eq!(actor.try_enqueue(hint), MaintenanceEnqueueResult::Coalesced);
    });

    assert!(
        rendered_log.is_empty(),
        "normal coalescing emitted a warning log: {rendered_log}"
    );
    assert!(!rendered_log.contains("raw-provider-body-canary"));
    assert!(!rendered_log.contains("sk-live-token-canary"));
    assert!(!rendered_log.contains("Authorization"));
    assert!(!rendered_log.contains("acct_raw_canary"));
    assert!(!rendered_log.contains("friendly account label"));
    assert!(!rendered_log.contains("reservation_raw_canary"));
    assert!(!rendered_log.contains("/Users/shravansunder"));

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn stale_cleanup_hints_coalesce_by_route_band_and_class_without_cutoff_timestamp() {
    let repository = Arc::new(BlockingMaintenanceRepository::default());
    let actor = MaintenanceActor::start(repository.clone(), 8);
    let first_hint = MaintenanceHint::CleanupStaleActiveClients {
        route_band: RouteBand::Responses,
        stale_before_unix_seconds: 1_000,
    };
    let later_cutoff_hint = MaintenanceHint::CleanupStaleActiveClients {
        route_band: RouteBand::Responses,
        stale_before_unix_seconds: 2_000,
    };

    assert_eq!(
        actor.try_enqueue(first_hint),
        MaintenanceEnqueueResult::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(1), repository.entered.notified())
        .await
        .unwrap_or_else(|_elapsed| panic!("actor should begin processing first cleanup hint"));

    assert_eq!(
        actor.try_enqueue(later_cutoff_hint),
        MaintenanceEnqueueResult::Coalesced
    );

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn session_affinity_cleanup_hints_coalesce_without_cutoff_timestamp() {
    let repository = Arc::new(BlockingMaintenanceRepository::default());
    let actor = MaintenanceActor::start(repository.clone(), 8);

    assert_eq!(
        actor.try_enqueue(MaintenanceHint::CleanupStaleSessionAccountAffinities {
            stale_before_unix_seconds: 1_000,
        }),
        MaintenanceEnqueueResult::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(1), repository.entered.notified())
        .await
        .unwrap_or_else(|_elapsed| panic!("actor should begin session affinity cleanup"));
    assert_eq!(
        actor.try_enqueue(MaintenanceHint::CleanupStaleSessionAccountAffinities {
            stale_before_unix_seconds: 2_000,
        }),
        MaintenanceEnqueueResult::Coalesced
    );

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn session_affinity_cleanup_hint_deletes_old_sqlite_rows() {
    let database_path = test_database_path("session_affinity_cleanup_hint");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("test state store should open: {error}"));
    let old_affinity = SessionAccountAffinity::new(
        codex_router_core::provider::Provider::Openai,
        "session-old",
        AccountId::new("acct_old")
            .unwrap_or_else(|error| panic!("test account should validate: {error}")),
        999,
    );
    let cutoff_affinity = SessionAccountAffinity::new(
        codex_router_core::provider::Provider::Openai,
        "session-cutoff",
        AccountId::new("acct_cutoff")
            .unwrap_or_else(|error| panic!("test account should validate: {error}")),
        1_000,
    );
    for affinity in [&old_affinity, &cutoff_affinity] {
        AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(&store, affinity)
            .await
            .unwrap_or_else(|error| panic!("test affinity should persist: {error}"));
    }
    let actor = MaintenanceActor::start(store.clone(), 8);

    assert_eq!(
        actor.try_enqueue(MaintenanceHint::CleanupStaleSessionAccountAffinities {
            stale_before_unix_seconds: 1_000,
        }),
        MaintenanceEnqueueResult::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let old = AsyncSessionAccountAffinityRepository::load_session_account_affinity(
                &store,
                Provider::Openai,
                old_affinity.session_id(),
            )
            .await
            .unwrap_or_else(|error| panic!("old affinity lookup should succeed: {error}"));
            if old.is_none() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("maintenance actor should delete the old affinity"));

    assert_eq!(
        AsyncSessionAccountAffinityRepository::load_session_account_affinity(
            &store,
            Provider::Openai,
            cutoff_affinity.session_id(),
        )
        .await,
        Ok(Some(cutoff_affinity))
    );
    actor.shutdown().await;
    store
        .close()
        .await
        .unwrap_or_else(|error| panic!("test state store should close: {error}"));
}

#[tokio::test]
async fn active_session_history_compaction_hint_deletes_completed_old_sqlite_events() {
    let database_path = test_database_path("active_session_history_compaction_hint");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("test state store should open: {error}"));
    let account = AccountId::new("acct_compaction")
        .unwrap_or_else(|error| panic!("test account should validate: {error}"));
    let reservation = ReservationId::new("reservation-compaction");
    store
        .record_active_client_acquired("responses", "process", &reservation, &account, 100, 1)
        .await
        .unwrap_or_else(|error| panic!("test session acquire should persist: {error}"));
    store
        .record_active_client_released("responses", "process", &reservation, 200)
        .await
        .unwrap_or_else(|error| panic!("test session release should persist: {error}"));
    let actor = MaintenanceActor::start(store.clone(), 8);

    assert_eq!(
        actor.try_enqueue(MaintenanceHint::CompactActiveSessionHistory {
            route_band: RouteBand::Responses,
            compact_before_unix_seconds: 500,
        }),
        MaintenanceEnqueueResult::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let events = store
                .active_session_events_for_route_band("responses")
                .await
                .unwrap_or_else(|error| panic!("active event lookup should succeed: {error}"));
            if events.is_empty() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("maintenance actor should compact completed events"));

    actor.shutdown().await;
    store
        .close()
        .await
        .unwrap_or_else(|error| panic!("test state store should close: {error}"));
}

#[tokio::test]
async fn coalesced_maintenance_remains_silent_after_waiting() {
    let repository = Arc::new(BlockingMaintenanceRepository::default());
    let actor = MaintenanceActor::start(repository.clone(), 8);
    let hint = refresh_rollups_hint();

    assert_eq!(
        actor.try_enqueue(hint.clone()),
        MaintenanceEnqueueResult::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(1), repository.entered.notified())
        .await
        .unwrap_or_else(|_elapsed| panic!("actor should begin processing first hint"));
    tokio::time::sleep(Duration::from_millis(2)).await;

    let rendered_log = capture_log_output(|| {
        assert_eq!(actor.try_enqueue(hint), MaintenanceEnqueueResult::Coalesced);
    });

    assert!(
        rendered_log.is_empty(),
        "normal coalescing emitted a warning log: {rendered_log}"
    );

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test]
async fn maintenance_actor_shutdown_releases_sqlite_handle_without_socket_wait() {
    let database_path = test_database_path("maintenance_actor_shutdown_releases_sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("test state store should open: {error}"));
    let actor = MaintenanceActor::start(store, 8);

    assert_eq!(
        actor.try_enqueue(refresh_rollups_hint()),
        MaintenanceEnqueueResult::Enqueued
    );

    tokio::time::timeout(Duration::from_secs(1), actor.shutdown())
        .await
        .unwrap_or_else(|_elapsed| panic!("maintenance shutdown must not wait on sockets"));

    assert_eq!(
        actor.try_enqueue(refresh_rollups_hint()),
        MaintenanceEnqueueResult::ClosedDegraded
    );

    let reopened = tokio::time::timeout(
        Duration::from_secs(1),
        AsyncSqliteStateStore::open(&database_path),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("sqlite handle should be released after shutdown"))
    .unwrap_or_else(|error| panic!("sqlite should reopen after actor shutdown: {error}"));
    reopened
        .close()
        .await
        .unwrap_or_else(|error| panic!("reopened sqlite store should close: {error}"));
}

#[tokio::test]
async fn request_shutdown_closes_maintenance_admission_before_joining_task() {
    let repository = Arc::new(FailingOnceMaintenanceRepository::default());
    let actor = MaintenanceActor::start(repository, 8);

    actor.request_shutdown();

    assert_eq!(
        actor.try_enqueue(refresh_rollups_hint()),
        MaintenanceEnqueueResult::ClosedDegraded
    );
    actor.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_shutdown_retains_maintenance_handle_for_concurrent_retry() {
    let actor = MaintenanceActor::start(Arc::new(FailingOnceMaintenanceRepository::default()), 8);

    let mut interrupted_shutdown = Box::pin(actor.shutdown());
    let interrupted_poll = futures_util::future::poll_fn(|context| {
        Poll::Ready(interrupted_shutdown.as_mut().poll(context))
    })
    .await;
    assert!(
        matches!(interrupted_poll, Poll::Pending),
        "shutdown should await the actual maintenance task"
    );
    drop(interrupted_shutdown);

    assert_eq!(
        actor.try_enqueue(refresh_rollups_hint()),
        MaintenanceEnqueueResult::ClosedDegraded
    );
    let mut repeated_shutdown = Box::pin(actor.shutdown());
    let repeated_poll = futures_util::future::poll_fn(|context| {
        Poll::Ready(repeated_shutdown.as_mut().poll(context))
    })
    .await;
    assert!(
        matches!(repeated_poll, Poll::Pending),
        "a repeated shutdown must retain and join the same maintenance task"
    );
    let mut concurrent_shutdown = Box::pin(actor.shutdown());
    let concurrent_poll = futures_util::future::poll_fn(|context| {
        Poll::Ready(concurrent_shutdown.as_mut().poll(context))
    })
    .await;
    assert!(
        matches!(concurrent_poll, Poll::Pending),
        "concurrent shutdown must wait behind the stored task join"
    );

    tokio::task::yield_now().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(repeated_shutdown, concurrent_shutdown);
        actor.shutdown().await;
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("all repeated maintenance shutdown joins should complete"));
    assert_eq!(
        actor.try_enqueue(refresh_rollups_hint()),
        MaintenanceEnqueueResult::ClosedDegraded
    );
}

#[tokio::test]
async fn maintenance_actor_shutdown_cancels_blocked_hint_within_threshold() {
    let repository = Arc::new(BlockingMaintenanceRepository::default());
    let actor = MaintenanceActor::start(repository.clone(), 8);

    assert_eq!(
        actor.try_enqueue(refresh_rollups_hint()),
        MaintenanceEnqueueResult::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(1), repository.entered.notified())
        .await
        .unwrap_or_else(|_elapsed| panic!("actor should enter blocking maintenance hint"));

    tokio::time::timeout(Duration::from_millis(50), actor.shutdown())
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("maintenance actor shutdown must cancel blocked maintenance hints")
        });

    assert_eq!(
        actor.try_enqueue(refresh_rollups_hint()),
        MaintenanceEnqueueResult::ClosedDegraded
    );
}

#[tokio::test]
async fn maintenance_actor_degraded_enqueue_emits_scrubbed_lag_log() {
    let repository = Arc::new(BlockingMaintenanceRepository::default());
    let actor = MaintenanceActor::start(repository.clone(), 1);

    assert_eq!(
        actor.try_enqueue(refresh_rollups_hint()),
        MaintenanceEnqueueResult::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(1), repository.entered.notified())
        .await
        .unwrap_or_else(|_elapsed| panic!("actor should enter blocking maintenance hint"));
    assert_eq!(
        actor.try_enqueue(retention_hint()),
        MaintenanceEnqueueResult::Enqueued
    );

    let rendered_log = capture_log_output(|| {
        assert_eq!(
            actor.try_enqueue(compaction_hint()),
            MaintenanceEnqueueResult::FullDegraded
        );
    });

    assert!(rendered_log.contains("codex_router.maintenance_degraded"));
    assert!(rendered_log.contains("active_session_history_compaction"));
    assert!(rendered_log.contains("responses"));
    assert!(rendered_log.contains("degraded"));
    assert!(!rendered_log.contains("raw-provider-body-canary"));
    assert!(!rendered_log.contains("sk-live-token-canary"));
    assert!(!rendered_log.contains("Authorization"));
    assert!(!rendered_log.contains("acct_raw_canary"));
    assert!(!rendered_log.contains("friendly account label"));
    assert!(!rendered_log.contains("reservation_raw_canary"));
    assert!(!rendered_log.contains("/Users/shravansunder"));

    repository.release.notify_waiters();
    actor.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn failed_maintenance_hint_is_degraded_then_allows_a_later_normal_hint() {
    let (rendered_log, ()) = capture_log_output_async(async {
        let repository = Arc::new(FailingOnceMaintenanceRepository::default());
        let actor = MaintenanceActor::start(repository.clone(), 8);
        let hint = refresh_rollups_hint();

        assert_eq!(
            actor.try_enqueue(hint.clone()),
            MaintenanceEnqueueResult::Enqueued
        );
        wait_for_maintenance_call(&repository, 1).await;
        assert_eq!(
            actor.try_enqueue(hint),
            MaintenanceEnqueueResult::Enqueued,
            "a failed hint must remove its pending key before the next normal hint"
        );
        wait_for_maintenance_call(&repository, 2).await;
        assert_eq!(repository.calls.load(Ordering::Acquire), 2);

        actor.shutdown().await;
    })
    .await;

    assert!(rendered_log.contains("codex_router.maintenance_degraded"));
    assert!(rendered_log.contains("active_session_rollup_refresh"));
    assert!(rendered_log.contains("responses"));
    assert!(rendered_log.contains("degraded"));
    assert!(!rendered_log.contains("raw-maintenance-error-canary"));
}

#[test]
fn maintenance_actor_exposes_stale_cleanup_retention_and_compaction_hints() {
    let actor_source = include_str!("../maintenance_actor.rs");
    let hint_enum_start = actor_source
        .find("pub enum MaintenanceHint")
        .unwrap_or_else(|| panic!("MaintenanceHint enum must exist"));
    let hint_enum_end = actor_source
        .find("/// Maintenance repository boundary.")
        .unwrap_or_else(|| panic!("MaintenanceRepository boundary must follow MaintenanceHint"));
    let hint_enum_source = actor_source
        .get(hint_enum_start..hint_enum_end)
        .unwrap_or_else(|| panic!("MaintenanceHint source slice should be valid"));

    for expected_hint_variant in [
        "CleanupStaleActiveClients",
        "ApplyActiveSessionRetention",
        "CompactActiveSessionHistory",
    ] {
        assert!(
            hint_enum_source.contains(expected_hint_variant),
            "MaintenanceActor must expose the R5 maintenance hint variant {expected_hint_variant}"
        );
    }
}

#[derive(Default)]
struct BlockingMaintenanceRepository {
    entered: Notify,
    release: Notify,
}

#[derive(Default)]
struct FailingOnceMaintenanceRepository {
    calls: AtomicUsize,
}

impl MaintenanceRepository for FailingOnceMaintenanceRepository {
    fn run_maintenance_hint<'a>(
        &'a self,
        _hint: MaintenanceHint,
    ) -> BoxFuture<'a, Result<(), MaintenanceRepositoryError>> {
        Box::pin(async move {
            let call_number = self.calls.fetch_add(1, Ordering::AcqRel);
            if call_number == 0 {
                return Err(MaintenanceRepositoryError::State(
                    codex_router_state::sqlite::StateStoreError::Sqlite {
                        message: "raw-maintenance-error-canary".to_owned(),
                    },
                ));
            }
            Ok(())
        })
    }
}

async fn wait_for_maintenance_call(
    repository: &FailingOnceMaintenanceRepository,
    expected_calls: usize,
) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while repository.calls.load(Ordering::Acquire) < expected_calls {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("maintenance actor should process hint {expected_calls}"));
}

impl MaintenanceRepository for BlockingMaintenanceRepository {
    fn run_maintenance_hint<'a>(
        &'a self,
        _hint: MaintenanceHint,
    ) -> BoxFuture<'a, Result<(), MaintenanceRepositoryError>> {
        Box::pin(async move {
            self.entered.notify_waiters();
            self.release.notified().await;
            Ok(())
        })
    }
}

fn refresh_rollups_hint() -> MaintenanceHint {
    MaintenanceHint::RefreshActiveSessionRollups {
        route_band: RouteBand::Responses,
        interval_start_unix_seconds: 1_000,
        interval_end_unix_seconds: 1_300,
        bucket_seconds: 300,
    }
}

fn retention_hint() -> MaintenanceHint {
    MaintenanceHint::ApplyActiveSessionRetention {
        route_band: RouteBand::Responses,
        retain_after_unix_seconds: 1_000,
    }
}

fn compaction_hint() -> MaintenanceHint {
    MaintenanceHint::CompactActiveSessionHistory {
        route_band: RouteBand::Responses,
        compact_before_unix_seconds: 1_000,
    }
}

fn test_database_path(name: &str) -> PathBuf {
    let process_id = std::process::id();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!(
        "codex-router-proxy-{name}-{process_id}-{counter}.sqlite",
    ))
}
