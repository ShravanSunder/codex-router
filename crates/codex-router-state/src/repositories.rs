//! State repository contracts used by proxy and selection.

use codex_router_core::ids::AccountId;
use codex_router_core::ids::AffinityKey;

use crate::account::AccountCredentialMetadata;
use crate::account::AccountRecord;
use crate::quota_snapshot::PersistedQuotaSnapshot;
use crate::quota_snapshot::PersistedQuotaStatusRow;
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

/// Non-secret account credential metadata repository.
pub trait AccountCredentialRepository {
    /// Inserts or updates account credential metadata.
    fn upsert_credential_metadata(
        &self,
        metadata: &AccountCredentialMetadata,
    ) -> Result<(), StateStoreError>;

    /// Loads account credential metadata.
    fn load_credential_metadata(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountCredentialMetadata>, StateStoreError>;
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

/// Quota status repository for local-only human output.
pub trait QuotaStatusRepository {
    /// Inserts or updates one quota status row.
    fn upsert_status_row(&self, row: &PersistedQuotaStatusRow) -> Result<(), StateStoreError>;

    /// Lists quota status rows in deterministic display order.
    fn list_status_rows(&self) -> Result<Vec<PersistedQuotaStatusRow>, StateStoreError>;

    /// Replaces one account/route-band selector snapshot and detailed status rows atomically.
    fn replace_route_quota_state(
        &self,
        snapshot: &PersistedQuotaSnapshot,
        status_rows: &[PersistedQuotaStatusRow],
    ) -> Result<(), StateStoreError>;
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
