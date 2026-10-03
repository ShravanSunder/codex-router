//! SQLite repository contracts responsibilities.
use super::*;
/// Async quota history repository contract for Tokio runtime callers.
pub trait AsyncQuotaHistoryRepository {
    /// Appends one quota history observation.
    fn append_quota_history_observation<'a>(
        &'a self,
        observation: &'a PersistedQuotaHistoryObservation,
    ) -> BoxFuture<'a, Result<(), StateStoreError>>;

    /// Loads quota history observations for one account/route/window/time range.
    fn quota_history_observations_for_window<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        limit_window_seconds: u64,
        observed_from_unix_seconds: u64,
        observed_to_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<PersistedQuotaHistoryObservation>, StateStoreError>>;

    /// Purges quota history older than the given observation timestamp.
    fn purge_quota_history_before(
        &self,
        observed_before_unix_seconds: u64,
    ) -> BoxFuture<'_, Result<(), StateStoreError>>;
}

impl AsyncQuotaHistoryRepository for AsyncSqliteStateStore {
    fn append_quota_history_observation<'a>(
        &'a self,
        observation: &'a PersistedQuotaHistoryObservation,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async move { self.append_quota_history_observation(observation).await })
    }

    fn quota_history_observations_for_window<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        limit_window_seconds: u64,
        observed_from_unix_seconds: u64,
        observed_to_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<PersistedQuotaHistoryObservation>, StateStoreError>> {
        Box::pin(async move {
            self.quota_history_observations_for_window(
                account_id,
                route_band,
                limit_window_seconds,
                observed_from_unix_seconds,
                observed_to_unix_seconds,
            )
            .await
        })
    }

    fn purge_quota_history_before(
        &self,
        observed_before_unix_seconds: u64,
    ) -> BoxFuture<'_, Result<(), StateStoreError>> {
        Box::pin(async move {
            self.purge_quota_history_before(observed_before_unix_seconds)
                .await
        })
    }
}

/// Async selector quota repository contract for Tokio runtime callers.
pub trait AsyncSelectorQuotaRepository {
    /// Loads selector input rows for one route band.
    fn selector_inputs_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<SelectorQuotaInput>, StateStoreError>>;
}

impl AsyncSelectorQuotaRepository for AsyncSqliteStateStore {
    fn selector_inputs_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<SelectorQuotaInput>, StateStoreError>> {
        Box::pin(async move {
            self.selector_inputs_for_route_band(route_band, now_unix_seconds)
                .await
        })
    }
}

/// Async quota exhaustion writer for Tokio runtime callers.
pub trait AsyncQuotaExhaustionRepository {
    /// Marks an account exhausted for one route band.
    fn mark_route_band_quota_exhausted<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), StateStoreError>>;
}

impl AsyncQuotaExhaustionRepository for AsyncSqliteStateStore {
    fn mark_route_band_quota_exhausted<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async move {
            self.mark_route_band_quota_exhausted(account_id, route_band, observed_unix_seconds)
                .await
        })
    }
}

/// Async provider session account-affinity repository.
pub trait AsyncSessionAccountAffinityRepository {
    /// Inserts or replaces one session account affinity.
    fn upsert_session_account_affinity<'a>(
        &'a self,
        affinity: &'a SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), StateStoreError>>;

    /// Compare-and-sets one pin if its observed account, version, and TTL are current.
    fn compare_and_set_session_account_affinity<'a>(
        &'a self,
        observation: &'a PinObservation,
        affinity: &'a SessionAccountAffinity,
        pin_ttl_seconds: u64,
    ) -> BoxFuture<'a, Result<bool, StateStoreError>>;

    /// Loads one session account affinity.
    fn load_session_account_affinity<'a>(
        &'a self,
        provider: Provider,
        session_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<SessionAccountAffinity>, StateStoreError>>;
}

impl AsyncSessionAccountAffinityRepository for AsyncSqliteStateStore {
    fn upsert_session_account_affinity<'a>(
        &'a self,
        affinity: &'a SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async move { self.upsert_session_account_affinity(affinity).await })
    }

    fn compare_and_set_session_account_affinity<'a>(
        &'a self,
        observation: &'a PinObservation,
        affinity: &'a SessionAccountAffinity,
        pin_ttl_seconds: u64,
    ) -> BoxFuture<'a, Result<bool, StateStoreError>> {
        Box::pin(async move {
            self.compare_and_set_session_account_affinity(observation, affinity, pin_ttl_seconds)
                .await
        })
    }

    fn load_session_account_affinity<'a>(
        &'a self,
        provider: Provider,
        session_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<SessionAccountAffinity>, StateStoreError>> {
        Box::pin(async move {
            self.load_session_account_affinity(provider, session_id)
                .await
        })
    }
}

/// Async affinity repository contract for Tokio runtime callers.
pub trait AsyncAffinityRepository {
    /// Writes a hashed previous-response owner record.
    fn write_previous_response_owner<'a>(
        &'a self,
        owner: &'a PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), StateStoreError>>;

    /// Loads a hashed previous-response owner record for one route band.
    fn load_previous_response_owner<'a>(
        &'a self,
        affinity_key_hash: &'a AffinityKeyHash,
        route_band: &'a str,
    ) -> BoxFuture<'a, Result<PreviousResponseAffinityOwnerLookup, StateStoreError>>;
}

impl AsyncAffinityRepository for AsyncSqliteStateStore {
    fn write_previous_response_owner<'a>(
        &'a self,
        owner: &'a PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async move { self.write_previous_response_owner(owner).await })
    }

    fn load_previous_response_owner<'a>(
        &'a self,
        affinity_key_hash: &'a AffinityKeyHash,
        route_band: &'a str,
    ) -> BoxFuture<'a, Result<PreviousResponseAffinityOwnerLookup, StateStoreError>> {
        Box::pin(async move {
            self.load_previous_response_owner(affinity_key_hash, route_band)
                .await
        })
    }
}
