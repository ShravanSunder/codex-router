//! State repository contracts used by proxy and selection.

use codex_router_core::ids::AccountId;
use codex_router_core::ids::AffinityKey;

use crate::account::AccountRecord;
use crate::quota_snapshot::PersistedQuotaSnapshot;
use crate::quota_snapshot::PersistedSelectorQuotaWindow;
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

    /// Loads selector input rows for one route band.
    fn selector_inputs_for_route_band(
        &self,
        route_band: &str,
    ) -> Result<Vec<SelectorQuotaInput>, StateStoreError>;
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
}
