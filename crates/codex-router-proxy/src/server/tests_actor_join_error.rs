use super::*;
use crate::db_write_actor::{DbWriteRepository, DbWriteRepositoryError};
use codex_router_core::ids::ReservationId;
use codex_router_state::session_account_affinity::SessionAccountAffinity;

struct ErrorThenHeldSessionRepository {
    inner: SqliteDbWriteRepository,
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    release: Arc<tokio::sync::Semaphore>,
}
impl DbWriteRepository for ErrorThenHeldSessionRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        account: AccountId,
        band: RouteBand,
        classification: ProviderErrorClassification,
        now: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        let _original_input = (account, band, classification, now);
        Box::pin(async { panic!("controlled actual main actor failure") })
    }
    fn record_active_client_acquired<'a>(
        &'a self,
        band: RouteBand,
        process: String,
        reservation: ReservationId,
        account: AccountId,
        now: u64,
        pressure: u32,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.inner
            .record_active_client_acquired(band, process, reservation, account, now, pressure)
    }
    fn record_active_client_released<'a>(
        &'a self,
        band: RouteBand,
        process: String,
        reservation: ReservationId,
        now: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.inner
            .record_active_client_released(band, process, reservation, now)
    }
    fn record_session_account_affinity<'a>(
        &'a self,
        affinity: SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.entered
                .send(())
                .expect("real later actor write entered");
            self.release
                .acquire()
                .await
                .expect("later actor released")
                .forget();
            self.inner.record_session_account_affinity(affinity).await
        })
    }
    fn record_previous_response_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.inner.record_previous_response_affinity_owner(owner)
    }
}

#[tokio::test]
async fn earlier_actual_actor_error_survives_cancelled_later_join_and_all_handles_complete() {
    let (mut runtime, account, path, _secrets) =
        proxy_refresh_fixture("retained_actor_error", Duration::from_secs(1)).await;
    runtime.db_write_actor.shutdown().await;
    let state = AsyncSqliteStateStore::open(&path)
        .await
        .expect("actual durable writer");
    let (entered, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let actor = DbWriteActor::start_on_handle(
        &tokio::runtime::Handle::current(),
        Arc::new(ErrorThenHeldSessionRepository {
            inner: SqliteDbWriteRepository::new(state.clone()),
            entered,
            release: release.clone(),
        }),
        runtime.route_band_queue_health.clone(),
        PROVIDER_EXHAUSTION_QUEUE_CAPACITY,
    );
    runtime.db_write_actor = actor;
    assert_eq!(
        runtime
            .db_write_actor
            .try_enqueue(DbWriteCommand::session_account_affinity(
                RouteBand::Responses,
                SessionAccountAffinity::new(
                    codex_router_core::provider::Provider::Openai,
                    "retained-error-session",
                    account.clone(),
                    1_000
                )
            )),
        DbWriteEnqueueResult::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(2), observed.recv())
        .await
        .expect("actual session write enters")
        .expect("registered write entry");
    assert_eq!(
        runtime
            .db_write_actor
            .try_enqueue(DbWriteCommand::provider_quota_exhausted(
                account.clone(),
                RouteBand::Responses,
                ProviderErrorClassification::AccountQuotaExhausted,
                1_000
            )),
        DbWriteEnqueueResult::Enqueued
    );
    let mut stopped = runtime
        .stop_owned_protocol_connections_until_cancelled(0, CancellationToken::new())
        .await;
    stopped.drain_responses(&runtime).await;
    let pending = tokio::time::timeout(
        Duration::from_millis(20),
        stopped.drain_actor_work(&runtime),
    )
    .await
    .is_err();
    let retained = matches!(stopped.actor_join_error.as_ref(),Some(LoopbackRouterRuntimeError::ActorJoin {actor:LoopbackActorTask::DatabaseWrites,source}) if source.is_panic());
    let error_not_reported_early = stopped.take_actor_join_error().is_none();
    let maintenance_stopped = runtime.maintenance_actor.try_enqueue(
        crate::maintenance_actor::MaintenanceHint::CleanupStaleSessionAccountAffinities {
            stale_before_unix_seconds: 0,
        },
    );
    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), stopped.drain_actor_work(&runtime))
        .await
        .expect("same remaining handles join");
    let row = state
        .load_session_account_affinity(
            codex_router_core::provider::Provider::Openai,
            "retained-error-session",
        )
        .await
        .expect("actual session row")
        .expect("accepted write completed");
    let failure = stopped
        .take_actor_join_error()
        .expect("actual original join failure retained");
    let original = stopped
        .take_serving_result()
        .expect("completed outcome available");
    state.close().await.expect("fixture state closes");
    eprintln!(
        "actor_error pending={pending} retained_before_later_wait={retained} actor_failure={failure:?} durable_session_account={:?}",
        row.account_id()
    );
    assert!(pending);
    assert!(retained);
    assert!(error_not_reported_early);
    assert_eq!(
        maintenance_stopped,
        crate::maintenance_actor::MaintenanceEnqueueResult::ClosedDegraded
    );
    assert!(
        matches!(failure,LoopbackRouterRuntimeError::ActorJoin {actor:LoopbackActorTask::DatabaseWrites,source} if source.is_panic())
    );
    assert_eq!(row.account_id(), Some(&account));
    assert_eq!(
        original.expect("serving result remains distinct from actor error"),
        0
    );
}
