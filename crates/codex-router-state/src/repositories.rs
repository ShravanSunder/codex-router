//! State repository contracts used by proxy and selection.

use codex_router_core::affinity::AffinityKeyHash;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::AffinityKey;

use crate::account::AccountRecord;
use crate::affinity_owner::PreviousResponseAffinityOwnerLookup;
use crate::affinity_owner::PreviousResponseAffinityOwnerRecord;
use crate::quota_snapshot::PersistedQuotaSnapshot;
use crate::quota_snapshot::PersistedSelectorQuotaWindow;
use crate::quota_snapshot::QuotaRefreshErrorClass;
use crate::quota_snapshot::QuotaRefreshStatusView;
use crate::quota_snapshot::SelectorQuotaInput;
use crate::sqlite::StateStoreError;

/// Account metadata repository.
pub trait AccountStateRepository {
    /// Inserts or updates account metadata.
    fn upsert_account(&self, account: &AccountRecord) -> Result<(), StateStoreError>;

    /// Loads account metadata.
    fn load_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountRecord>, StateStoreError>;

    /// Lists account metadata in deterministic selector order.
    fn list_accounts(&self) -> Result<Vec<AccountRecord>, StateStoreError>;
}

/// Quota snapshot repository.
pub trait QuotaSnapshotRepository {
    /// Inserts or updates a quota snapshot.
    fn upsert_snapshot(&self, snapshot: &PersistedQuotaSnapshot) -> Result<(), StateStoreError>;

    /// Loads a quota snapshot.
    fn load_snapshot(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<PersistedQuotaSnapshot>, StateStoreError>;

    /// Loads a quota snapshot for one account and route band.
    fn load_snapshot_for_route_band(
        &self,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Option<PersistedQuotaSnapshot>, StateStoreError>;
}

/// Selector quota input repository.
pub trait SelectorQuotaRepository {
    /// Inserts or updates one selector quota window.
    fn upsert_selector_window(
        &self,
        window: &PersistedSelectorQuotaWindow,
    ) -> Result<(), StateStoreError>;

    /// Atomically records refresh success and replaces account route-band windows.
    fn record_refresh_success_and_replace_selector_windows(
        &self,
        account_id: &AccountId,
        route_band: &str,
        windows: &[PersistedSelectorQuotaWindow],
        last_success_unix_seconds: u64,
        stale_after_unix_seconds: u64,
    ) -> Result<(), StateStoreError>;

    /// Atomically records refresh failure while preserving selector windows.
    fn record_refresh_failure_preserving_selector_windows(
        &self,
        account_id: &AccountId,
        route_band: &str,
        last_attempt_unix_seconds: u64,
        last_error_class: QuotaRefreshErrorClass,
    ) -> Result<(), StateStoreError>;

    /// Loads selector input rows for one route band.
    fn selector_inputs_for_route_band(
        &self,
        route_band: &str,
        now_unix_seconds: u64,
    ) -> Result<Vec<SelectorQuotaInput>, StateStoreError>;

    /// Loads refresh status view rows for one route band.
    fn quota_refresh_statuses_for_route_band(
        &self,
        route_band: &str,
    ) -> Result<Vec<QuotaRefreshStatusView>, StateStoreError>;
}

/// Previous-response affinity repository.
pub trait AffinityRepository {
    /// Pins an affinity key to an account.
    fn pin_account(
        &self,
        affinity_key: &AffinityKey,
        account_id: &AccountId,
    ) -> Result<(), StateStoreError>;

    /// Loads an affinity pin.
    fn load_pin(&self, affinity_key: &AffinityKey) -> Result<Option<AccountId>, StateStoreError>;

    /// Writes a hashed previous-response owner record.
    fn write_previous_response_owner(
        &self,
        owner: &PreviousResponseAffinityOwnerRecord,
    ) -> Result<(), StateStoreError>;

    /// Loads a hashed previous-response owner record for one route band.
    fn load_previous_response_owner(
        &self,
        affinity_key_hash: &AffinityKeyHash,
        route_band: &str,
    ) -> Result<PreviousResponseAffinityOwnerLookup, StateStoreError>;

    /// Purges all hashed previous-response owner rows.
    fn purge_previous_response_owners(&self) -> Result<(), StateStoreError>;
}
