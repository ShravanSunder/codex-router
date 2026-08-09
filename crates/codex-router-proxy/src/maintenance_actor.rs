//! Bounded actor for SQLite maintenance that must never gate socket progress.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_router_core::routes::RouteBand;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use futures_util::future::BoxFuture;
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::telemetry::emit_maintenance_lag_observed;

/// Default bounded capacity for coalesced maintenance hints.
pub const MAINTENANCE_QUEUE_CAPACITY: usize = 32;

/// Result of a non-blocking maintenance hint enqueue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceEnqueueResult {
    /// Hint entered the bounded queue.
    Enqueued,
    /// Duplicate hint is already queued or running.
    Coalesced,
    /// Queue had no capacity and maintenance is degraded.
    FullDegraded,
    /// Actor is closed and maintenance is degraded.
    ClosedDegraded,
}

/// Coalescible maintenance hint.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum MaintenanceHint {
    /// Cleanup session-account affinities last observed before the cutoff.
    CleanupStaleSessionAccountAffinities { stale_before_unix_seconds: u64 },
    /// Cleanup active-client leases older than the cutoff for one route band.
    CleanupStaleActiveClients {
        route_band: RouteBand,
        stale_before_unix_seconds: u64,
    },
    /// Refresh active-session rollups for one interval.
    RefreshActiveSessionRollups {
        route_band: RouteBand,
        interval_start_unix_seconds: u64,
        interval_end_unix_seconds: u64,
        bucket_seconds: u64,
    },
    /// Apply retention to persisted active-session rollup buckets for one route band.
    ApplyActiveSessionRetention {
        route_band: RouteBand,
        retain_after_unix_seconds: u64,
    },
    /// Run active-session history compaction for one route band.
    CompactActiveSessionHistory {
        route_band: RouteBand,
        compact_before_unix_seconds: u64,
    },
}

#[derive(Clone, Debug)]
struct QueuedMaintenanceHint {
    hint: MaintenanceHint,
    enqueued_at: Instant,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum MaintenanceCoalescingKey {
    GlobalClass {
        maintenance_class: &'static str,
    },
    RouteBandClass {
        route_band: RouteBand,
        maintenance_class: &'static str,
    },
    RollupInterval {
        route_band: RouteBand,
        interval_start_unix_seconds: u64,
        interval_end_unix_seconds: u64,
        bucket_seconds: u64,
    },
}

/// Maintenance repository boundary.
pub trait MaintenanceRepository: Send + Sync + 'static {
    /// Runs one maintenance hint.
    fn run_maintenance_hint<'a>(
        &'a self,
        hint: MaintenanceHint,
    ) -> BoxFuture<'a, Result<(), MaintenanceRepositoryError>>;

    /// Runs active-session history compaction for one route band.
    fn compact_active_session_history<'a>(
        &'a self,
        _route_band: RouteBand,
        _compact_before_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), MaintenanceRepositoryError>> {
        Box::pin(async { Ok(()) })
    }
}

impl<T> MaintenanceRepository for Arc<T>
where
    T: MaintenanceRepository,
{
    fn run_maintenance_hint<'a>(
        &'a self,
        hint: MaintenanceHint,
    ) -> BoxFuture<'a, Result<(), MaintenanceRepositoryError>> {
        self.as_ref().run_maintenance_hint(hint)
    }

    fn compact_active_session_history<'a>(
        &'a self,
        route_band: RouteBand,
        compact_before_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), MaintenanceRepositoryError>> {
        self.as_ref()
            .compact_active_session_history(route_band, compact_before_unix_seconds)
    }
}

/// Maintenance repository failure.
#[derive(Debug, Error)]
pub enum MaintenanceRepositoryError {
    /// SQLite/state failed while running maintenance.
    #[error("state store unavailable while running maintenance")]
    State(#[from] StateStoreError),
}

impl MaintenanceRepository for AsyncSqliteStateStore {
    fn run_maintenance_hint<'a>(
        &'a self,
        hint: MaintenanceHint,
    ) -> BoxFuture<'a, Result<(), MaintenanceRepositoryError>> {
        Box::pin(async move {
            match hint {
                MaintenanceHint::CleanupStaleSessionAccountAffinities {
                    stale_before_unix_seconds,
                } => {
                    self.purge_session_account_affinities_before(stale_before_unix_seconds)
                        .await?;
                }
                MaintenanceHint::CleanupStaleActiveClients {
                    route_band,
                    stale_before_unix_seconds,
                } => {
                    self.active_client_counts_for_route_band(
                        route_band.as_str(),
                        stale_before_unix_seconds,
                        0,
                    )
                    .await?;
                }
                MaintenanceHint::RefreshActiveSessionRollups {
                    route_band,
                    interval_start_unix_seconds,
                    interval_end_unix_seconds,
                    bucket_seconds,
                } => {
                    self.refresh_active_session_rollups_for_interval(
                        route_band.as_str(),
                        interval_start_unix_seconds,
                        interval_end_unix_seconds,
                        bucket_seconds,
                    )
                    .await?;
                }
                MaintenanceHint::ApplyActiveSessionRetention {
                    retain_after_unix_seconds,
                    ..
                } => {
                    self.purge_active_session_rollups_before(retain_after_unix_seconds)
                        .await?;
                }
                MaintenanceHint::CompactActiveSessionHistory {
                    route_band,
                    compact_before_unix_seconds,
                } => {
                    self.compact_active_session_history(route_band, compact_before_unix_seconds)
                        .await?;
                }
            }
            Ok(())
        })
    }
}

/// Bounded maintenance actor handle.
#[derive(Clone, Debug)]
pub struct MaintenanceActor {
    sender: mpsc::Sender<QueuedMaintenanceHint>,
    pending: Arc<Mutex<HashMap<MaintenanceCoalescingKey, Instant>>>,
    closed: Arc<AtomicBool>,
    shutdown: CancellationToken,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl MaintenanceActor {
    /// Starts a bounded maintenance actor.
    #[must_use]
    pub fn start<R>(repository: R, capacity: usize) -> Self
    where
        R: MaintenanceRepository,
    {
        let repository = Arc::new(repository);
        Self::start_on_handle(&Handle::current(), repository, capacity)
    }

    /// Starts a bounded maintenance actor from a shared repository.
    #[must_use]
    pub fn start_with_repository(
        repository: Arc<dyn MaintenanceRepository>,
        capacity: usize,
    ) -> Self {
        Self::start_on_handle(&Handle::current(), repository, capacity)
    }

    /// Starts a bounded maintenance actor from a shared repository on an explicit runtime handle.
    #[must_use]
    pub fn start_on_handle(
        runtime_handle: &Handle,
        repository: Arc<dyn MaintenanceRepository>,
        capacity: usize,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(capacity);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let shutdown = CancellationToken::new();
        let task_pending = Arc::clone(&pending);
        let task_closed = Arc::clone(&closed);
        let task_shutdown = shutdown.clone();
        let task = runtime_handle.spawn(async move {
            run_maintenance_actor(
                repository,
                receiver,
                task_pending,
                task_shutdown,
                task_closed,
            )
            .await;
        });
        Self {
            sender,
            pending,
            closed,
            shutdown,
            task: Arc::new(Mutex::new(Some(task))),
        }
    }

    /// Attempts to enqueue without waiting on maintenance work.
    #[must_use]
    pub fn try_enqueue(&self, hint: MaintenanceHint) -> MaintenanceEnqueueResult {
        let enqueued_at = Instant::now();
        if self.closed.load(Ordering::Acquire) {
            emit_maintenance_lag_observed(
                hint.maintenance_class(),
                hint.route_band_label(),
                "degraded",
                0,
            );
            return MaintenanceEnqueueResult::ClosedDegraded;
        }
        let coalescing_key = hint.coalescing_key();
        {
            let mut pending = match self.pending.lock() {
                Ok(pending) => pending,
                Err(error) => {
                    tracing::warn!(
                        component = "maintenance_actor",
                        lock = "pending_set",
                        error.message = %error,
                        "codex_router.maintenance_pending_set_lock_poisoned"
                    );
                    emit_maintenance_lag_observed(
                        hint.maintenance_class(),
                        hint.route_band_label(),
                        "degraded",
                        0,
                    );
                    return MaintenanceEnqueueResult::ClosedDegraded;
                }
            };
            if let Some(existing_enqueued_at) = pending.get(&coalescing_key) {
                emit_maintenance_lag_observed(
                    hint.maintenance_class(),
                    hint.route_band_label(),
                    "coalesced",
                    lag_millis_since(*existing_enqueued_at),
                );
                return MaintenanceEnqueueResult::Coalesced;
            }
            pending.insert(coalescing_key.clone(), enqueued_at);
        }
        let queued_hint = QueuedMaintenanceHint {
            hint: hint.clone(),
            enqueued_at,
        };
        match self.sender.try_send(queued_hint) {
            Ok(()) => MaintenanceEnqueueResult::Enqueued,
            Err(mpsc::error::TrySendError::Full(_hint)) => {
                self.remove_pending_key(&coalescing_key);
                emit_maintenance_lag_observed(
                    hint.maintenance_class(),
                    hint.route_band_label(),
                    "degraded",
                    0,
                );
                MaintenanceEnqueueResult::FullDegraded
            }
            Err(mpsc::error::TrySendError::Closed(_hint)) => {
                self.remove_pending_key(&coalescing_key);
                self.closed.store(true, Ordering::Release);
                emit_maintenance_lag_observed(
                    hint.maintenance_class(),
                    hint.route_band_label(),
                    "degraded",
                    0,
                );
                MaintenanceEnqueueResult::ClosedDegraded
            }
        }
    }

    /// Cancels the actor and waits for the task to stop.
    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        self.shutdown.cancel();
        let task = match self.task.lock() {
            Ok(mut task) => task.take(),
            Err(error) => {
                tracing::warn!(
                    component = "maintenance_actor",
                    lock = "task",
                    error.message = %error,
                    "codex_router.maintenance_task_lock_poisoned"
                );
                None
            }
        };
        if let Some(task) = task {
            let _join_result = task.await;
        }
    }

    fn remove_pending_key(&self, coalescing_key: &MaintenanceCoalescingKey) {
        match self.pending.lock() {
            Ok(mut pending) => {
                pending.remove(coalescing_key);
            }
            Err(error) => {
                tracing::warn!(
                    component = "maintenance_actor",
                    lock = "pending_set",
                    error.message = %error,
                    "codex_router.maintenance_pending_remove_lock_poisoned"
                );
            }
        }
    }
}

impl MaintenanceHint {
    fn coalescing_key(&self) -> MaintenanceCoalescingKey {
        match self {
            Self::CleanupStaleSessionAccountAffinities { .. } => {
                MaintenanceCoalescingKey::GlobalClass {
                    maintenance_class: self.maintenance_class(),
                }
            }
            Self::RefreshActiveSessionRollups {
                route_band,
                interval_start_unix_seconds,
                interval_end_unix_seconds,
                bucket_seconds,
            } => MaintenanceCoalescingKey::RollupInterval {
                route_band: *route_band,
                interval_start_unix_seconds: *interval_start_unix_seconds,
                interval_end_unix_seconds: *interval_end_unix_seconds,
                bucket_seconds: *bucket_seconds,
            },
            Self::CleanupStaleActiveClients { route_band, .. }
            | Self::ApplyActiveSessionRetention { route_band, .. }
            | Self::CompactActiveSessionHistory { route_band, .. } => {
                MaintenanceCoalescingKey::RouteBandClass {
                    route_band: *route_band,
                    maintenance_class: self.maintenance_class(),
                }
            }
        }
    }

    const fn maintenance_class(&self) -> &'static str {
        match self {
            Self::CleanupStaleSessionAccountAffinities { .. } => {
                "stale_session_account_affinity_cleanup"
            }
            Self::CleanupStaleActiveClients { .. } => "stale_active_client_cleanup",
            Self::RefreshActiveSessionRollups { .. } => "active_session_rollup_refresh",
            Self::ApplyActiveSessionRetention { .. } => "active_session_retention",
            Self::CompactActiveSessionHistory { .. } => "active_session_history_compaction",
        }
    }

    const fn route_band_label(&self) -> &'static str {
        match self {
            Self::CleanupStaleSessionAccountAffinities { .. } => "all",
            Self::CleanupStaleActiveClients { route_band, .. }
            | Self::RefreshActiveSessionRollups { route_band, .. }
            | Self::ApplyActiveSessionRetention { route_band, .. }
            | Self::CompactActiveSessionHistory { route_band, .. } => route_band.as_str(),
        }
    }
}

async fn run_maintenance_actor(
    repository: Arc<dyn MaintenanceRepository>,
    mut receiver: mpsc::Receiver<QueuedMaintenanceHint>,
    pending: Arc<Mutex<HashMap<MaintenanceCoalescingKey, Instant>>>,
    shutdown: CancellationToken,
    closed: Arc<AtomicBool>,
) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                receiver.close();
                break;
            }
            queued_hint = receiver.recv() => {
                let Some(queued_hint) = queued_hint else {
                    break;
                };
                let QueuedMaintenanceHint { hint, enqueued_at } = queued_hint;
                let hint_for_cleanup = hint.clone();
                let coalescing_key = hint.coalescing_key();
                emit_maintenance_lag_observed(
                    hint.maintenance_class(),
                    hint.route_band_label(),
                    "processing",
                    lag_millis_since(enqueued_at),
                );
                tokio::select! {
                    () = shutdown.cancelled() => {
                        match pending.lock() {
                            Ok(mut pending) => {
                                pending.remove(&coalescing_key);
                            }
                            Err(error) => {
                                tracing::warn!(
                                    component = "maintenance_actor",
                                    lock = "pending_set",
                                    error.message = %error,
                                    "codex_router.maintenance_pending_shutdown_lock_poisoned"
                                );
                            }
                        }
                        receiver.close();
                        break;
                    }
                    _result = repository.run_maintenance_hint(hint.clone()) => {}
                }
                match pending.lock() {
                    Ok(mut pending) => {
                        pending.remove(&hint_for_cleanup.coalescing_key());
                    }
                    Err(error) => {
                        tracing::warn!(
                            component = "maintenance_actor",
                            lock = "pending_set",
                            error.message = %error,
                            "codex_router.maintenance_pending_cleanup_lock_poisoned"
                        );
                    }
                }
            }
        }
    }
    closed.store(true, Ordering::Release);
}

fn lag_millis_since(enqueued_at: Instant) -> u64 {
    u64::try_from(enqueued_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use codex_router_core::ids::AccountId;
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
    async fn maintenance_hint_coalescing_emits_scrubbed_telemetry() {
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

        assert!(rendered_log.contains("codex_router.maintenance_lag_observed"));
        assert!(rendered_log.contains("active_session_rollup_refresh"));
        assert!(rendered_log.contains("responses"));
        assert!(rendered_log.contains("coalesced"));
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
            "session-old",
            AccountId::new("acct_old")
                .unwrap_or_else(|error| panic!("test account should validate: {error}")),
            999,
        );
        let cutoff_affinity = SessionAccountAffinity::new(
            "session-cutoff",
            AccountId::new("acct_cutoff")
                .unwrap_or_else(|error| panic!("test account should validate: {error}")),
            1_000,
        );
        for affinity in [&old_affinity, &cutoff_affinity] {
            AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(
                &store, affinity,
            )
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
    async fn coalesced_maintenance_lag_uses_original_enqueue_age() {
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

        assert!(rendered_log.contains("codex_router.maintenance_lag_observed"));
        assert!(
            !rendered_log.contains("maintenance.lag_millis=0"),
            "coalesced maintenance lag must be measured from original enqueue time: {rendered_log}"
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

        assert!(rendered_log.contains("codex_router.maintenance_lag_observed"));
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

    #[test]
    fn maintenance_actor_exposes_stale_cleanup_retention_and_compaction_hints() {
        let actor_source = include_str!("maintenance_actor.rs");
        let hint_enum_start = actor_source
            .find("pub enum MaintenanceHint")
            .unwrap_or_else(|| panic!("MaintenanceHint enum must exist"));
        let hint_enum_end = actor_source
            .find("/// Maintenance repository boundary.")
            .unwrap_or_else(|| {
                panic!("MaintenanceRepository boundary must follow MaintenanceHint")
            });
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
}
