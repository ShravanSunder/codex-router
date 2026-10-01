//! Per-account credit policy, provider observations and ordered refresh commits.

use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_core::ids::AccountId;

use crate::quota_snapshot::PersistedQuotaHistoryObservation;
use crate::quota_snapshot::PersistedQuotaSnapshot;
use crate::quota_snapshot::PersistedSelectorQuotaWindow;
use crate::sqlite::StateStoreError;

/// One per-account refresh attempt allocated before provider IO begins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreditRefreshAttempt {
    account_id: AccountId,
    credential_generation: u64,
    sequence: u64,
}

impl CreditRefreshAttempt {
    /// Returns the account whose provider request this token orders.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the credential generation used by the provider request.
    #[must_use]
    pub const fn credential_generation(&self) -> u64 {
        self.credential_generation
    }

    /// Returns this account's monotonically allocated attempt sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// One validated candidate for the atomic Responses quota-and-credit commit.
pub struct ResponsesRefreshSuccessCommit<'a> {
    /// Attempt token that must still be the account's latest started request.
    pub attempt: &'a CreditRefreshAttempt,
    /// Selector windows returned by the same Responses observation.
    pub selector_windows: &'a [PersistedSelectorQuotaWindow],
    /// Provider response observation time.
    pub observed_unix_seconds: u64,
    /// Last timestamp at which this observation remains current.
    pub stale_after_unix_seconds: u64,
    /// Credit and spend-control facts from the same provider response.
    pub provider_observation: &'a CreditProviderObservation,
    /// History rows paired with the provider response and selector windows.
    pub history_observations: &'a [PersistedQuotaHistoryObservation],
    /// Scalar snapshot paired with the provider response and selector windows.
    pub snapshot: &'a PersistedQuotaSnapshot,
}

/// Stored credit facts plus the generation and attempt that established them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreditUsageObservation {
    account_id: AccountId,
    credential_generation: u64,
    latest_started_attempt: u64,
    committed_attempt: Option<u64>,
    observed_unix_seconds: Option<u64>,
    stale_after_unix_seconds: Option<u64>,
    provider_observation: CreditProviderObservation,
}

impl CreditUsageObservation {
    /// Returns the account owning this provider observation.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the credential generation tied to these facts.
    #[must_use]
    pub const fn credential_generation(&self) -> u64 {
        self.credential_generation
    }

    /// Returns the latest attempt sequence started for the account.
    #[must_use]
    pub const fn latest_started_attempt(&self) -> u64 {
        self.latest_started_attempt
    }

    /// Returns the successful attempt sequence currently stored, if any.
    #[must_use]
    pub const fn committed_attempt(&self) -> Option<u64> {
        self.committed_attempt
    }

    /// Returns the original timestamp of these provider facts, if a response committed.
    #[must_use]
    pub const fn observed_unix_seconds(&self) -> Option<u64> {
        self.observed_unix_seconds
    }

    /// Returns the stored provider facts.
    #[must_use]
    pub const fn provider_observation(&self) -> &CreditProviderObservation {
        &self.provider_observation
    }

    /// Returns whether these facts are current, fresh and provider-authorized for spending.
    #[must_use]
    pub fn authorizes_credit_usage(
        &self,
        active_credential_generation: Option<u64>,
        now_unix_seconds: u64,
    ) -> bool {
        active_credential_generation == Some(self.credential_generation)
            && self.committed_attempt == Some(self.latest_started_attempt)
            && self
                .observed_unix_seconds
                .is_some_and(|observed| observed <= now_unix_seconds)
            && self
                .stale_after_unix_seconds
                .is_some_and(|stale_after| now_unix_seconds < stale_after)
            && self.provider_observation.authorizes_credit_usage()
    }
}

fn corrupt_credit_account(account_id: &AccountId, field: &'static str) -> StateStoreError {
    StateStoreError::CorruptAccount {
        account_id: account_id.as_str().to_owned(),
        field,
    }
}

fn stored_nonnegative_u64(
    value: i64,
    account_id: &AccountId,
    field: &'static str,
) -> Result<u64, StateStoreError> {
    u64::try_from(value).map_err(|_| corrupt_credit_account(account_id, field))
}

fn optional_stored_nonnegative_u64(
    value: Option<i64>,
    account_id: &AccountId,
    field: &'static str,
) -> Result<Option<u64>, StateStoreError> {
    value
        .map(|value| stored_nonnegative_u64(value, account_id, field))
        .transpose()
}

mod observation_store;
mod policy_mutation_store;
mod policy_store;
mod refresh_transaction;

#[cfg(test)]
mod tests;

pub(crate) use observation_store::load_credit_usage_for_selector;
pub use policy_mutation_store::AsyncCreditUsagePolicyMutationStore;
pub(crate) use refresh_transaction::invalidate_credit_observation_after_credential_mutation;
