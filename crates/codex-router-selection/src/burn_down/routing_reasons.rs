//! Public quota-routing explanations and reason assignment.
use super::assessment_result::{
    AccountAvailability, BurnDownAccountAssessment, RoutingExclusion, SelectedPool,
};
use super::route_assessment::account_matches_selected_pool;

/// Raw quota evidence reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaEvidenceReason {
    /// Quota evidence supports routing.
    Ok,
    /// Account needs quota probe.
    NeedsQuotaProbe,
    /// Expected v1 window is missing.
    MissingExpectedWindow,
    /// A window is ineligible.
    WindowIneligible,
    /// A window is exhausted.
    WindowExhausted,
    /// A window has unknown quota.
    UnknownQuotaWindow,
    /// A window is missing reset time.
    MissingResetTime,
    /// Short-window flow guard blocks new work.
    ShortWindowGuard,
    /// Account is disabled.
    AccountDisabled,
    /// Account lacks active credentials.
    MissingCredential,
    /// Configured hard weekly quota floor excludes the account from new work.
    WeeklyQuotaFloor,
    /// Fresh opted-in usage credits back this exhausted Responses Reserve candidate.
    CreditBacked,
}

/// Public routing reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutingReason {
    /// Preferred for one initial real client because fresh near-reset quota has no runway estimate.
    PreferredNearResetInitialAdmission,
    /// Preferred because near-reset weekly quota needs more active sessions to drain.
    PreferredNearResetDrainable,
    /// Preferred because near-reset weekly quota is safe to keep draining.
    PreferredNearResetControlledDrain,
    /// Preferred because weekly quota is healthier than alternatives.
    PreferredWeeklyHealthier,
    /// Preferred because weekly reset is near.
    PreferredWeeklyResetSoon,
    /// Preferred because the short window reset is near.
    PreferredShortResetSoon,
    /// Preferred because projected burn lasts longer than alternatives.
    PreferredProjectedBurn,
    /// Preferred by the safest quota guard when no narrower reason wins.
    PreferredSafestQuota,
    /// Preferred as the least-bad account when only 5h-guard accounts remain.
    PreferredLastResortShortWindowGuard,
    /// Same-pool selectable account.
    AvailableSamePool,
    /// Reserve account held behind usable accounts.
    HeldReserve,
    /// Unknown account held behind known accounts.
    HeldUnknown,
    /// Account is held because its short-window quota would stall before reset.
    HeldShortWindowGuard,
    /// Preferred fallback account that needs refresh.
    UnknownFallbackPreferred,
    /// Non-preferred fallback account in the unknown pool.
    UnknownFallbackAvailable,
    /// Preferred real initial work for eligible idle allowance beyond 48 hours.
    PreferredIdleFarResetAdmission,
    /// Above-floor account yields new work to a healthy peer at its switch point.
    HeldFloorSwitch,
    /// Excluded because the account is disabled.
    ExcludedDisabled,
    /// Excluded because the account has no active credential.
    ExcludedMissingCredential,
    /// Excluded by the configured hard weekly quota floor.
    ExcludedWeeklyQuotaFloor,
    /// Blocked because quota is exhausted.
    BlockedWindowExhausted,
    /// Blocked because quota is ineligible.
    BlockedWindowIneligible,
    /// Candidate is in Reserve because current usage credits back exhausted Responses quota.
    CreditBacked,
    /// Held because included quota is available on another account.
    HeldForIncludedQuota,
}

impl RoutingReason {
    /// Returns the stable machine code for this public routing reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreferredNearResetInitialAdmission => "preferred_near_reset_initial_admission",
            Self::PreferredNearResetDrainable => "preferred_near_reset_drainable",
            Self::PreferredNearResetControlledDrain => "preferred_near_reset_controlled_drain",
            Self::PreferredWeeklyHealthier => "preferred_weekly_healthier",
            Self::PreferredWeeklyResetSoon => "preferred_weekly_reset_soon",
            Self::PreferredShortResetSoon => "preferred_short_reset_soon",
            Self::PreferredProjectedBurn => "preferred_projected_burn",
            Self::PreferredSafestQuota => "preferred_safest_quota",
            Self::PreferredLastResortShortWindowGuard => "preferred_last_resort_short_window_guard",
            Self::AvailableSamePool => "available_same_pool",
            Self::HeldReserve => "held_reserve",
            Self::HeldUnknown => "held_unknown",
            Self::UnknownFallbackPreferred => "unknown_fallback_preferred",
            Self::UnknownFallbackAvailable => "unknown_fallback_available",
            Self::PreferredIdleFarResetAdmission => "preferred_idle_far_reset",
            Self::HeldFloorSwitch => "held_floor_switch",
            Self::ExcludedDisabled => "excluded_disabled",
            Self::ExcludedMissingCredential => "excluded_missing_credential",
            Self::ExcludedWeeklyQuotaFloor => "excluded_weekly_quota_floor",
            Self::BlockedWindowExhausted => "blocked_window_exhausted",
            Self::BlockedWindowIneligible => "blocked_window_ineligible",
            Self::CreditBacked => "credit_backed",
            Self::HeldForIncludedQuota => "held_for_included_quota",
            Self::HeldShortWindowGuard => "held_short_window_guard",
        }
    }

    /// Returns the stable human phrase for this public routing reason.
    #[must_use]
    pub const fn human_phrase(self) -> &'static str {
        match self {
            Self::PreferredNearResetInitialAdmission => {
                "preferred next: near-reset initial admission"
            }
            Self::PreferredNearResetDrainable => "preferred next: near-reset drainable",
            Self::PreferredNearResetControlledDrain => {
                "preferred next: near-reset controlled drain"
            }
            Self::PreferredWeeklyHealthier => "preferred next: weekly healthier",
            Self::PreferredWeeklyResetSoon => "preferred next: weekly reset soon",
            Self::PreferredShortResetSoon => "preferred next: 5h reset soon",
            Self::PreferredProjectedBurn => "preferred next: projected burn",
            Self::PreferredSafestQuota => "preferred next: safest quota",
            Self::PreferredLastResortShortWindowGuard => "preferred next: last-resort 5h guard",
            Self::AvailableSamePool => "available: same pool",
            Self::HeldReserve => "held: far-reset reserve",
            Self::HeldUnknown => "held: needs refresh",
            Self::UnknownFallbackPreferred => "fallback: needs refresh",
            Self::UnknownFallbackAvailable => "fallback: same unknown pool",
            Self::PreferredIdleFarResetAdmission => "preferred next: idle far-reset allowance",
            Self::HeldFloorSwitch => "held: weekly floor switch",
            Self::ExcludedDisabled => "blocked: disabled",
            Self::ExcludedMissingCredential => "blocked: missing credential",
            Self::ExcludedWeeklyQuotaFloor => "blocked: weekly quota floor",
            Self::BlockedWindowExhausted => "blocked: quota empty",
            Self::BlockedWindowIneligible => "blocked: quota ineligible",
            Self::CreditBacked => "available: usage credits",
            Self::HeldForIncludedQuota => "held: included quota available",
            Self::HeldShortWindowGuard => "held: 5h guard",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RoutingReasonContext {
    selected_pool: SelectedPool,
    preferred_long_pressure: u32,
    preferred_projected_burn_pressure: u32,
    has_worse_known_selected_pool_long_pressure: bool,
    has_worse_known_selected_pool_projected_burn_pressure: bool,
    has_held_reserve_account: bool,
}

impl RoutingReasonContext {
    pub(super) fn from_accounts(
        accounts: &[BurnDownAccountAssessment],
        selected_pool: SelectedPool,
    ) -> Self {
        let preferred_long_pressure = accounts
            .iter()
            .find(|account| account.preferred_next)
            .map_or(0, |account| account.long_pressure);
        let preferred_projected_burn_pressure = accounts
            .iter()
            .find(|account| account.preferred_next)
            .map_or(0, |account| account.projected_burn_pressure);
        let has_worse_known_selected_pool_long_pressure = accounts.iter().any(|account| {
            account_matches_selected_pool(account, selected_pool)
                && account.routing_weight.is_some()
                && matches!(
                    account.availability,
                    AccountAvailability::Usable | AccountAvailability::Reserve
                )
                && !account.preferred_next
                && account.long_pressure > preferred_long_pressure
        });
        let has_worse_known_selected_pool_projected_burn_pressure =
            accounts.iter().any(|account| {
                account_matches_selected_pool(account, selected_pool)
                    && account.routing_weight.is_some()
                    && matches!(
                        account.availability,
                        AccountAvailability::Usable | AccountAvailability::Reserve
                    )
                    && !account.preferred_next
                    && account.projected_burn_pressure > preferred_projected_burn_pressure
            });
        let has_held_reserve_account = selected_pool == SelectedPool::Usable
            && accounts.iter().any(|account| {
                account.availability == AccountAvailability::Reserve
                    && account.routing_weight.is_some()
            });

        Self {
            selected_pool,
            preferred_long_pressure,
            preferred_projected_burn_pressure,
            has_worse_known_selected_pool_long_pressure,
            has_worse_known_selected_pool_projected_burn_pressure,
            has_held_reserve_account,
        }
    }
}

pub(super) fn routing_reason_for_account(
    account: &BurnDownAccountAssessment,
    context: RoutingReasonContext,
) -> RoutingReason {
    match account.routing_exclusion {
        RoutingExclusion::Disabled => return RoutingReason::ExcludedDisabled,
        RoutingExclusion::MissingCredential => return RoutingReason::ExcludedMissingCredential,
        RoutingExclusion::WeeklyQuotaFloor => {
            return RoutingReason::ExcludedWeeklyQuotaFloor;
        }
        RoutingExclusion::None => {}
    }

    match account.quota_evidence_reason {
        QuotaEvidenceReason::CreditBacked if account.routing_weight.is_none() => {
            return RoutingReason::HeldForIncludedQuota;
        }
        QuotaEvidenceReason::CreditBacked => return RoutingReason::CreditBacked,
        QuotaEvidenceReason::WindowExhausted => return RoutingReason::BlockedWindowExhausted,
        QuotaEvidenceReason::WindowIneligible => return RoutingReason::BlockedWindowIneligible,
        QuotaEvidenceReason::ShortWindowGuard if account.preferred_next => {
            return RoutingReason::PreferredLastResortShortWindowGuard;
        }
        QuotaEvidenceReason::ShortWindowGuard => return RoutingReason::HeldShortWindowGuard,
        QuotaEvidenceReason::Ok
        | QuotaEvidenceReason::NeedsQuotaProbe
        | QuotaEvidenceReason::MissingExpectedWindow
        | QuotaEvidenceReason::UnknownQuotaWindow
        | QuotaEvidenceReason::MissingResetTime
        | QuotaEvidenceReason::AccountDisabled
        | QuotaEvidenceReason::MissingCredential
        | QuotaEvidenceReason::WeeklyQuotaFloor => {}
    }

    match account.availability {
        AccountAvailability::Unknown if context.selected_pool != SelectedPool::Unknown => {
            return RoutingReason::HeldUnknown;
        }
        AccountAvailability::Reserve if context.selected_pool == SelectedPool::Usable => {
            return RoutingReason::HeldReserve;
        }
        AccountAvailability::Unknown if !account.preferred_next => {
            return RoutingReason::UnknownFallbackAvailable;
        }
        AccountAvailability::Unknown => return RoutingReason::UnknownFallbackPreferred,
        AccountAvailability::Usable | AccountAvailability::Reserve if !account.preferred_next => {
            return RoutingReason::AvailableSamePool;
        }
        AccountAvailability::Usable | AccountAvailability::Reserve => {}
        AccountAvailability::Blocked => return RoutingReason::BlockedWindowIneligible,
        AccountAvailability::Excluded => return RoutingReason::ExcludedDisabled,
    }

    if account.initial_admission_priority {
        return RoutingReason::PreferredNearResetInitialAdmission;
    }

    if account.weekly_in_drain_pool {
        if account
            .projected_drain_gap_after_selection
            .is_some_and(|gap| gap > 0)
        {
            return RoutingReason::PreferredNearResetDrainable;
        }

        return RoutingReason::PreferredNearResetControlledDrain;
    }
    if account.far_idle_priority {
        return RoutingReason::PreferredIdleFarResetAdmission;
    }
    if account.long_salvage > 0 {
        return RoutingReason::PreferredWeeklyResetSoon;
    }
    if !account.weekly_survives_to_reset
        && account.weekly_projected_exhaustion_unix_seconds.is_some()
    {
        return RoutingReason::PreferredProjectedBurn;
    }
    if account.long_pressure == context.preferred_long_pressure
        && (context.has_worse_known_selected_pool_long_pressure || context.has_held_reserve_account)
    {
        return RoutingReason::PreferredWeeklyHealthier;
    }
    if account.short_salvage > 0 {
        return RoutingReason::PreferredShortResetSoon;
    }
    if account.projected_burn_pressure == context.preferred_projected_burn_pressure
        && (context.has_worse_known_selected_pool_projected_burn_pressure
            || context.has_held_reserve_account)
    {
        return RoutingReason::PreferredProjectedBurn;
    }

    RoutingReason::PreferredSafestQuota
}
