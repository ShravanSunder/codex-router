//! Typed selection outcomes and Claude unavailable-reason precedence.

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::WindowKind;

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
    /// The account has one or more durable rejected quota windows.
    Exhausted,
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
}

/// One persisted per-window observation projected into selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionWindowObservation {
    window_kind: WindowKind,
    remaining_basis_points: u32,
    reset_unix_seconds: Option<HeadroomTimestamp>,
    observation_started_at: u64,
    freshness: crate::burn_down::QuotaEvidenceFreshness,
}

impl SelectionWindowObservation {
    /// Creates one typed observation for selector and R12 reasoning.
    #[must_use]
    pub const fn new(
        window_kind: WindowKind,
        remaining_basis_points: u32,
        reset_unix_seconds: Option<HeadroomTimestamp>,
        observation_started_at: u64,
        freshness: crate::burn_down::QuotaEvidenceFreshness,
    ) -> Self {
        Self {
            window_kind,
            remaining_basis_points,
            reset_unix_seconds,
            observation_started_at,
            freshness,
        }
    }

    /// Returns the window kind.
    #[must_use]
    pub const fn window_kind(self) -> WindowKind {
        self.window_kind
    }

    /// Returns exact remaining quota in basis points.
    #[must_use]
    pub const fn remaining_basis_points(self) -> u32 {
        self.remaining_basis_points
    }

    /// Returns the provider-reported reset time.
    #[must_use]
    pub const fn reset_unix_seconds(self) -> Option<HeadroomTimestamp> {
        self.reset_unix_seconds
    }

    /// Returns when the observation poll or response began.
    #[must_use]
    pub const fn observation_started_at(self) -> u64 {
        self.observation_started_at
    }

    /// Returns whether this observation is fresh for selection.
    #[must_use]
    pub const fn freshness(self) -> crate::burn_down::QuotaEvidenceFreshness {
        self.freshness
    }
}

/// One persisted provider rejection projected into selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionWindowRejection {
    window_kind: WindowKind,
    rejected_at: u64,
    reported_reset: Option<HeadroomTimestamp>,
}

impl SelectionWindowRejection {
    /// Creates one typed rejection barrier for selector and R12 reasoning.
    #[must_use]
    pub const fn new(
        window_kind: WindowKind,
        rejected_at: u64,
        reported_reset: Option<HeadroomTimestamp>,
    ) -> Self {
        Self {
            window_kind,
            rejected_at,
            reported_reset,
        }
    }

    /// Returns the rejected window kind.
    #[must_use]
    pub const fn window_kind(self) -> WindowKind {
        self.window_kind
    }

    /// Returns when the provider rejected the account's window.
    #[must_use]
    pub const fn rejected_at(self) -> u64 {
        self.rejected_at
    }

    /// Returns the reset reported with the rejection.
    #[must_use]
    pub const fn reported_reset(self) -> Option<HeadroomTimestamp> {
        self.reported_reset
    }
}

/// Provider account state needed to classify an unavailable selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionAccountState {
    /// Disabled account; it does not participate in credential or quota precedence.
    Disabled {
        /// Account identity.
        account_id: AccountId,
        /// Provider that owns the account.
        provider: Provider,
    },
    /// Enabled account and its typed current restrictions and D9 state.
    Enabled {
        /// Account identity.
        account_id: AccountId,
        /// Provider that owns the account.
        provider: Provider,
        /// Current credential or quota restriction.
        restriction: SelectionAccountRestriction,
        /// Latest persisted observation for each quota window.
        window_observations: Vec<SelectionWindowObservation>,
        /// Latest persisted rejection for each quota window.
        window_rejections: Vec<SelectionWindowRejection>,
    },
}

impl SelectionAccountState {
    /// Creates state for one enabled account.
    #[must_use]
    pub fn enabled(
        account_id: AccountId,
        provider: Provider,
        restriction: SelectionAccountRestriction,
        window_observations: Vec<SelectionWindowObservation>,
        window_rejections: Vec<SelectionWindowRejection>,
    ) -> Self {
        Self::Enabled {
            account_id,
            provider,
            restriction,
            window_observations,
            window_rejections,
        }
    }

    /// Creates state for one disabled account without inventing a credential placeholder.
    #[must_use]
    pub fn disabled(account_id: AccountId, provider: Provider) -> Self {
        Self::Disabled {
            account_id,
            provider,
        }
    }

    /// Returns the account identity.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        match self {
            Self::Disabled { account_id, .. } | Self::Enabled { account_id, .. } => account_id,
        }
    }

    /// Returns the account provider.
    #[must_use]
    pub const fn provider(&self) -> Provider {
        match self {
            Self::Disabled { provider, .. } | Self::Enabled { provider, .. } => *provider,
        }
    }

    /// Returns whether the account is enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    /// Returns the enabled account's current restriction.
    #[must_use]
    pub const fn restriction(&self) -> Option<&SelectionAccountRestriction> {
        match self {
            Self::Disabled { .. } => None,
            Self::Enabled { restriction, .. } => Some(restriction),
        }
    }

    /// Returns projected per-window observations for an enabled account.
    #[must_use]
    pub fn window_observations(&self) -> &[SelectionWindowObservation] {
        match self {
            Self::Disabled { .. } => &[],
            Self::Enabled {
                window_observations,
                ..
            } => window_observations,
        }
    }

    /// Returns projected per-window rejection barriers for an enabled account.
    #[must_use]
    pub fn window_rejections(&self) -> &[SelectionWindowRejection] {
        match self {
            Self::Disabled { .. } => &[],
            Self::Enabled {
                window_rejections, ..
            } => window_rejections,
        }
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
        .filter(|account| account.provider() == provider && account.is_enabled())
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

    if enabled_accounts
        .iter()
        .all(|account| account.restriction() == Some(&SelectionAccountRestriction::NeedsLogin))
    {
        return UnavailableReason::AllNeedLogin {
            accounts: enabled_accounts
                .iter()
                .map(|account| account.account_id().clone())
                .collect(),
        };
    }

    let accounts_with_usable_credentials = enabled_accounts
        .iter()
        .copied()
        .filter(|account| account.restriction() != Some(&SelectionAccountRestriction::NeedsLogin))
        .collect::<Vec<_>>();
    if !accounts_with_usable_credentials.is_empty()
        && accounts_with_usable_credentials
            .iter()
            .all(|account| account.restriction() == Some(&SelectionAccountRestriction::Exhausted))
    {
        return UnavailableReason::AllExhausted {
            earliest_headroom: earliest_headroom(&accounts_with_usable_credentials),
        };
    }

    let held_accounts = enabled_accounts
        .iter()
        .filter_map(|account| match account.restriction() {
            Some(SelectionAccountRestriction::HeldByFloor { reason }) => Some(HeldAccount {
                account_id: account.account_id().clone(),
                reason: *reason,
            }),
            _ => None,
        })
        .collect();
    UnavailableReason::HeldByFloors {
        accounts: held_accounts,
    }
}

fn earliest_headroom(accounts: &[&SelectionAccountState]) -> Option<HeadroomTimestamp> {
    let mut account_headroom = Vec::with_capacity(accounts.len());
    for account in accounts {
        if account.restriction() != Some(&SelectionAccountRestriction::Exhausted) {
            continue;
        }
        if account.window_rejections().is_empty() {
            return None;
        }
        let latest_rejected_window_reset = account
            .window_rejections()
            .iter()
            .map(|rejection| rejection.reported_reset())
            .collect::<Option<Vec<_>>>()?;
        let account_headroom_timestamp = latest_rejected_window_reset.into_iter().max()?;
        account_headroom.push(account_headroom_timestamp);
    }
    account_headroom.into_iter().min()
}

#[cfg(test)]
#[path = "selection_outcome_tests.rs"]
mod tests;
