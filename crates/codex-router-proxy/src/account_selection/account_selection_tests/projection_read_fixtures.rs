use super::*;

pub(super) struct PanicSelectionProjectionRepository;

impl codex_router_state::selection_projection::AsyncSelectionProjectionRepository
    for PanicSelectionProjectionRepository
{
    fn list_account_routing_policies(
        &self,
    ) -> futures_util::future::BoxFuture<
        '_,
        Result<
            Vec<codex_router_state::account_routing_policy::AccountRoutingPolicy>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { panic!("degraded queue health should prevent policy reads") })
    }

    fn selector_inputs_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::quota_snapshot::SelectorQuotaInput>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { panic!("degraded queue health should prevent selector input reads") })
    }

    fn active_client_counts_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::sqlite::ActiveClientCount>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { panic!("degraded queue health should prevent active count reads") })
    }

    fn active_client_counts_for_route_band_read_only<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::sqlite::ActiveClientCount>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { panic!("degraded queue health should prevent active count reads") })
    }

    fn quota_history_observations_for_window<'a>(
        &'a self,
        _account_id: &'a AccountId,
        _route_band: &'a str,
        _limit_window_seconds: u64,
        _observed_from_unix_seconds: u64,
        _observed_to_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::quota_snapshot::PersistedQuotaHistoryObservation>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { panic!("degraded queue health should prevent quota history reads") })
    }

    fn active_session_rollups_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::sqlite::ActiveSessionRollup>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { panic!("degraded queue health should prevent rollup reads") })
    }

    fn refresh_active_session_rollups_for_interval<'a>(
        &'a self,
        _route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
        _bucket_seconds: u64,
    ) -> futures_util::future::BoxFuture<'a, Result<(), codex_router_state::sqlite::StateStoreError>>
    {
        Box::pin(async { panic!("read-only alternative selection must not refresh rollups") })
    }
}

#[derive(Clone)]
pub(super) struct StaticSelectionProjectionRepository {
    inputs: Vec<codex_router_state::quota_snapshot::SelectorQuotaInput>,
}

impl StaticSelectionProjectionRepository {
    pub(super) fn new(inputs: Vec<codex_router_state::quota_snapshot::SelectorQuotaInput>) -> Self {
        Self { inputs }
    }
}

impl codex_router_state::selection_projection::AsyncSelectionProjectionRepository
    for StaticSelectionProjectionRepository
{
    fn list_account_routing_policies(
        &self,
    ) -> futures_util::future::BoxFuture<
        '_,
        Result<
            Vec<codex_router_state::account_routing_policy::AccountRoutingPolicy>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn selector_inputs_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::quota_snapshot::SelectorQuotaInput>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async move { Ok(self.inputs.clone()) })
    }

    fn active_client_counts_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::sqlite::ActiveClientCount>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn active_client_counts_for_route_band_read_only<'a>(
        &'a self,
        _route_band: &'a str,
        _now_unix_seconds: u64,
        _max_age_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::sqlite::ActiveClientCount>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn quota_history_observations_for_window<'a>(
        &'a self,
        _account_id: &'a AccountId,
        _route_band: &'a str,
        _limit_window_seconds: u64,
        _observed_from_unix_seconds: u64,
        _observed_to_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::quota_snapshot::PersistedQuotaHistoryObservation>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn active_session_rollups_for_route_band<'a>(
        &'a self,
        _route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<
            Vec<codex_router_state::sqlite::ActiveSessionRollup>,
            codex_router_state::sqlite::StateStoreError,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn refresh_active_session_rollups_for_interval<'a>(
        &'a self,
        _route_band: &'a str,
        _interval_start_unix_seconds: u64,
        _interval_end_unix_seconds: u64,
        _bucket_seconds: u64,
    ) -> futures_util::future::BoxFuture<'a, Result<(), codex_router_state::sqlite::StateStoreError>>
    {
        Box::pin(async { panic!("read-only alternative selection must not refresh rollups") })
    }
}
