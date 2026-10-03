//! Public route and account assessment outputs.
use super::REACTIVE_RECONNECT_MIN_RUNWAY_SECONDS;
use super::routing_reasons::{QuotaEvidenceReason, RoutingReason};
use crate::run_rate::QuotaRunRateConfidence;
use codex_router_core::ids::AccountId;
use codex_router_core::routes::RouteBand;

/// Route-band assessment support status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteBandAssessmentStatus {
    /// Route band is supported by the burn-down scorer.
    Supported,
    /// Route band is not supported by the burn-down scorer.
    UnsupportedRouteBand,
}

/// Route-band assessment output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BurnDownRouteBandAssessmentResult {
    pub(super) route_band: RouteBand,
    pub(super) route_status: RouteBandAssessmentStatus,
    pub(super) accounts: Vec<BurnDownAccountAssessment>,
    pub(super) selected_pool: SelectedPool,
    pub(super) weighted_candidates: Vec<(AccountId, u32)>,
    pub(super) preferred_next: Option<AccountId>,
}

impl BurnDownRouteBandAssessmentResult {
    /// Returns the assessed route band.
    #[must_use]
    pub const fn route_band(&self) -> RouteBand {
        self.route_band
    }

    /// Returns the route-band assessment support status.
    #[must_use]
    pub const fn route_status(&self) -> RouteBandAssessmentStatus {
        self.route_status
    }

    /// Returns account assessments in deterministic account order.
    #[must_use]
    pub fn accounts(&self) -> &[BurnDownAccountAssessment] {
        &self.accounts
    }

    /// Returns the selected availability pool.
    #[must_use]
    pub const fn selected_pool(&self) -> SelectedPool {
        self.selected_pool
    }

    /// Returns ordered weighted candidates.
    #[must_use]
    pub fn weighted_candidates(&self) -> &[(AccountId, u32)] {
        &self.weighted_candidates
    }

    /// Returns neutral preferred next account.
    #[must_use]
    pub const fn preferred_next(&self) -> Option<&AccountId> {
        self.preferred_next.as_ref()
    }
}

/// Per-account assessment output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BurnDownAccountAssessment {
    pub(super) account_id: AccountId,
    pub(super) account_label: String,
    pub(super) availability: AccountAvailability,
    pub(super) freshness: QuotaEvidenceFreshness,
    pub(super) routing_exclusion: RoutingExclusion,
    pub(super) limiting_window: Option<LimitingWindow>,
    pub(super) quota_evidence_reason: QuotaEvidenceReason,
    pub(super) short_pressure: u32,
    pub(super) long_pressure: u32,
    pub(super) short_salvage: u32,
    pub(super) long_salvage: u32,
    pub(super) projected_burn_pressure: u32,
    pub(super) routing_weight: Option<u32>,
    pub(super) routing_reason: RoutingReason,
    pub(super) weekly_floor_switch_band: bool,
    pub(super) credit_backed: bool,
    pub(super) preferred_next: bool,
    pub(super) far_idle_priority: bool,
    pub(super) current_active_sessions: u32,
    pub(super) weekly_reset_unix_seconds: Option<u64>,
    pub(super) weekly_projected_exhaustion_unix_seconds: Option<u64>,
    pub(super) weekly_survives_to_reset: bool,
    pub(super) weekly_survival_margin_basis_points: Option<i64>,
    pub(super) weekly_burn_rate_confidence: QuotaRunRateConfidence,
    pub(super) weekly_in_drain_pool: bool,
    pub(super) required_active_connections_to_drain: Option<u32>,
    pub(super) projected_drain_gap_after_selection: Option<i64>,
    pub(super) projected_weekly_runway_seconds: Option<u64>,
    pub(super) weekly_remaining_headroom: Option<u32>,
    pub(super) weekly_remaining_basis_points: Option<u32>,
    pub(super) weekly_projected_candidate_burn_basis_points_per_hour: Option<u32>,
    pub(super) initial_admission_priority: bool,
    pub(super) salvage_sort_key: Option<SalvageSortKey>,
}

impl BurnDownAccountAssessment {
    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the account label.
    #[must_use]
    pub fn account_label(&self) -> &str {
        &self.account_label
    }

    /// Returns the availability class.
    #[must_use]
    pub const fn availability(&self) -> AccountAvailability {
        self.availability
    }

    /// Returns evidence freshness.
    #[must_use]
    pub const fn freshness(&self) -> QuotaEvidenceFreshness {
        self.freshness
    }

    /// Returns routing exclusion.
    #[must_use]
    pub const fn routing_exclusion(&self) -> RoutingExclusion {
        self.routing_exclusion
    }

    /// Returns limiting window.
    #[must_use]
    pub const fn limiting_window(&self) -> Option<LimitingWindow> {
        self.limiting_window
    }

    /// Returns quota evidence reason.
    #[must_use]
    pub const fn quota_evidence_reason(&self) -> QuotaEvidenceReason {
        self.quota_evidence_reason
    }

    /// Returns short-window pressure.
    #[must_use]
    pub const fn short_pressure(&self) -> u32 {
        self.short_pressure
    }

    /// Returns long-window pressure.
    #[must_use]
    pub const fn long_pressure(&self) -> u32 {
        self.long_pressure
    }

    /// Returns short-window salvage.
    #[must_use]
    pub const fn short_salvage(&self) -> u32 {
        self.short_salvage
    }

    /// Returns long-window salvage.
    #[must_use]
    pub const fn long_salvage(&self) -> u32 {
        self.long_salvage
    }

    /// Returns projected burn pressure including active load.
    #[must_use]
    pub const fn projected_burn_pressure(&self) -> u32 {
        self.projected_burn_pressure
    }

    /// Returns routing weight.
    #[must_use]
    pub const fn routing_weight(&self) -> Option<u32> {
        self.routing_weight
    }

    /// Returns routing reason.
    #[must_use]
    pub const fn routing_reason(&self) -> RoutingReason {
        self.routing_reason
    }

    /// Whether fresh eligible weekly evidence is above the hard floor and at the switch point.
    #[must_use]
    pub const fn in_weekly_floor_switch_band(&self) -> bool {
        self.weekly_floor_switch_band
    }

    /// Whether this account is a safe peer for an early floor switch.
    #[must_use]
    pub fn is_healthy_floor_switch_peer(&self) -> bool {
        if self.weekly_floor_switch_band
            || self.routing_exclusion != RoutingExclusion::None
            || self.freshness != QuotaEvidenceFreshness::Fresh
            || !matches!(
                self.availability,
                AccountAvailability::Usable | AccountAvailability::Reserve
            )
        {
            return false;
        }
        if self.credit_backed {
            return self.availability == AccountAvailability::Reserve
                && self.quota_evidence_reason == QuotaEvidenceReason::CreditBacked
                && self.routing_reason == RoutingReason::CreditBacked;
        }
        if self.quota_evidence_reason != QuotaEvidenceReason::Ok {
            return false;
        }
        match self.projected_weekly_runway_seconds {
            Some(runway) => runway >= REACTIVE_RECONNECT_MIN_RUNWAY_SECONDS,
            None => {
                self.current_active_sessions == 0
                    && (matches!(
                        self.weekly_burn_rate_confidence,
                        QuotaRunRateConfidence::Unknown
                            | QuotaRunRateConfidence::Insufficient
                            | QuotaRunRateConfidence::Stale
                    ) || self.weekly_projected_candidate_burn_basis_points_per_hour == Some(0))
            }
        }
    }

    /// Returns whether this is neutral preferred next.
    #[must_use]
    pub const fn preferred_next(&self) -> bool {
        self.preferred_next
    }

    /// Returns current active sessions used for selection.
    #[must_use]
    pub const fn current_active_sessions_for_selection(&self) -> u32 {
        self.current_active_sessions
    }

    /// Returns weekly survival margin in basis points.
    #[must_use]
    pub const fn weekly_survival_margin_basis_points(&self) -> Option<i64> {
        self.weekly_survival_margin_basis_points
    }

    /// Returns projected weekly exhaustion time.
    #[must_use]
    pub const fn weekly_projected_exhaustion_unix_seconds(&self) -> Option<u64> {
        self.weekly_projected_exhaustion_unix_seconds
    }

    /// Returns weekly burn-rate confidence.
    #[must_use]
    pub const fn weekly_burn_rate_confidence(&self) -> QuotaRunRateConfidence {
        self.weekly_burn_rate_confidence
    }

    /// Returns whether the account is in the near-reset weekly drain pool.
    #[must_use]
    pub const fn weekly_in_drain_pool(&self) -> bool {
        self.weekly_in_drain_pool
    }

    /// Returns active sessions needed to drain weekly quota by reset.
    #[must_use]
    pub const fn required_active_connections_to_drain(&self) -> Option<u32> {
        self.required_active_connections_to_drain
    }

    /// Returns drain gap after adding one candidate session.
    #[must_use]
    pub const fn projected_drain_gap_after_selection(&self) -> Option<i64> {
        self.projected_drain_gap_after_selection
    }

    /// Returns projected weekly runway after adding one candidate session.
    #[must_use]
    pub const fn projected_weekly_runway_seconds(&self) -> Option<u64> {
        self.projected_weekly_runway_seconds
    }

    /// Returns whether this account has the one-client initial-admission priority.
    #[must_use]
    pub const fn has_initial_admission_priority(&self) -> bool {
        self.initial_admission_priority
    }
}

/// Account availability class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountAvailability {
    /// Selectable in the normal pool.
    Usable,
    /// Selectable only when no usable account exists.
    Reserve,
    /// Not selectable because known quota is exhausted or ineligible.
    Blocked,
    /// Selectable only as fallback because quota evidence is missing or unknown.
    Unknown,
    /// Excluded because account metadata disallows routing.
    Excluded,
}

/// Selected candidate pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectedPool {
    /// Usable pool selected.
    Usable,
    /// Reserve pool selected.
    Reserve,
    /// Unknown fallback pool selected because no known usable or reserve account exists.
    Unknown,
    /// Last-resort pool selected because only 5h-guard accounts remain.
    LastResort,
    /// No selectable pool exists.
    None,
}

/// Quota evidence freshness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaEvidenceFreshness {
    /// All relevant evidence is fresh.
    Fresh,
    /// At least one relevant window is stale.
    Stale,
    /// Evidence is insufficient.
    Unknown,
}

/// Hard routing exclusion applied before candidate-pool construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutingExclusion {
    /// No non-quota exclusion applies.
    None,
    /// Account is disabled.
    Disabled,
    /// Account lacks active credentials.
    MissingCredential,
    /// Account is at or below its configured hard weekly quota floor or lacks required evidence.
    WeeklyQuotaFloor,
}

/// Limiting window explanation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LimitingWindow {
    pub(super) window_seconds: u64,
    pub(super) remaining_headroom: u32,
    pub(super) pressure: u32,
    pub(super) reset_unix_seconds: Option<u64>,
}

impl LimitingWindow {
    /// Returns window seconds.
    #[must_use]
    pub const fn window_seconds(self) -> u64 {
        self.window_seconds
    }

    /// Returns remaining headroom.
    #[must_use]
    pub const fn remaining_headroom(self) -> u32 {
        self.remaining_headroom
    }

    /// Returns pressure.
    #[must_use]
    pub const fn pressure(self) -> u32 {
        self.pressure
    }

    /// Returns reset time.
    #[must_use]
    pub const fn reset_unix_seconds(self) -> Option<u64> {
        self.reset_unix_seconds
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct SalvageSortKey {
    pub(super) reset_unix_seconds: u64,
    pub(super) window_seconds: u64,
}
