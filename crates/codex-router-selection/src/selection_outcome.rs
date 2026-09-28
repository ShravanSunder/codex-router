//! Typed selection outcomes and Claude unavailable-reason precedence.

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;

use crate::burn_down::BurnDownRouteBandAssessmentResult;
use crate::burn_down::RoutingReason;
use crate::burn_down::SelectedPool;

/// Unix timestamp at which every rejected quota window is expected to have headroom.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HeadroomTimestamp(u64);

impl HeadroomTimestamp {
    /// Creates a headroom timestamp in Unix seconds.
    #[must_use]
    pub const fn from_unix_seconds(unix_seconds: u64) -> Self {
        Self(unix_seconds)
    }

    /// Returns the timestamp in Unix seconds.
    #[must_use]
    pub const fn unix_seconds(self) -> u64 {
        self.0
    }
}

/// Tier of an account selected for one route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tier {
    /// Fresh quota below every configured switching point.
    Preferred,
    /// Fresh quota inside a configured switch band.
    Reserve,
    /// Quota evidence is absent, stale, or unknown.
    Unknown,
    /// OpenAI-only fallback for the legacy short-window survival guard.
    LastResort,
}

/// Why the quota assessment selected one account inside its tier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChoiceReason(RoutingReason);

impl ChoiceReason {
    /// Returns the quota assessor's stable selection reason.
    #[must_use]
    pub const fn routing_reason(self) -> RoutingReason {
        self.0
    }
}

/// One successful or unavailable account selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionOutcome {
    /// One eligible provider account won the quota ranking.
    Chosen {
        /// Selected account identity.
        account: AccountId,
        /// Selected account tier.
        tier: Tier,
        /// Why this account won inside its tier.
        reason: ChoiceReason,
    },
    /// No eligible account exists; the reason follows R12 precedence.
    Unavailable(UnavailableReason),
}

impl SelectionOutcome {
    /// Builds a complete choice or R12 unavailability result for one provider route.
    #[must_use]
    pub fn from_assessment(
        assessment: &BurnDownRouteBandAssessmentResult,
        provider: Provider,
        credential_store: CredentialStoreAvailability,
        account_states: &[SelectionAccountState],
    ) -> Self {
        if credential_store == CredentialStoreAvailability::Available
            && let Some(chosen) = Self::chosen_from_assessment(assessment)
        {
            return chosen;
        }
        Self::Unavailable(classify_unavailable_reason(
            provider,
            credential_store,
            account_states,
        ))
    }

    /// Converts an assessment with a candidate into its typed choice result.
    #[must_use]
    pub fn chosen_from_assessment(assessment: &BurnDownRouteBandAssessmentResult) -> Option<Self> {
        let account_id = assessment.preferred_next()?.clone();
        let account = assessment
            .accounts()
            .iter()
            .find(|account| account.account_id() == &account_id)?;
        let tier = match assessment.selected_pool() {
            SelectedPool::Usable => Tier::Preferred,
            SelectedPool::Reserve => Tier::Reserve,
            SelectedPool::Unknown => Tier::Unknown,
            SelectedPool::LastResort => Tier::LastResort,
            SelectedPool::None => return None,
        };

        Some(Self::Chosen {
            account: account_id,
            tier,
            reason: ChoiceReason(account.routing_reason()),
        })
    }
}

/// Account restriction supplied to the R12 precedence classifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionAccountRestriction {
    /// Credentials are usable and no quota restriction blocks the account.
    Available,
    /// The enabled account must be logged in before it can serve requests.
    NeedsLogin,
    /// One or more provider-rejected windows still have no fresh headroom observation.
    Exhausted {
        /// Provider-reported reset time for each rejected window.
        reported_resets: Vec<Option<HeadroomTimestamp>>,
    },
    /// The account is held from new requests by a floor or missing weekly evidence.
    HeldByFloor {
        /// The reason the account is held.
        reason: SelectionHoldReason,
    },
}

/// Specific R12 reason an otherwise usable account is held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionHoldReason {
    /// Current quota is at or below the configured hard floor.
    HardFloor,
    /// A weekly floor is configured but the weekly observation is not fresh.
    WaitingForFreshWeeklyObservation,
    /// The assessment had no reliable quota evidence to classify the account.
    UnknownQuotaEvidence,
}

/// Provider account facts needed to classify an unavailable selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionAccountState {
    account_id: AccountId,
    provider: Provider,
    enabled: bool,
    restriction: SelectionAccountRestriction,
}

impl SelectionAccountState {
    /// Creates a state row for an enabled account.
    #[must_use]
    pub fn enabled(
        account_id: AccountId,
        provider: Provider,
        restriction: SelectionAccountRestriction,
    ) -> Self {
        Self {
            account_id,
            provider,
            enabled: true,
            restriction,
        }
    }

    /// Creates a state row for a disabled account.
    #[must_use]
    pub fn disabled(account_id: AccountId, provider: Provider) -> Self {
        Self {
            account_id,
            provider,
            enabled: false,
            restriction: SelectionAccountRestriction::NeedsLogin,
        }
    }

    /// Returns the account identity.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }
}

/// State of Router's pooled-credential store for one selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialStoreAvailability {
    /// The key is readable and pooled-credential migration is complete.
    Available,
    /// Router cannot read its Keychain key.
    KeyUnreadable,
    /// The pooled-credential conversion has not completed.
    MigrationIncomplete,
}

/// One named account held by a configured quota floor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeldAccount {
    account_id: AccountId,
    reason: SelectionHoldReason,
}

impl HeldAccount {
    /// Returns the held account identity.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns why the account is held.
    #[must_use]
    pub const fn reason(&self) -> SelectionHoldReason {
        self.reason
    }
}

/// First applicable R12 reason no provider account can serve the request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnavailableReason {
    /// No account exists for the provider, or every provider account is disabled.
    NoneConfiguredOrEnabled,
    /// Router's Keychain key cannot be read.
    KeyUnreadable,
    /// Pooled credentials are unavailable until conversion completes.
    MigrationIncomplete,
    /// Every enabled provider account needs a successful login.
    AllNeedLogin {
        /// Accounts that need login.
        accounts: Vec<AccountId>,
    },
    /// Every enabled account with a usable credential is exhausted.
    AllExhausted {
        /// Earliest expected headroom across the exhausted accounts, if known.
        earliest_headroom: Option<HeadroomTimestamp>,
    },
    /// Remaining usable accounts are held by floors or stale weekly evidence.
    HeldByFloors {
        /// Accounts and their individual hold reasons.
        accounts: Vec<HeldAccount>,
    },
}

impl UnavailableReason {
    /// Returns the ordinal of this reason in the R12 precedence order.
    #[must_use]
    pub const fn r12_order(&self) -> u8 {
        match self {
            Self::NoneConfiguredOrEnabled => 1,
            Self::KeyUnreadable | Self::MigrationIncomplete => 2,
            Self::AllNeedLogin { .. } => 3,
            Self::AllExhausted { .. } => 4,
            Self::HeldByFloors { .. } => 5,
        }
    }
}

/// Classifies an unavailable selection in the exact order required by R12.
#[must_use]
pub fn classify_unavailable_reason(
    provider: Provider,
    credential_store: CredentialStoreAvailability,
    account_states: &[SelectionAccountState],
) -> UnavailableReason {
    let enabled_accounts = account_states
        .iter()
        .filter(|account| account.provider == provider && account.enabled)
        .collect::<Vec<_>>();

    if enabled_accounts.is_empty() {
        return UnavailableReason::NoneConfiguredOrEnabled;
    }

    match credential_store {
        CredentialStoreAvailability::KeyUnreadable => return UnavailableReason::KeyUnreadable,
        CredentialStoreAvailability::MigrationIncomplete => {
            return UnavailableReason::MigrationIncomplete;
        }
        CredentialStoreAvailability::Available => {}
    }

    if enabled_accounts.iter().all(|account| {
        matches!(
            &account.restriction,
            SelectionAccountRestriction::NeedsLogin
        )
    }) {
        return UnavailableReason::AllNeedLogin {
            accounts: enabled_accounts
                .iter()
                .map(|account| account.account_id.clone())
                .collect(),
        };
    }

    let accounts_with_usable_credentials = enabled_accounts
        .iter()
        .copied()
        .filter(|account| {
            !matches!(
                &account.restriction,
                SelectionAccountRestriction::NeedsLogin
            )
        })
        .collect::<Vec<_>>();
    if !accounts_with_usable_credentials.is_empty()
        && accounts_with_usable_credentials.iter().all(|account| {
            matches!(
                &account.restriction,
                SelectionAccountRestriction::Exhausted { .. }
            )
        })
    {
        return UnavailableReason::AllExhausted {
            earliest_headroom: earliest_headroom(&accounts_with_usable_credentials),
        };
    }

    let held_accounts = enabled_accounts
        .iter()
        .filter_map(|account| match &account.restriction {
            SelectionAccountRestriction::HeldByFloor { reason } => Some(HeldAccount {
                account_id: account.account_id.clone(),
                reason: *reason,
            }),
            SelectionAccountRestriction::Available => Some(HeldAccount {
                account_id: account.account_id.clone(),
                reason: SelectionHoldReason::UnknownQuotaEvidence,
            }),
            SelectionAccountRestriction::NeedsLogin
            | SelectionAccountRestriction::Exhausted { .. } => None,
        })
        .collect();
    UnavailableReason::HeldByFloors {
        accounts: held_accounts,
    }
}

fn earliest_headroom(accounts: &[&SelectionAccountState]) -> Option<HeadroomTimestamp> {
    let mut account_headroom = Vec::with_capacity(accounts.len());
    for account in accounts {
        let SelectionAccountRestriction::Exhausted { reported_resets } = &account.restriction
        else {
            continue;
        };
        if reported_resets.is_empty() {
            return None;
        }
        let latest_rejected_window_reset = reported_resets
            .iter()
            .copied()
            .collect::<Option<Vec<_>>>()?;
        let account_headroom_timestamp = latest_rejected_window_reset.into_iter().max()?;
        account_headroom.push(account_headroom_timestamp);
    }
    account_headroom.into_iter().min()
}

#[cfg(test)]
#[path = "selection_outcome_tests.rs"]
mod tests;
