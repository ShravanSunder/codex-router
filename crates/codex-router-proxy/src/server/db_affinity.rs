use super::*;

#[derive(Clone, Debug)]
pub(super) struct DbWriteAffinityOwnerRecorder {
    pub(super) db_write_actor: DbWriteActor,
}

impl DbWriteAffinityOwnerRecorder {
    pub(super) const fn new(db_write_actor: DbWriteActor) -> Self {
        Self { db_write_actor }
    }
}

impl AsyncHttpAffinityOwnerRecorder for DbWriteAffinityOwnerRecorder {
    fn record_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), HttpProxyError>> {
        Box::pin(async move {
            match self
                .db_write_actor
                .try_enqueue(DbWriteCommand::previous_response_affinity_owner(owner))
            {
                DbWriteEnqueueResult::Enqueued => Ok(()),
                DbWriteEnqueueResult::FullDegraded | DbWriteEnqueueResult::ClosedDegraded => {
                    Err(HttpProxyError::Selection {
                        reason:
                            crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
                    })
                }
            }
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct AsyncSqliteProviderErrorObserver {
    pub(super) writable_state_store: AsyncSqliteStateStore,
    pub(super) selection_state_store: AsyncSqliteStateStore,
    pub(super) active_reservations: RouteBandReservationBooks,
    pub(super) runtime_exhaustions: RouteBandRuntimeExhaustions,
    pub(super) route_band_queue_health: RouteBandQueueHealth,
    pub(super) db_write_actor: DbWriteActor,
}

impl AsyncSqliteProviderErrorObserver {
    pub(super) fn new(
        writable_state_store: AsyncSqliteStateStore,
        selection_state_store: AsyncSqliteStateStore,
        active_reservations: RouteBandReservationBooks,
        runtime_exhaustions: RouteBandRuntimeExhaustions,
        route_band_queue_health: RouteBandQueueHealth,
        db_write_actor: DbWriteActor,
    ) -> Self {
        Self {
            writable_state_store,
            selection_state_store,
            active_reservations,
            runtime_exhaustions,
            route_band_queue_health,
            db_write_actor,
        }
    }
}

impl AsyncProviderErrorObserver for AsyncSqliteProviderErrorObserver {
    fn mark_runtime_account_quota_exhausted(
        &self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        observed_unix_seconds: u64,
    ) -> Result<(), ProviderErrorObservationError> {
        mark_runtime_quota_exhausted(
            &self.runtime_exhaustions,
            route_band,
            account_id,
            observed_unix_seconds,
        )
        .map_err(ProviderErrorObservationError::from)
    }

    fn observe_provider_error<'a>(
        &'a self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
        Box::pin(async move {
            record_provider_error_observation(
                &self.writable_state_store,
                &account_id,
                route_band.as_str(),
                classification,
                observed_unix_seconds,
            )
            .await
            .map(|_classification| ())
            .map_err(ProviderErrorObservationError::from)
        })
    }

    fn enqueue_provider_quota_exhaustion(
        &self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    ) -> DbWriteEnqueueResult {
        self.db_write_actor
            .try_enqueue(DbWriteCommand::provider_quota_exhausted(
                account_id,
                route_band,
                classification,
                observed_unix_seconds,
            ))
    }

    fn route_band_post_exhaustion_outcome<'a>(
        &'a self,
        exhausted_account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        route_profile: RouteProfile,
        observed_unix_seconds: u64,
    ) -> BoxFuture<
        'a,
        Result<
            crate::account_selection::PostExhaustionRouteBandOutcome,
            ProviderErrorObservationError,
        >,
    > {
        Box::pin(async move {
            route_band_post_exhaustion_outcome(RouteBandPostExhaustionOutcomeInput {
                state_repository: &self.selection_state_store,
                active_reservations: Some(&self.active_reservations),
                runtime_exhaustions: Some(&self.runtime_exhaustions),
                route_band_queue_health: Some(&self.route_band_queue_health),
                route_band,
                route_profile,
                excluded_account_id: &exhausted_account_id,
                now_unix_seconds: observed_unix_seconds,
            })
            .await
            .map_err(ProviderErrorObservationError::from)
        })
    }
}
