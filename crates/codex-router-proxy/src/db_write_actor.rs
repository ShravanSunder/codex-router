//! Bounded write-side actor for proxy runtime SQLite mirror/proof writes.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::ReservationId;
use codex_router_core::routes::RouteBand;
use futures_util::future::BoxFuture;
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::account_selection::RouteBandQueueDegradedReason;
use crate::account_selection::RouteBandQueueHealth;
use crate::account_selection::clear_route_band_queue_degraded_for_queue;
use crate::account_selection::mark_route_band_queue_degraded_for_queue;
use crate::account_selection::route_band_queue_health_key_prefix;
use crate::provider_error::ProviderErrorClassification;
use crate::telemetry::QueueDegradedEvent;
use crate::telemetry::QueueDegradedReason;
use crate::telemetry::QueueLagEvent;
use crate::telemetry::emit_db_write_queue_degraded;
use crate::telemetry::emit_db_write_queue_lag_observed;
use crate::telemetry::record_db_write_queue_depth;
use crate::telemetry::record_db_write_queue_event;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::session_account_affinity::SessionAccountAffinity;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;

/// Default bounded capacity for durable provider exhaustion writes.
pub const PROVIDER_EXHAUSTION_QUEUE_CAPACITY: usize = 128;
const PROVIDER_EXHAUSTION_QUEUE_NAME: &str = "provider_quota_exhaustion";
const DB_WRITE_SHUTDOWN_DRAIN_GRACE_MS: u64 = 250;

/// Result of a non-blocking write actor enqueue attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DbWriteEnqueueResult {
    /// Command entered the bounded actor queue.
    Enqueued,
    /// Queue had no immediate capacity and the route band should be treated as degraded.
    FullDegraded,
    /// Actor was closed and the route band should be treated as degraded.
    ClosedDegraded,
}

/// Durable write command with only derived/scrubbed fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DbWriteCommand {
    /// Persist a derived provider quota exhaustion observation after runtime quarantine.
    ProviderQuotaExhausted {
        account_id: AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    },
    /// Persist a previous-response affinity owner record.
    PreviousResponseAffinityOwner {
        owner: PreviousResponseAffinityOwnerRecord,
    },
    /// Persist the latest account selected for one Codex session.
    SessionAccountAffinity {
        route_band: RouteBand,
        affinity: SessionAccountAffinity,
    },
    /// Persist an active-client mirror acquisition.
    ActiveClientAcquired {
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        account_id: AccountId,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    },
    /// Persist an active-client mirror release.
    ActiveClientReleased {
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        released_unix_seconds: u64,
    },
}

impl DbWriteCommand {
    /// Creates a derived provider quota exhaustion command.
    #[must_use]
    pub const fn provider_quota_exhausted(
        account_id: AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    ) -> Self {
        Self::ProviderQuotaExhausted {
            account_id,
            route_band,
            classification,
            observed_unix_seconds,
        }
    }

    /// Creates a buffered previous-response affinity owner persistence command.
    #[must_use]
    pub const fn previous_response_affinity_owner(
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> Self {
        Self::PreviousResponseAffinityOwner { owner }
    }

    /// Creates a buffered Codex session account-affinity command.
    #[must_use]
    pub const fn session_account_affinity(
        route_band: RouteBand,
        affinity: SessionAccountAffinity,
    ) -> Self {
        Self::SessionAccountAffinity {
            route_band,
            affinity,
        }
    }

    /// Creates a best-effort active-client mirror acquisition command.
    #[must_use]
    pub fn active_client_acquired(
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        account_id: AccountId,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    ) -> Self {
        Self::ActiveClientAcquired {
            route_band,
            process_run_id,
            reservation_id,
            account_id,
            acquired_unix_seconds,
            active_pressure,
        }
    }

    /// Creates a best-effort active-client mirror release command.
    #[must_use]
    pub fn active_client_released(
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        released_unix_seconds: u64,
    ) -> Self {
        Self::ActiveClientReleased {
            route_band,
            process_run_id,
            reservation_id,
            released_unix_seconds,
        }
    }

    const fn routing_degraded_route_band(&self) -> Option<RouteBand> {
        match self {
            Self::ProviderQuotaExhausted { route_band, .. } => Some(*route_band),
            Self::PreviousResponseAffinityOwner { owner } => Some(owner.route_band()),
            Self::SessionAccountAffinity { .. } => None,
            Self::ActiveClientAcquired { .. } | Self::ActiveClientReleased { .. } => None,
        }
    }

    const fn route_band_label(&self) -> &'static str {
        match self {
            Self::ProviderQuotaExhausted { route_band, .. } => route_band.as_str(),
            Self::PreviousResponseAffinityOwner { owner } => owner.route_band().as_str(),
            Self::SessionAccountAffinity { route_band, .. } => route_band.as_str(),
            Self::ActiveClientAcquired { route_band, .. }
            | Self::ActiveClientReleased { route_band, .. } => route_band.as_str(),
        }
    }

    const fn observed_unix_seconds(&self) -> u64 {
        match self {
            Self::ProviderQuotaExhausted {
                observed_unix_seconds,
                ..
            } => *observed_unix_seconds,
            Self::PreviousResponseAffinityOwner { owner } => owner.created_unix_seconds(),
            Self::SessionAccountAffinity { affinity, .. } => affinity.last_seen_unix_seconds(),
            Self::ActiveClientAcquired {
                acquired_unix_seconds,
                ..
            } => *acquired_unix_seconds,
            Self::ActiveClientReleased {
                released_unix_seconds,
                ..
            } => *released_unix_seconds,
        }
    }

    const fn queue_name(&self) -> &'static str {
        match self {
            Self::ProviderQuotaExhausted { .. } => PROVIDER_EXHAUSTION_QUEUE_NAME,
            Self::PreviousResponseAffinityOwner { .. } => "affinity_owner",
            Self::SessionAccountAffinity { .. } => "affinity_owner",
            Self::ActiveClientAcquired { .. } | Self::ActiveClientReleased { .. } => {
                "active_client_mirror"
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QueuedDbWriteCommand {
    command: DbWriteCommand,
    enqueued_at: Instant,
}

/// Storage boundary used by the write actor.
pub trait DbWriteRepository: Send + Sync + 'static {
    /// Persists a provider quota exhaustion observation from derived fields.
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        account_id: AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>>;

    /// Persists an active-client mirror acquisition.
    fn record_active_client_acquired<'a>(
        &'a self,
        _route_band: RouteBand,
        _process_run_id: String,
        _reservation_id: ReservationId,
        _account_id: AccountId,
        _acquired_unix_seconds: u64,
        _active_pressure: u32,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async { Ok(()) })
    }

    /// Persists an active-client mirror release.
    fn record_active_client_released<'a>(
        &'a self,
        _route_band: RouteBand,
        _process_run_id: String,
        _reservation_id: ReservationId,
        _released_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async { Ok(()) })
    }

    /// Persists a previous-response affinity owner record.
    fn record_previous_response_affinity_owner<'a>(
        &'a self,
        _owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async { Ok(()) })
    }

    /// Persists a Codex session account affinity.
    fn record_session_account_affinity<'a>(
        &'a self,
        _affinity: SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Durable write repository failure.
#[derive(Debug, Error)]
pub enum DbWriteRepositoryError {
    /// SQLite or state repository failed.
    #[error("state store unavailable while recording DB write actor command")]
    State(#[from] StateStoreError),
}

/// SQLite-backed DB write repository.
#[derive(Clone, Debug)]
pub struct SqliteDbWriteRepository {
    state: AsyncSqliteStateStore,
}

impl SqliteDbWriteRepository {
    /// Creates a SQLite-backed DB write repository.
    #[must_use]
    pub const fn new(state: AsyncSqliteStateStore) -> Self {
        Self { state }
    }
}

impl DbWriteRepository for SqliteDbWriteRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        account_id: AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            if classification == ProviderErrorClassification::AccountQuotaExhausted {
                self.state
                    .mark_route_band_quota_exhausted(
                        &account_id,
                        route_band.as_str(),
                        observed_unix_seconds,
                    )
                    .await?;
            }
            Ok(())
        })
    }

    fn record_active_client_acquired<'a>(
        &'a self,
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        account_id: AccountId,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.state
                .record_active_client_acquired(
                    route_band.as_str(),
                    &process_run_id,
                    &reservation_id,
                    &account_id,
                    acquired_unix_seconds,
                    active_pressure,
                )
                .await?;
            Ok(())
        })
    }

    fn record_active_client_released<'a>(
        &'a self,
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        released_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.state
                .record_active_client_released(
                    route_band.as_str(),
                    &process_run_id,
                    &reservation_id,
                    released_unix_seconds,
                )
                .await?;
            Ok(())
        })
    }

    fn record_previous_response_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.state.write_previous_response_owner(&owner).await?;
            Ok(())
        })
    }

    fn record_session_account_affinity<'a>(
        &'a self,
        affinity: SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.state
                .upsert_session_account_affinity(&affinity)
                .await?;
            Ok(())
        })
    }
}

/// Bounded write-side actor handle.
#[derive(Clone, Debug)]
pub struct DbWriteActor {
    provider_exhaustion_sender: mpsc::Sender<QueuedDbWriteCommand>,
    affinity_owner_sender: mpsc::Sender<QueuedDbWriteCommand>,
    active_mirror_sender: mpsc::Sender<QueuedDbWriteCommand>,
    shutdown: CancellationToken,
    closed: Arc<AtomicBool>,
    route_band_queue_health: RouteBandQueueHealth,
    last_degraded_event: Arc<Mutex<Option<QueueDegradedEvent>>>,
    last_queue_lag_event: Arc<Mutex<Option<QueueLagEvent>>>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

struct DbWriteActorRuntime {
    repository: Arc<dyn DbWriteRepository>,
    provider_exhaustion_receiver: mpsc::Receiver<QueuedDbWriteCommand>,
    affinity_owner_receiver: mpsc::Receiver<QueuedDbWriteCommand>,
    active_mirror_receiver: mpsc::Receiver<QueuedDbWriteCommand>,
    shutdown: CancellationToken,
    closed: Arc<AtomicBool>,
    route_band_queue_health: RouteBandQueueHealth,
    low_water_depth: usize,
    last_queue_lag_event: Arc<Mutex<Option<QueueLagEvent>>>,
}

impl DbWriteActor {
    /// Starts a bounded write actor on the current Tokio runtime.
    #[must_use]
    pub fn start(repository: Arc<dyn DbWriteRepository>, capacity: usize) -> Self {
        Self::start_on_handle(
            &Handle::current(),
            repository,
            RouteBandQueueHealth::default(),
            capacity,
        )
    }

    /// Starts a bounded write actor on an explicit Tokio runtime handle.
    #[must_use]
    pub fn start_on_handle(
        runtime_handle: &Handle,
        repository: Arc<dyn DbWriteRepository>,
        route_band_queue_health: RouteBandQueueHealth,
        capacity: usize,
    ) -> Self {
        let (provider_exhaustion_sender, provider_exhaustion_receiver) = mpsc::channel(capacity);
        let (affinity_owner_sender, affinity_owner_receiver) = mpsc::channel(capacity);
        let (active_mirror_sender, active_mirror_receiver) = mpsc::channel(capacity);
        let shutdown = CancellationToken::new();
        let closed = Arc::new(AtomicBool::new(false));
        let task_shutdown = shutdown.clone();
        let task_closed = Arc::clone(&closed);
        let task_route_band_queue_health = Arc::clone(&route_band_queue_health);
        let last_queue_lag_event = Arc::new(Mutex::new(None));
        let task_last_queue_lag_event = Arc::clone(&last_queue_lag_event);
        let low_water_depth = capacity / 4;
        let task = runtime_handle.spawn(async move {
            run_db_write_actor(DbWriteActorRuntime {
                repository,
                provider_exhaustion_receiver,
                affinity_owner_receiver,
                active_mirror_receiver,
                shutdown: task_shutdown,
                closed: task_closed,
                route_band_queue_health: task_route_band_queue_health,
                low_water_depth,
                last_queue_lag_event: task_last_queue_lag_event,
            })
            .await;
        });
        Self {
            provider_exhaustion_sender,
            affinity_owner_sender,
            active_mirror_sender,
            shutdown,
            closed,
            route_band_queue_health,
            last_degraded_event: Arc::new(Mutex::new(None)),
            last_queue_lag_event,
            task: Arc::new(Mutex::new(Some(task))),
        }
    }

    /// Attempts to enqueue without awaiting queue capacity.
    #[must_use]
    pub fn try_enqueue(&self, command: DbWriteCommand) -> DbWriteEnqueueResult {
        if self.closed.load(Ordering::Acquire) {
            self.record_degraded_event(&command, QueueDegradedReason::Closed);
            return DbWriteEnqueueResult::ClosedDegraded;
        }
        let command_queue_name = command.queue_name();
        let command_route_band_label = command.route_band_label();
        let sender = self.sender_for_command(&command);
        let queued_command = QueuedDbWriteCommand {
            command,
            enqueued_at: Instant::now(),
        };
        match sender.try_send(queued_command) {
            Ok(()) => {
                record_db_write_queue_event(
                    command_queue_name,
                    command_route_band_label,
                    "enqueued",
                    "none",
                );
                self.record_queue_depth(command_queue_name, command_route_band_label);
                DbWriteEnqueueResult::Enqueued
            }
            Err(mpsc::error::TrySendError::Full(queued_command)) => {
                self.record_degraded_event(&queued_command.command, QueueDegradedReason::Full);
                DbWriteEnqueueResult::FullDegraded
            }
            Err(mpsc::error::TrySendError::Closed(queued_command)) => {
                self.closed.store(true, Ordering::Release);
                self.record_degraded_event(&queued_command.command, QueueDegradedReason::Closed);
                DbWriteEnqueueResult::ClosedDegraded
            }
        }
    }

    /// Returns the last scrubbed degraded queue event observed by this actor.
    #[must_use]
    pub fn last_degraded_event(&self) -> Option<QueueDegradedEvent> {
        match self.last_degraded_event.lock() {
            Ok(last_degraded_event) => last_degraded_event.clone(),
            Err(error) => {
                tracing::warn!(
                    error.class = "db_write_actor_degraded_event_lock_poisoned",
                    error.message = %error,
                    "codex_router.db_write_actor_degraded_event_unavailable"
                );
                None
            }
        }
    }

    /// Returns the last scrubbed queue-lag event observed by this actor.
    #[must_use]
    pub fn last_queue_lag_event(&self) -> Option<QueueLagEvent> {
        match self.last_queue_lag_event.lock() {
            Ok(last_queue_lag_event) => last_queue_lag_event.clone(),
            Err(error) => {
                tracing::warn!(
                    error.class = "db_write_actor_queue_lag_event_lock_poisoned",
                    error.message = %error,
                    "codex_router.db_write_actor_queue_lag_event_unavailable"
                );
                None
            }
        }
    }

    /// Cancels the actor and waits for the task to finish.
    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        self.shutdown.cancel();
        let task = match self.task.lock() {
            Ok(mut task) => task.take(),
            Err(error) => {
                tracing::warn!(
                    error.class = "db_write_actor_task_lock_poisoned",
                    error.message = %error,
                    "codex_router.db_write_actor_shutdown_task_unavailable"
                );
                None
            }
        };
        if let Some(mut task) = task {
            let drain_grace = tokio::time::sleep(std::time::Duration::from_millis(
                DB_WRITE_SHUTDOWN_DRAIN_GRACE_MS,
            ));
            tokio::pin!(drain_grace);
            tokio::select! {
                _join_result = &mut task => {}
                () = &mut drain_grace => {
                    task.abort();
                    let _join_result = task.await;
                }
            }
        }
    }

    fn record_queue_depth(&self, queue_name: &'static str, route_band_label: &'static str) {
        let sender = self.sender_for_queue_name(queue_name);
        let depth = sender.max_capacity().saturating_sub(sender.capacity());
        record_db_write_queue_depth(queue_name, route_band_label, depth as u64);
    }

    fn record_degraded_event(&self, command: &DbWriteCommand, reason: QueueDegradedReason) {
        let route_band_degraded_reason = match reason {
            QueueDegradedReason::Full => RouteBandQueueDegradedReason::DbWriteQueueFull,
            QueueDegradedReason::Closed => RouteBandQueueDegradedReason::DbWriteQueueClosed,
            QueueDegradedReason::WriteFailed => RouteBandQueueDegradedReason::DbWriteFailed,
        };
        if let Some(route_band) = command.routing_degraded_route_band() {
            let _mark_result = mark_route_band_queue_degraded_for_queue(
                &self.route_band_queue_health,
                route_band,
                command.queue_name(),
                route_band_degraded_reason,
                command.observed_unix_seconds(),
            );
        }
        let event = emit_db_write_queue_degraded(
            command.queue_name(),
            command.route_band_label(),
            reason,
            self.sender_for_command(command).max_capacity(),
        );
        match self.last_degraded_event.lock() {
            Ok(mut last_degraded_event) => {
                *last_degraded_event = Some(event);
            }
            Err(error) => {
                tracing::warn!(
                    error.class = "db_write_actor_degraded_event_lock_poisoned",
                    error.message = %error,
                    "codex_router.db_write_actor_degraded_event_dropped"
                );
            }
        }
    }

    fn sender_for_command(&self, command: &DbWriteCommand) -> &mpsc::Sender<QueuedDbWriteCommand> {
        match command {
            DbWriteCommand::ProviderQuotaExhausted { .. } => &self.provider_exhaustion_sender,
            DbWriteCommand::PreviousResponseAffinityOwner { .. }
            | DbWriteCommand::SessionAccountAffinity { .. } => &self.affinity_owner_sender,
            DbWriteCommand::ActiveClientAcquired { .. }
            | DbWriteCommand::ActiveClientReleased { .. } => &self.active_mirror_sender,
        }
    }

    fn sender_for_queue_name(&self, queue_name: &str) -> &mpsc::Sender<QueuedDbWriteCommand> {
        match queue_name {
            PROVIDER_EXHAUSTION_QUEUE_NAME => &self.provider_exhaustion_sender,
            "affinity_owner" => &self.affinity_owner_sender,
            _ => &self.active_mirror_sender,
        }
    }
}

async fn run_db_write_actor(mut runtime: DbWriteActorRuntime) {
    loop {
        tokio::select! {
            biased;
            command = runtime.provider_exhaustion_receiver.recv() => {
                if !handle_db_write_actor_command(
                    runtime.repository.as_ref(),
                    command,
                    &mut runtime.provider_exhaustion_receiver,
                    &runtime.route_band_queue_health,
                    runtime.low_water_depth,
                    &runtime.last_queue_lag_event,
                ).await {
                    break;
                }
            }
            command = runtime.affinity_owner_receiver.recv() => {
                if !handle_db_write_actor_command(
                    runtime.repository.as_ref(),
                    command,
                    &mut runtime.affinity_owner_receiver,
                    &runtime.route_band_queue_health,
                    runtime.low_water_depth,
                    &runtime.last_queue_lag_event,
                ).await {
                    break;
                }
            }
            command = runtime.active_mirror_receiver.recv() => {
                if !handle_db_write_actor_command(
                    runtime.repository.as_ref(),
                    command,
                    &mut runtime.active_mirror_receiver,
                    &runtime.route_band_queue_health,
                    runtime.low_water_depth,
                    &runtime.last_queue_lag_event,
                ).await {
                    break;
                }
            }
            () = runtime.shutdown.cancelled() => {
                runtime.provider_exhaustion_receiver.close();
                runtime.affinity_owner_receiver.close();
                runtime.active_mirror_receiver.close();
            }
        }
    }
    runtime.closed.store(true, Ordering::Release);
}

async fn handle_db_write_actor_command(
    repository: &dyn DbWriteRepository,
    queued_command: Option<QueuedDbWriteCommand>,
    receiver: &mut mpsc::Receiver<QueuedDbWriteCommand>,
    route_band_queue_health: &RouteBandQueueHealth,
    low_water_depth: usize,
    last_queue_lag_event: &Arc<Mutex<Option<QueueLagEvent>>>,
) -> bool {
    let Some(QueuedDbWriteCommand {
        command,
        enqueued_at,
    }) = queued_command
    else {
        return false;
    };
    let queue_lag_event = emit_db_write_queue_lag_observed(
        command.queue_name(),
        command.route_band_label(),
        "processing",
        queue_lag_millis_since(enqueued_at),
    );
    {
        match last_queue_lag_event.lock() {
            Ok(mut last_queue_lag_event) => {
                *last_queue_lag_event = Some(queue_lag_event);
            }
            Err(error) => {
                tracing::warn!(
                    error.class = "db_write_actor_queue_lag_event_lock_poisoned",
                    error.message = %error,
                    "codex_router.db_write_actor_queue_lag_event_dropped"
                );
            }
        }
    }
    let command_result = handle_db_write_command(repository, command).await;
    apply_db_write_command_result(
        command_result,
        route_band_queue_health,
        receiver.capacity(),
        receiver.max_capacity(),
        low_water_depth,
    );
    true
}

fn queue_lag_millis_since(enqueued_at: Instant) -> u64 {
    u64::try_from(enqueued_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn apply_db_write_command_result(
    command_result: DbWriteCommandResult,
    route_band_queue_health: &RouteBandQueueHealth,
    receiver_capacity: usize,
    receiver_max_capacity: usize,
    low_water_depth: usize,
) {
    match command_result {
        DbWriteCommandResult::Succeeded {
            route_band,
            queue_name,
        } => {
            let queued_depth = receiver_max_capacity.saturating_sub(receiver_capacity);
            if queued_depth <= low_water_depth
                && !closed_queue_degraded(route_band_queue_health, route_band)
            {
                let _clear_result = clear_route_band_queue_degraded_for_queue(
                    route_band_queue_health,
                    route_band,
                    queue_name,
                );
            }
        }
        DbWriteCommandResult::SucceededNoRoutingEffect => {}
        DbWriteCommandResult::Failed(command) => {
            if let Some(route_band) = command.routing_degraded_route_band() {
                let _mark_result = mark_route_band_queue_degraded_for_queue(
                    route_band_queue_health,
                    route_band,
                    command.queue_name(),
                    RouteBandQueueDegradedReason::DbWriteFailed,
                    command.observed_unix_seconds(),
                );
            }
            let _event = emit_db_write_queue_degraded(
                command.queue_name(),
                command.route_band_label(),
                QueueDegradedReason::WriteFailed,
                receiver_max_capacity,
            );
        }
    }
}

fn route_band_queue_degraded_reason_is(
    route_band_queue_health: &RouteBandQueueHealth,
    route_band: RouteBand,
    reason: RouteBandQueueDegradedReason,
) -> bool {
    let Ok(queue_health) = route_band_queue_health.lock() else {
        return true;
    };
    let prefix = route_band_queue_health_key_prefix(route_band);
    queue_health
        .iter()
        .filter(|(key, _state)| key.as_str() == route_band.as_str() || key.starts_with(&prefix))
        .any(|(_key, state)| state.reason() == reason)
}

async fn handle_db_write_command(
    repository: &dyn DbWriteRepository,
    command: DbWriteCommand,
) -> DbWriteCommandResult {
    match command {
        DbWriteCommand::ProviderQuotaExhausted {
            account_id,
            route_band,
            classification,
            observed_unix_seconds,
        } => {
            let result = repository
                .record_provider_quota_exhausted(
                    account_id.clone(),
                    route_band,
                    classification,
                    observed_unix_seconds,
                )
                .await;
            if result.is_ok() {
                return DbWriteCommandResult::Succeeded {
                    route_band,
                    queue_name: PROVIDER_EXHAUSTION_QUEUE_NAME,
                };
            }
            DbWriteCommandResult::Failed(DbWriteCommand::ProviderQuotaExhausted {
                account_id,
                route_band,
                classification,
                observed_unix_seconds,
            })
        }
        DbWriteCommand::PreviousResponseAffinityOwner { owner } => {
            let result = repository
                .record_previous_response_affinity_owner(owner.clone())
                .await;
            if result.is_ok() {
                return DbWriteCommandResult::Succeeded {
                    route_band: owner.route_band(),
                    queue_name: "affinity_owner",
                };
            }
            DbWriteCommandResult::Failed(DbWriteCommand::PreviousResponseAffinityOwner { owner })
        }
        DbWriteCommand::SessionAccountAffinity {
            route_band,
            affinity,
        } => {
            let result = repository
                .record_session_account_affinity(affinity.clone())
                .await;
            if result.is_ok() {
                return DbWriteCommandResult::SucceededNoRoutingEffect;
            }
            DbWriteCommandResult::Failed(DbWriteCommand::SessionAccountAffinity {
                route_band,
                affinity,
            })
        }
        DbWriteCommand::ActiveClientAcquired {
            route_band,
            process_run_id,
            reservation_id,
            account_id,
            acquired_unix_seconds,
            active_pressure,
        } => {
            let result = repository
                .record_active_client_acquired(
                    route_band,
                    process_run_id.clone(),
                    reservation_id.clone(),
                    account_id.clone(),
                    acquired_unix_seconds,
                    active_pressure,
                )
                .await;
            if result.is_ok() {
                return DbWriteCommandResult::SucceededNoRoutingEffect;
            }
            DbWriteCommandResult::Failed(DbWriteCommand::ActiveClientAcquired {
                route_band,
                process_run_id,
                reservation_id,
                account_id,
                acquired_unix_seconds,
                active_pressure,
            })
        }
        DbWriteCommand::ActiveClientReleased {
            route_band,
            process_run_id,
            reservation_id,
            released_unix_seconds,
        } => {
            let result = repository
                .record_active_client_released(
                    route_band,
                    process_run_id.clone(),
                    reservation_id.clone(),
                    released_unix_seconds,
                )
                .await;
            if result.is_ok() {
                return DbWriteCommandResult::SucceededNoRoutingEffect;
            }
            DbWriteCommandResult::Failed(DbWriteCommand::ActiveClientReleased {
                route_band,
                process_run_id,
                reservation_id,
                released_unix_seconds,
            })
        }
    }
}

enum DbWriteCommandResult {
    Succeeded {
        route_band: RouteBand,
        queue_name: &'static str,
    },
    SucceededNoRoutingEffect,
    Failed(DbWriteCommand),
}

fn closed_queue_degraded(
    route_band_queue_health: &RouteBandQueueHealth,
    route_band: RouteBand,
) -> bool {
    route_band_queue_degraded_reason_is(
        route_band_queue_health,
        route_band,
        RouteBandQueueDegradedReason::DbWriteQueueClosed,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use codex_router_core::ids::AccountId;
    use codex_router_core::ids::ReservationId;
    use codex_router_core::routes::RouteBand;
    use futures_util::future::BoxFuture;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use tokio::sync::Notify;

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
    use super::DbWriteRepository;
    use super::DbWriteRepositoryError;

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
        route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses)
            .unwrap_or_else(|error| {
                panic!(
                    "best-effort active mirror pressure must not degrade affinity routing: {error}"
                )
            });

        repository.release.notify_waiters();
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

        assert!(
            route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses).is_err()
        );

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
                if route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses)
                    .is_ok()
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

        route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses)
            .unwrap_or_else(|error| {
                panic!("queue health should recover once queued depth is below low-water: {error}")
            });
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
                if route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses)
                    .is_ok()
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

        route_band_queue_health_allows_selection(&queue_health, RouteBand::Responses)
            .unwrap_or_else(|error| {
                panic!("cache-affinity persistence failure must not block routing: {error}")
            });
        actor.shutdown().await;
    }

    #[derive(Default)]
    struct BlockingDbWriteRepository {
        entered: Notify,
        release: Notify,
        calls: AtomicUsize,
    }

    impl DbWriteRepository for BlockingDbWriteRepository {
        fn record_provider_quota_exhausted<'a>(
            &'a self,
            _account_id: AccountId,
            _route_band: RouteBand,
            _classification: ProviderErrorClassification,
            _observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            self.block_first_write()
        }

        fn record_active_client_acquired<'a>(
            &'a self,
            _route_band: RouteBand,
            _process_run_id: String,
            _reservation_id: ReservationId,
            _account_id: AccountId,
            _acquired_unix_seconds: u64,
            _active_pressure: u32,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            self.block_first_write()
        }

        fn record_active_client_released<'a>(
            &'a self,
            _route_band: RouteBand,
            _process_run_id: String,
            _reservation_id: ReservationId,
            _released_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            self.block_first_write()
        }

        fn record_previous_response_affinity_owner<'a>(
            &'a self,
            _owner: PreviousResponseAffinityOwnerRecord,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            self.block_first_write()
        }
    }

    impl BlockingDbWriteRepository {
        fn block_first_write<'a>(&'a self) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async move {
                self.entered.notify_waiters();
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    self.release.notified().await;
                }
                Ok(())
            })
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum RecordedDbWrite {
        ProviderQuotaExhausted {
            account_id: AccountId,
            route_band: RouteBand,
            classification: ProviderErrorClassification,
            observed_unix_seconds: u64,
        },
        PreviousResponseAffinityOwner(PreviousResponseAffinityOwnerRecord),
        ActiveClientAcquired {
            route_band: RouteBand,
            process_run_id: String,
            reservation_id: ReservationId,
            account_id: AccountId,
            acquired_unix_seconds: u64,
            active_pressure: u32,
        },
        ActiveClientReleased {
            route_band: RouteBand,
            process_run_id: String,
            reservation_id: ReservationId,
            released_unix_seconds: u64,
        },
    }

    #[derive(Default)]
    struct RecordingDbWriteRepository {
        records: Mutex<Vec<RecordedDbWrite>>,
    }

    impl RecordingDbWriteRepository {
        fn records(&self) -> Vec<RecordedDbWrite> {
            self.records
                .lock()
                .unwrap_or_else(|error| panic!("recording repository lock should hold: {error}"))
                .clone()
        }
    }

    impl DbWriteRepository for RecordingDbWriteRepository {
        fn record_provider_quota_exhausted<'a>(
            &'a self,
            account_id: AccountId,
            route_band: RouteBand,
            classification: ProviderErrorClassification,
            observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async move {
                self.records
                    .lock()
                    .unwrap_or_else(|error| {
                        panic!("recording repository lock should hold: {error}")
                    })
                    .push(RecordedDbWrite::ProviderQuotaExhausted {
                        account_id,
                        route_band,
                        classification,
                        observed_unix_seconds,
                    });
                Ok(())
            })
        }

        fn record_active_client_acquired<'a>(
            &'a self,
            route_band: RouteBand,
            process_run_id: String,
            reservation_id: ReservationId,
            account_id: AccountId,
            acquired_unix_seconds: u64,
            active_pressure: u32,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async move {
                self.records
                    .lock()
                    .unwrap_or_else(|error| {
                        panic!("recording repository lock should hold: {error}")
                    })
                    .push(RecordedDbWrite::ActiveClientAcquired {
                        route_band,
                        process_run_id,
                        reservation_id,
                        account_id,
                        acquired_unix_seconds,
                        active_pressure,
                    });
                Ok(())
            })
        }

        fn record_previous_response_affinity_owner<'a>(
            &'a self,
            owner: PreviousResponseAffinityOwnerRecord,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async move {
                self.records
                    .lock()
                    .unwrap_or_else(|error| {
                        panic!("recording repository lock should hold: {error}")
                    })
                    .push(RecordedDbWrite::PreviousResponseAffinityOwner(owner));
                Ok(())
            })
        }

        fn record_active_client_released<'a>(
            &'a self,
            route_band: RouteBand,
            process_run_id: String,
            reservation_id: ReservationId,
            released_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async move {
                self.records
                    .lock()
                    .unwrap_or_else(|error| {
                        panic!("recording repository lock should hold: {error}")
                    })
                    .push(RecordedDbWrite::ActiveClientReleased {
                        route_band,
                        process_run_id,
                        reservation_id,
                        released_unix_seconds,
                    });
                Ok(())
            })
        }
    }

    struct SlowRecordingDbWriteRepository {
        delay: std::time::Duration,
        records: Mutex<Vec<RecordedDbWrite>>,
    }

    impl SlowRecordingDbWriteRepository {
        fn new(delay: std::time::Duration) -> Self {
            Self {
                delay,
                records: Mutex::new(Vec::new()),
            }
        }

        fn records(&self) -> Vec<RecordedDbWrite> {
            self.records
                .lock()
                .unwrap_or_else(|error| {
                    panic!("slow recording repository lock should hold: {error}")
                })
                .clone()
        }
    }

    impl DbWriteRepository for SlowRecordingDbWriteRepository {
        fn record_provider_quota_exhausted<'a>(
            &'a self,
            account_id: AccountId,
            route_band: RouteBand,
            classification: ProviderErrorClassification,
            observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async move {
                tokio::time::sleep(self.delay).await;
                self.records
                    .lock()
                    .unwrap_or_else(|error| {
                        panic!("slow recording repository lock should hold: {error}")
                    })
                    .push(RecordedDbWrite::ProviderQuotaExhausted {
                        account_id,
                        route_band,
                        classification,
                        observed_unix_seconds,
                    });
                Ok(())
            })
        }
    }

    #[derive(Default)]
    struct FailingOnceDbWriteRepository {
        calls: AtomicUsize,
    }

    impl DbWriteRepository for FailingOnceDbWriteRepository {
        fn record_provider_quota_exhausted<'a>(
            &'a self,
            _account_id: AccountId,
            _route_band: RouteBand,
            _classification: ProviderErrorClassification,
            _observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async move {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(DbWriteRepositoryError::State(
                        codex_router_state::sqlite::StateStoreError::Sqlite {
                            message: "injected write failure".to_owned(),
                        },
                    ));
                }
                Ok(())
            })
        }
    }

    struct FailingWriteRepository;

    impl DbWriteRepository for FailingWriteRepository {
        fn record_provider_quota_exhausted<'a>(
            &'a self,
            _account_id: AccountId,
            _route_band: RouteBand,
            _classification: ProviderErrorClassification,
            _observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async move {
                Err(DbWriteRepositoryError::State(
                    codex_router_state::sqlite::StateStoreError::Sqlite {
                        message: "injected write failure before health probe".to_owned(),
                    },
                ))
            })
        }
    }

    #[derive(Default)]
    struct FailingSessionAffinityRepository {
        write_attempted: Notify,
    }

    impl DbWriteRepository for FailingSessionAffinityRepository {
        fn record_provider_quota_exhausted<'a>(
            &'a self,
            _account_id: AccountId,
            _route_band: RouteBand,
            _classification: ProviderErrorClassification,
            _observed_unix_seconds: u64,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async { Ok(()) })
        }

        fn record_session_account_affinity<'a>(
            &'a self,
            _affinity: SessionAccountAffinity,
        ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
            Box::pin(async move {
                self.write_attempted.notify_one();
                Err(DbWriteRepositoryError::State(
                    codex_router_state::sqlite::StateStoreError::Sqlite {
                        message: "injected session affinity write failure".to_owned(),
                    },
                ))
            })
        }
    }

    fn account_id(value: &str) -> AccountId {
        AccountId::new(value)
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"))
    }
}
