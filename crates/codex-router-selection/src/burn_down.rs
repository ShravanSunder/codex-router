//! Reset-aware quota burn-down assessment.

use crate::run_rate::QuotaRunRateConfidence;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::safe_account_label;
use codex_router_core::routes::RouteBand;

/// Fixed v1 short quota window in seconds.
pub const V1_SHORT_WINDOW_SECONDS: u64 = 18_000;
/// Fixed v1 weekly quota window in seconds.
pub const V1_WEEKLY_WINDOW_SECONDS: u64 = 604_800;
/// Fixed v1 weekly survival safety buffer in basis points.
pub const WEEKLY_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS: i64 = 200;
/// Fixed v1 short-window survival safety buffer in basis points.
pub const SHORT_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS: i64 = 100;
/// Fixed v1 short-window near-reset threshold.
pub const SHORT_NEAR_RESET_THRESHOLD_SECONDS: u64 = 1_800;
/// Fixed v1 same-pool reset tolerance.
pub const SAME_POOL_RESET_TOLERANCE_SECONDS: u64 = 7_200;
/// Fixed v1 same-pool projected-runout tolerance.
pub const SAME_POOL_PROJECTED_RUNOUT_TOLERANCE_SECONDS: u64 = 7_200;
/// Fixed v1 same-pool survival margin tolerance in basis points.
pub const SAME_POOL_SURVIVAL_MARGIN_TOLERANCE_BASIS_POINTS: i64 = 500;
/// Fixed v1 active-session imbalance threshold.
pub const ACTIVE_SESSION_IMBALANCE_THRESHOLD: u32 = 1;
/// Fixed v1 usage-limit suspect TTL.
pub const USAGE_LIMIT_SUSPECT_TTL_SECONDS: u64 = 300;
/// Fixed v1 active-session rollup bucket size.
pub const ACTIVE_SESSION_ROLLUP_BUCKET_SECONDS: u64 = 300;
/// Fixed v1 minimum weekly runway before asking Codex to reconnect.
pub const REACTIVE_RECONNECT_MIN_RUNWAY_SECONDS: u64 = 900;
/// Fixed v1 weekly reset horizon for the near-reset drain pool.
pub const DRAIN_POOL_RESET_HORIZON_SECONDS: u64 = 172_800;

const DEFAULT_SHORT_WINDOW_CUTOFF_SECONDS: u64 = 86_400;
const DEFAULT_LONG_NEAR_RESET_MAX_SECONDS: u64 = 43_200;
const DEFAULT_RESERVE_PRESSURE_THRESHOLD: u32 = 25;
const DEFAULT_RESERVE_HEADROOM_THRESHOLD: u32 = 10;
const DEFAULT_LONG_PRESSURE_MULTIPLIER: u32 = 3;
const DEFAULT_SHORT_SALVAGE_CAP: u32 = 10;
const DEFAULT_LONG_SALVAGE_CAP: u32 = 20;
const DEFAULT_RISK_PENALTY_CAP: u32 = 90;
const DEFAULT_SELECTABLE_WEIGHT_MIN: u32 = 0;
const DEFAULT_SELECTABLE_WEIGHT_MAX: u32 = 100;
const DEFAULT_UNKNOWN_FALLBACK_WEIGHT: u32 = 1;
const DEFAULT_NEAR_ZERO_HEADROOM_THRESHOLD: u32 = 5;
const DEFAULT_NEAR_ZERO_PROJECTED_RUNOUT_SECONDS: u64 = 1_800;

/// Input for one route-band assessment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BurnDownRouteBandAssessmentInput {
    route_band: RouteBand,
    now_unix_seconds: u64,
    accounts: Vec<BurnDownAccountInput>,
    policy: BurnDownRouteBandPolicy,
}

impl BurnDownRouteBandAssessmentInput {
    /// Creates route-band assessment input.
    #[must_use]
    pub fn new(
        route_band: RouteBand,
        now_unix_seconds: u64,
        accounts: Vec<BurnDownAccountInput>,
    ) -> Self {
        Self {
            route_band,
            now_unix_seconds,
            accounts,
            policy: policy_for_route_band(route_band),
        }
    }

    /// Returns the route band.
    #[must_use]
    pub const fn route_band(&self) -> RouteBand {
        self.route_band
    }
}

/// Input for one account in a route-band assessment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BurnDownAccountInput {
    account_id: AccountId,
    account_label: String,
    windows: Vec<QuotaWindowFact>,
    account_enabled: bool,
    has_active_credential: bool,
    active_load_pressure: u32,
    current_active_sessions: u32,
}

impl BurnDownAccountInput {
    /// Creates an account input.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        account_label: impl Into<String>,
        windows: Vec<QuotaWindowFact>,
    ) -> Self {
        Self {
            account_id,
            account_label: account_label.into(),
            windows,
            account_enabled: true,
            has_active_credential: true,
            active_load_pressure: 0,
            current_active_sessions: 0,
        }
    }

    /// Sets whether the account is enabled.
    #[must_use]
    pub const fn with_account_enabled(mut self, account_enabled: bool) -> Self {
        self.account_enabled = account_enabled;
        self
    }

    /// Sets whether the account has an active credential generation.
    #[must_use]
    pub const fn with_active_credential(mut self, has_active_credential: bool) -> Self {
        self.has_active_credential = has_active_credential;
        self
    }

    /// Sets additional projected pressure from active in-flight load.
    #[must_use]
    pub const fn with_active_load_pressure(mut self, active_load_pressure: u32) -> Self {
        self.active_load_pressure = clamp_u32(active_load_pressure, 0, 100);
        self
    }

    /// Sets current active sessions for measured active-balancing decisions.
    #[must_use]
    pub const fn with_current_active_sessions(mut self, current_active_sessions: u32) -> Self {
        self.current_active_sessions = current_active_sessions;
        self
    }

    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the account windows.
    #[must_use]
    pub fn windows(&self) -> &[QuotaWindowFact] {
        &self.windows
    }

    /// Returns current active sessions.
    #[must_use]
    pub const fn current_active_sessions(&self) -> u32 {
        self.current_active_sessions
    }
}

/// Pure fact for one provider quota window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaWindowFact {
    window_seconds: u64,
    status: QuotaWindowStatus,
    remaining_headroom: u32,
    reset_unix_seconds: Option<u64>,
    observed_unix_seconds: u64,
    effective: bool,
    projected_exhaustion_unix_seconds: Option<u64>,
    per_connection_burn_basis_points_per_hour: Option<u32>,
    aggregate_burn_basis_points_per_hour: Option<u32>,
    projected_candidate_burn_basis_points_per_hour: Option<u32>,
    burn_rate_confidence: QuotaRunRateConfidence,
}

impl QuotaWindowFact {
    /// Creates a quota window fact.
    #[must_use]
    pub const fn new(window_seconds: u64, status: QuotaWindowStatus) -> Self {
        Self {
            window_seconds,
            status,
            remaining_headroom: 0,
            reset_unix_seconds: None,
            observed_unix_seconds: 0,
            effective: false,
            projected_exhaustion_unix_seconds: None,
            per_connection_burn_basis_points_per_hour: None,
            aggregate_burn_basis_points_per_hour: None,
            projected_candidate_burn_basis_points_per_hour: None,
            burn_rate_confidence: QuotaRunRateConfidence::Unknown,
        }
    }

    /// Sets remaining headroom, clamped to `0..=100`.
    #[must_use]
    pub const fn with_remaining_headroom(mut self, remaining_headroom: u32) -> Self {
        self.remaining_headroom = clamp_u32(remaining_headroom, 0, 100);
        self
    }

    /// Sets reset time.
    #[must_use]
    pub const fn with_reset_unix_seconds(mut self, reset_unix_seconds: u64) -> Self {
        self.reset_unix_seconds = Some(reset_unix_seconds);
        self
    }

    /// Sets observed time.
    #[must_use]
    pub const fn with_observed_unix_seconds(mut self, observed_unix_seconds: u64) -> Self {
        self.observed_unix_seconds = observed_unix_seconds;
        self
    }

    /// Marks the window as effective.
    #[must_use]
    pub const fn with_effective(mut self, effective: bool) -> Self {
        self.effective = effective;
        self
    }

    /// Sets projected exhaustion time.
    #[must_use]
    pub const fn with_projected_exhaustion_unix_seconds(
        mut self,
        projected_exhaustion_unix_seconds: u64,
    ) -> Self {
        self.projected_exhaustion_unix_seconds = Some(projected_exhaustion_unix_seconds);
        self
    }

    /// Returns projected exhaustion time.
    #[must_use]
    pub const fn projected_exhaustion_unix_seconds(&self) -> Option<u64> {
        self.projected_exhaustion_unix_seconds
    }

    /// Sets observed per-connection burn rate in basis points per hour.
    #[must_use]
    pub const fn with_per_connection_burn_basis_points_per_hour(
        mut self,
        per_connection_burn_basis_points_per_hour: u32,
    ) -> Self {
        self.per_connection_burn_basis_points_per_hour =
            Some(per_connection_burn_basis_points_per_hour);
        self.projected_candidate_burn_basis_points_per_hour =
            Some(per_connection_burn_basis_points_per_hour);
        self.burn_rate_confidence = QuotaRunRateConfidence::Normal;
        self
    }

    /// Sets aggregate fallback burn rate in basis points per hour.
    #[must_use]
    pub const fn with_aggregate_burn_basis_points_per_hour(
        mut self,
        aggregate_burn_basis_points_per_hour: u32,
    ) -> Self {
        self.aggregate_burn_basis_points_per_hour = Some(aggregate_burn_basis_points_per_hour);
        self.projected_candidate_burn_basis_points_per_hour =
            Some(aggregate_burn_basis_points_per_hour);
        self.burn_rate_confidence = QuotaRunRateConfidence::Normal;
        self
    }

    /// Sets projected candidate burn rate after adding the next session.
    #[must_use]
    pub const fn with_projected_candidate_burn_basis_points_per_hour(
        mut self,
        projected_candidate_burn_basis_points_per_hour: u32,
    ) -> Self {
        self.projected_candidate_burn_basis_points_per_hour =
            Some(projected_candidate_burn_basis_points_per_hour);
        self
    }

    /// Sets burn-rate confidence.
    #[must_use]
    pub const fn with_burn_rate_confidence(
        mut self,
        burn_rate_confidence: QuotaRunRateConfidence,
    ) -> Self {
        self.burn_rate_confidence = burn_rate_confidence;
        self
    }

    /// Returns window seconds.
    #[must_use]
    pub const fn window_seconds(&self) -> u64 {
        self.window_seconds
    }

    /// Returns observed per-connection burn rate in basis points per hour.
    #[must_use]
    pub const fn per_connection_burn_basis_points_per_hour(&self) -> Option<u32> {
        self.per_connection_burn_basis_points_per_hour
    }

    /// Returns aggregate fallback burn rate in basis points per hour.
    #[must_use]
    pub const fn aggregate_burn_basis_points_per_hour(&self) -> Option<u32> {
        self.aggregate_burn_basis_points_per_hour
    }

    /// Returns projected candidate burn rate in basis points per hour.
    #[must_use]
    pub const fn projected_candidate_burn_basis_points_per_hour(&self) -> Option<u32> {
        self.projected_candidate_burn_basis_points_per_hour
    }

    /// Returns burn-rate confidence.
    #[must_use]
    pub const fn burn_rate_confidence(&self) -> QuotaRunRateConfidence {
        self.burn_rate_confidence
    }
}

/// Quota window status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaWindowStatus {
    /// Window can be used for account selection.
    Eligible,
    /// Window is stale but may be used conservatively.
    Stale,
    /// Window state is unknown and needs background probe.
    Unknown,
    /// Window must not be used for selection.
    Ineligible,
}

/// Fixed v1 route-band policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BurnDownRouteBandPolicy {
    short_window_cutoff_seconds: u64,
    reserve_pressure_threshold: u32,
    reserve_headroom_threshold: u32,
    long_pressure_multiplier: u32,
    short_salvage_cap: u32,
    long_salvage_cap: u32,
    risk_penalty_cap: u32,
    selectable_weight_min: u32,
    selectable_weight_max: u32,
}

impl Default for BurnDownRouteBandPolicy {
    fn default() -> Self {
        Self {
            short_window_cutoff_seconds: DEFAULT_SHORT_WINDOW_CUTOFF_SECONDS,
            reserve_pressure_threshold: DEFAULT_RESERVE_PRESSURE_THRESHOLD,
            reserve_headroom_threshold: DEFAULT_RESERVE_HEADROOM_THRESHOLD,
            long_pressure_multiplier: DEFAULT_LONG_PRESSURE_MULTIPLIER,
            short_salvage_cap: DEFAULT_SHORT_SALVAGE_CAP,
            long_salvage_cap: DEFAULT_LONG_SALVAGE_CAP,
            risk_penalty_cap: DEFAULT_RISK_PENALTY_CAP,
            selectable_weight_min: DEFAULT_SELECTABLE_WEIGHT_MIN,
            selectable_weight_max: DEFAULT_SELECTABLE_WEIGHT_MAX,
        }
    }
}

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
    route_band: RouteBand,
    route_status: RouteBandAssessmentStatus,
    accounts: Vec<BurnDownAccountAssessment>,
    selected_pool: SelectedPool,
    weighted_candidates: Vec<(AccountId, u32)>,
    preferred_next: Option<AccountId>,
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
    account_id: AccountId,
    account_label: String,
    availability: AccountAvailability,
    freshness: QuotaEvidenceFreshness,
    routing_exclusion: RoutingExclusion,
    limiting_window: Option<LimitingWindow>,
    quota_evidence_reason: QuotaEvidenceReason,
    short_pressure: u32,
    long_pressure: u32,
    short_salvage: u32,
    long_salvage: u32,
    projected_burn_pressure: u32,
    routing_weight: Option<u32>,
    routing_reason: RoutingReason,
    preferred_next: bool,
    near_zero_retirement_candidate: bool,
    current_active_sessions: u32,
    weekly_reset_unix_seconds: Option<u64>,
    weekly_projected_exhaustion_unix_seconds: Option<u64>,
    weekly_survives_to_reset: bool,
    weekly_survival_margin_basis_points: Option<i64>,
    weekly_burn_rate_confidence: QuotaRunRateConfidence,
    weekly_in_drain_pool: bool,
    required_active_connections_to_drain: Option<u32>,
    projected_drain_gap_after_selection: Option<i64>,
    projected_weekly_runway_seconds: Option<u64>,
    salvage_sort_key: Option<SalvageSortKey>,
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
}

/// Account availability class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountAvailability {
    /// Selectable in the normal pool.
    Usable,
    /// Selectable only when no usable account exists.
    Reserve,
    /// Not selectable for new work because remaining quota is close to zero.
    Retiring,
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

/// Non-quota routing exclusion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutingExclusion {
    /// No non-quota exclusion applies.
    None,
    /// Account is disabled.
    Disabled,
    /// Account lacks active credentials.
    MissingCredential,
}

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
}

/// Public routing reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutingReason {
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
    /// Existing work may finish, but new work should not start here.
    RetiringNearZero,
    /// Excluded because the account is disabled.
    ExcludedDisabled,
    /// Excluded because the account has no active credential.
    ExcludedMissingCredential,
    /// Blocked because quota is exhausted.
    BlockedWindowExhausted,
    /// Blocked because quota is ineligible.
    BlockedWindowIneligible,
}

impl RoutingReason {
    /// Returns the stable machine code for this public routing reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreferredNearResetDrainable => "preferred_near_reset_drainable",
            Self::PreferredNearResetControlledDrain => "preferred_near_reset_controlled_drain",
            Self::PreferredWeeklyHealthier => "preferred_weekly_healthier",
            Self::PreferredWeeklyResetSoon => "preferred_weekly_reset_soon",
            Self::PreferredShortResetSoon => "preferred_short_reset_soon",
            Self::PreferredProjectedBurn => "preferred_projected_burn",
            Self::PreferredSafestQuota => "preferred_safest_quota",
            Self::AvailableSamePool => "available_same_pool",
            Self::HeldReserve => "held_reserve",
            Self::HeldUnknown => "held_unknown",
            Self::UnknownFallbackPreferred => "unknown_fallback_preferred",
            Self::UnknownFallbackAvailable => "unknown_fallback_available",
            Self::RetiringNearZero => "retiring_near_zero",
            Self::ExcludedDisabled => "excluded_disabled",
            Self::ExcludedMissingCredential => "excluded_missing_credential",
            Self::BlockedWindowExhausted => "blocked_window_exhausted",
            Self::BlockedWindowIneligible => "blocked_window_ineligible",
            Self::HeldShortWindowGuard => "held_short_window_guard",
        }
    }

    /// Returns the stable human phrase for this public routing reason.
    #[must_use]
    pub const fn human_phrase(self) -> &'static str {
        match self {
            Self::PreferredNearResetDrainable => "preferred next: near-reset drainable",
            Self::PreferredNearResetControlledDrain => {
                "preferred next: near-reset controlled drain"
            }
            Self::PreferredWeeklyHealthier => "preferred next: weekly healthier",
            Self::PreferredWeeklyResetSoon => "preferred next: weekly reset soon",
            Self::PreferredShortResetSoon => "preferred next: 5h reset soon",
            Self::PreferredProjectedBurn => "preferred next: projected burn",
            Self::PreferredSafestQuota => "preferred next: safest quota",
            Self::AvailableSamePool => "available: same pool",
            Self::HeldReserve => "held: far-reset reserve",
            Self::HeldUnknown => "held: needs refresh",
            Self::UnknownFallbackPreferred => "fallback: needs refresh",
            Self::UnknownFallbackAvailable => "fallback: same unknown pool",
            Self::RetiringNearZero => "retiring: near zero quota",
            Self::ExcludedDisabled => "blocked: disabled",
            Self::ExcludedMissingCredential => "blocked: missing credential",
            Self::BlockedWindowExhausted => "blocked: quota empty",
            Self::BlockedWindowIneligible => "blocked: quota ineligible",
            Self::HeldShortWindowGuard => "held: 5h guard",
        }
    }
}

/// Limiting window explanation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LimitingWindow {
    window_seconds: u64,
    remaining_headroom: u32,
    pressure: u32,
    reset_unix_seconds: Option<u64>,
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
struct SalvageSortKey {
    reset_unix_seconds: u64,
    window_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowAssessment {
    window_seconds: u64,
    remaining_headroom: u32,
    reset_unix_seconds: Option<u64>,
    status: QuotaWindowStatus,
    pressure: u32,
    projected_pressure: u32,
    projected_exhaustion_unix_seconds: Option<u64>,
    surplus: u32,
    time_left_seconds: Option<u64>,
    near_reset: bool,
    per_connection_burn_basis_points_per_hour: Option<u32>,
    aggregate_burn_basis_points_per_hour: Option<u32>,
    projected_candidate_burn_basis_points_per_hour: Option<u32>,
    burn_rate_confidence: QuotaRunRateConfidence,
    survival_margin_basis_points: Option<i64>,
}

/// Assesses a route band.
#[must_use]
pub fn assess_route_band(
    input: BurnDownRouteBandAssessmentInput,
) -> BurnDownRouteBandAssessmentResult {
    let mut accounts = input
        .accounts
        .iter()
        .map(|account| assess_account(account, input.now_unix_seconds, input.policy))
        .collect::<Vec<_>>();
    accounts.sort_by(|left, right| left.account_id.cmp(&right.account_id));
    apply_near_zero_retirement(&mut accounts);

    let selected_pool = if accounts
        .iter()
        .any(|account| account.availability == AccountAvailability::Usable)
    {
        SelectedPool::Usable
    } else if accounts
        .iter()
        .any(|account| account.availability == AccountAvailability::Reserve)
    {
        SelectedPool::Reserve
    } else if accounts
        .iter()
        .any(|account| account.availability == AccountAvailability::Unknown)
    {
        SelectedPool::Unknown
    } else {
        SelectedPool::None
    };

    let has_fresh_account_in_selected_pool = accounts.iter().any(|account| {
        selected_pool_matches(selected_pool, account.availability)
            && account.freshness == QuotaEvidenceFreshness::Fresh
    });

    for account in &mut accounts {
        if selected_pool_matches(selected_pool, account.availability)
            && let Some(weight) = account.routing_weight
        {
            let weight = selected_pool_weight(
                weight,
                account.freshness,
                has_fresh_account_in_selected_pool,
                input.policy,
            );
            account.routing_weight = Some(weight);
        }
    }

    let mut candidate_accounts = accounts
        .iter()
        .filter(|account| selected_pool_matches(selected_pool, account.availability))
        .filter_map(|account| account.routing_weight.map(|weight| (account, weight)))
        .collect::<Vec<_>>();

    candidate_accounts.sort_by(|(left, left_weight), (right, right_weight)| {
        candidate_priority_cmp(left, *left_weight, right, *right_weight)
    });

    let weighted_candidates = candidate_accounts
        .iter()
        .map(|(account, weight)| (account.account_id.clone(), *weight))
        .collect::<Vec<_>>();
    let preferred_next = weighted_candidates
        .first()
        .map(|(account_id, _weight)| account_id.clone());
    if let Some(preferred_next) = &preferred_next {
        for account in &mut accounts {
            account.preferred_next = &account.account_id == preferred_next;
        }
    }
    let reason_context = RoutingReasonContext::from_accounts(&accounts, selected_pool);
    for account in &mut accounts {
        account.routing_reason = routing_reason_for_account(account, reason_context);
    }

    BurnDownRouteBandAssessmentResult {
        route_band: input.route_band,
        route_status: RouteBandAssessmentStatus::Supported,
        accounts,
        selected_pool,
        weighted_candidates,
        preferred_next,
    }
}

fn assess_account(
    input: &BurnDownAccountInput,
    now_unix_seconds: u64,
    policy: BurnDownRouteBandPolicy,
) -> BurnDownAccountAssessment {
    let base = BurnDownAccountAssessment {
        account_id: input.account_id.clone(),
        account_label: safe_account_label(&input.account_label, &input.account_id)
            .as_str()
            .to_owned(),
        availability: AccountAvailability::Unknown,
        freshness: QuotaEvidenceFreshness::Unknown,
        routing_exclusion: RoutingExclusion::None,
        limiting_window: None,
        quota_evidence_reason: QuotaEvidenceReason::NeedsQuotaProbe,
        short_pressure: 0,
        long_pressure: 0,
        short_salvage: 0,
        long_salvage: 0,
        projected_burn_pressure: 0,
        routing_weight: Some(DEFAULT_UNKNOWN_FALLBACK_WEIGHT),
        routing_reason: RoutingReason::UnknownFallbackAvailable,
        preferred_next: false,
        near_zero_retirement_candidate: false,
        current_active_sessions: input.current_active_sessions,
        weekly_reset_unix_seconds: None,
        weekly_projected_exhaustion_unix_seconds: None,
        weekly_survives_to_reset: false,
        weekly_survival_margin_basis_points: None,
        weekly_burn_rate_confidence: QuotaRunRateConfidence::Unknown,
        weekly_in_drain_pool: false,
        required_active_connections_to_drain: None,
        projected_drain_gap_after_selection: None,
        projected_weekly_runway_seconds: None,
        salvage_sort_key: None,
    };

    if !input.account_enabled {
        return BurnDownAccountAssessment {
            availability: AccountAvailability::Excluded,
            routing_exclusion: RoutingExclusion::Disabled,
            quota_evidence_reason: QuotaEvidenceReason::AccountDisabled,
            routing_reason: RoutingReason::ExcludedDisabled,
            routing_weight: None,
            ..base
        };
    }
    if !input.has_active_credential {
        return BurnDownAccountAssessment {
            availability: AccountAvailability::Excluded,
            routing_exclusion: RoutingExclusion::MissingCredential,
            quota_evidence_reason: QuotaEvidenceReason::MissingCredential,
            routing_reason: RoutingReason::ExcludedMissingCredential,
            routing_weight: None,
            ..base
        };
    }

    let windows = input
        .windows
        .iter()
        .map(|window| assess_window(window, now_unix_seconds, policy))
        .collect::<Vec<_>>();
    if windows.is_empty() {
        return base;
    }
    if missing_expected_v1_window(&windows) {
        return BurnDownAccountAssessment {
            limiting_window: limiting_window(&windows),
            quota_evidence_reason: QuotaEvidenceReason::MissingExpectedWindow,
            ..base
        };
    }
    if windows
        .iter()
        .any(|window| window.status == QuotaWindowStatus::Ineligible)
    {
        return BurnDownAccountAssessment {
            availability: AccountAvailability::Blocked,
            freshness: freshness_for_windows(&windows),
            limiting_window: limiting_window(&windows),
            quota_evidence_reason: QuotaEvidenceReason::WindowIneligible,
            routing_reason: RoutingReason::BlockedWindowIneligible,
            routing_weight: None,
            ..base
        };
    }
    if windows
        .iter()
        .any(|window| window.status == QuotaWindowStatus::Unknown)
    {
        return BurnDownAccountAssessment {
            limiting_window: limiting_window(&windows),
            quota_evidence_reason: QuotaEvidenceReason::UnknownQuotaWindow,
            ..base
        };
    }
    if windows.iter().any(|window| window.remaining_headroom == 0) {
        return BurnDownAccountAssessment {
            availability: AccountAvailability::Blocked,
            freshness: freshness_for_windows(&windows),
            limiting_window: limiting_window(&windows),
            quota_evidence_reason: QuotaEvidenceReason::WindowExhausted,
            routing_reason: RoutingReason::BlockedWindowExhausted,
            routing_weight: None,
            ..base
        };
    }
    if windows
        .iter()
        .any(|window| window.reset_unix_seconds.is_none())
    {
        return BurnDownAccountAssessment {
            limiting_window: limiting_window(&windows),
            quota_evidence_reason: QuotaEvidenceReason::MissingResetTime,
            ..base
        };
    }

    let short_pressure = windows
        .iter()
        .filter(|window| is_short_window(window.window_seconds, policy))
        .map(|window| window.pressure)
        .max()
        .unwrap_or(0);
    let long_pressure = windows
        .iter()
        .filter(|window| !is_short_window(window.window_seconds, policy))
        .map(|window| window.pressure)
        .max()
        .unwrap_or(0);
    let short_salvage = windows
        .iter()
        .filter(|window| is_short_window(window.window_seconds, policy) && window.near_reset)
        .map(|window| window.surplus)
        .max()
        .unwrap_or(0)
        .min(policy.short_salvage_cap);
    let long_salvage = windows
        .iter()
        .filter(|window| !is_short_window(window.window_seconds, policy) && window.near_reset)
        .map(|window| window.surplus)
        .max()
        .unwrap_or(0)
        .min(policy.long_salvage_cap);
    let usable_headroom = windows
        .iter()
        .map(|window| window.remaining_headroom)
        .min()
        .unwrap_or(0);
    let risk_penalty = policy.risk_penalty_cap.min(
        policy
            .long_pressure_multiplier
            .saturating_mul(long_pressure)
            .saturating_add(short_pressure),
    );
    let projected_burn_pressure = windows
        .iter()
        .map(|window| window.projected_pressure)
        .max()
        .unwrap_or(0)
        .min(100);
    let risk_adjusted_weight = i64::from(usable_headroom) - i64::from(risk_penalty)
        + i64::from(short_salvage)
        + i64::from(long_salvage);
    let routing_weight = clamp_i64(
        risk_adjusted_weight,
        policy.selectable_weight_min,
        policy.selectable_weight_max,
    );
    let availability = if long_window_requires_reserve(&windows, policy) {
        AccountAvailability::Reserve
    } else {
        AccountAvailability::Usable
    };
    let near_zero_retirement_candidate = windows
        .iter()
        .any(|window| window_requires_near_zero_retirement(window, now_unix_seconds));
    let weekly_window = windows
        .iter()
        .find(|window| !is_short_window(window.window_seconds, policy));
    let weekly_in_drain_pool = weekly_window.is_some_and(weekly_window_is_drain_pool_candidate);
    let required_active_connections_to_drain =
        weekly_window.and_then(required_active_connections_to_drain);
    let projected_drain_gap_after_selection =
        required_active_connections_to_drain.map(|required_active_connections_to_drain| {
            i64::from(required_active_connections_to_drain)
                - i64::from(input.current_active_sessions.saturating_add(1))
        });
    let projected_weekly_runway_seconds = weekly_window.and_then(projected_runway_seconds);
    if short_window_fails_guard(&windows, policy) {
        return BurnDownAccountAssessment {
            availability: AccountAvailability::Blocked,
            freshness: freshness_for_windows(&windows),
            limiting_window: limiting_window(&windows),
            quota_evidence_reason: QuotaEvidenceReason::ShortWindowGuard,
            short_pressure,
            long_pressure,
            short_salvage,
            long_salvage,
            projected_burn_pressure,
            routing_reason: RoutingReason::HeldShortWindowGuard,
            routing_weight: None,
            near_zero_retirement_candidate,
            current_active_sessions: input.current_active_sessions,
            weekly_reset_unix_seconds: weekly_window.and_then(|window| window.reset_unix_seconds),
            weekly_projected_exhaustion_unix_seconds: weekly_window
                .and_then(|window| window.projected_exhaustion_unix_seconds),
            weekly_survives_to_reset: weekly_window.is_some_and(weekly_window_survives_to_reset),
            weekly_survival_margin_basis_points: weekly_window
                .and_then(|window| window.survival_margin_basis_points),
            weekly_burn_rate_confidence: weekly_window
                .map_or(QuotaRunRateConfidence::Unknown, |window| {
                    window.burn_rate_confidence
                }),
            weekly_in_drain_pool,
            required_active_connections_to_drain,
            projected_drain_gap_after_selection,
            projected_weekly_runway_seconds,
            salvage_sort_key: salvage_sort_key(&windows, short_salvage, long_salvage, policy),
            ..base
        };
    }

    BurnDownAccountAssessment {
        availability,
        freshness: freshness_for_windows(&windows),
        limiting_window: limiting_window(&windows),
        quota_evidence_reason: QuotaEvidenceReason::Ok,
        short_pressure,
        long_pressure,
        short_salvage,
        long_salvage,
        projected_burn_pressure,
        routing_weight: Some(routing_weight),
        near_zero_retirement_candidate,
        current_active_sessions: input.current_active_sessions,
        weekly_reset_unix_seconds: weekly_window.and_then(|window| window.reset_unix_seconds),
        weekly_projected_exhaustion_unix_seconds: weekly_window
            .and_then(|window| window.projected_exhaustion_unix_seconds),
        weekly_survives_to_reset: weekly_window.is_some_and(weekly_window_survives_to_reset),
        weekly_survival_margin_basis_points: weekly_window
            .and_then(|window| window.survival_margin_basis_points),
        weekly_burn_rate_confidence: weekly_window
            .map_or(QuotaRunRateConfidence::Unknown, |window| {
                window.burn_rate_confidence
            }),
        weekly_in_drain_pool,
        required_active_connections_to_drain,
        projected_drain_gap_after_selection,
        projected_weekly_runway_seconds,
        salvage_sort_key: salvage_sort_key(&windows, short_salvage, long_salvage, policy),
        routing_reason: RoutingReason::AvailableSamePool,
        ..base
    }
}

fn apply_near_zero_retirement(accounts: &mut [BurnDownAccountAssessment]) {
    let assessed_accounts = accounts.to_vec();
    for account in accounts.iter_mut() {
        if !account.near_zero_retirement_candidate
            || !matches!(
                account.availability,
                AccountAvailability::Usable | AccountAvailability::Reserve
            )
        {
            continue;
        }

        let has_not_worse_alternative = assessed_accounts
            .iter()
            .any(|alternative| not_worse_retirement_alternative(alternative, account));
        if has_not_worse_alternative {
            account.availability = AccountAvailability::Retiring;
            account.routing_weight = None;
            account.routing_reason = RoutingReason::RetiringNearZero;
        }
    }
}

fn not_worse_retirement_alternative(
    alternative: &BurnDownAccountAssessment,
    retirement_candidate: &BurnDownAccountAssessment,
) -> bool {
    if alternative.account_id == retirement_candidate.account_id
        || alternative.near_zero_retirement_candidate
        || !matches!(
            alternative.availability,
            AccountAvailability::Usable | AccountAvailability::Reserve
        )
    {
        return false;
    }

    let (Some(alternative_weight), Some(candidate_weight)) = (
        alternative.routing_weight,
        retirement_candidate.routing_weight,
    ) else {
        return false;
    };

    candidate_priority_cmp(
        alternative,
        alternative_weight,
        retirement_candidate,
        candidate_weight,
    ) != std::cmp::Ordering::Greater
}

fn assess_window(
    window: &QuotaWindowFact,
    now_unix_seconds: u64,
    policy: BurnDownRouteBandPolicy,
) -> WindowAssessment {
    let time_left_seconds = window.reset_unix_seconds.map(|reset_unix_seconds| {
        reset_unix_seconds
            .saturating_sub(now_unix_seconds)
            .min(window.window_seconds)
    });
    let expected_remaining_percent = time_left_seconds
        .map(|time_left_seconds| ceil_percent(time_left_seconds, window.window_seconds))
        .unwrap_or(0);
    let remaining_headroom = window.remaining_headroom.min(100);
    let baseline_pressure = expected_remaining_percent.saturating_sub(remaining_headroom);
    let projected_pressure = projected_pressure(window, now_unix_seconds);
    let pressure = baseline_pressure.max(projected_pressure);
    let surplus = remaining_headroom.saturating_sub(expected_remaining_percent);
    let near_reset = time_left_seconds.is_some_and(|time_left_seconds| {
        time_left_seconds <= near_reset_seconds(window.window_seconds, policy)
    });
    let survival_margin_basis_points = survival_margin_basis_points(window, time_left_seconds);

    WindowAssessment {
        window_seconds: window.window_seconds,
        remaining_headroom,
        reset_unix_seconds: window.reset_unix_seconds,
        status: window.status,
        pressure,
        projected_pressure,
        projected_exhaustion_unix_seconds: window.projected_exhaustion_unix_seconds,
        surplus,
        time_left_seconds,
        near_reset,
        per_connection_burn_basis_points_per_hour: window.per_connection_burn_basis_points_per_hour,
        aggregate_burn_basis_points_per_hour: window.aggregate_burn_basis_points_per_hour,
        projected_candidate_burn_basis_points_per_hour: window
            .projected_candidate_burn_basis_points_per_hour,
        burn_rate_confidence: window.burn_rate_confidence,
        survival_margin_basis_points,
    }
}

fn missing_expected_v1_window(windows: &[WindowAssessment]) -> bool {
    let has_short = windows
        .iter()
        .any(|window| window.window_seconds == V1_SHORT_WINDOW_SECONDS);
    let has_weekly = windows
        .iter()
        .any(|window| window.window_seconds == V1_WEEKLY_WINDOW_SECONDS);

    has_short != has_weekly
}

fn long_window_requires_reserve(
    windows: &[WindowAssessment],
    policy: BurnDownRouteBandPolicy,
) -> bool {
    windows
        .iter()
        .filter(|window| !is_short_window(window.window_seconds, policy))
        .any(|window| {
            !window.near_reset
                && !long_window_can_controlled_drain(window)
                && (window.pressure >= policy.reserve_pressure_threshold
                    || window.remaining_headroom <= policy.reserve_headroom_threshold)
        })
}

fn long_window_can_controlled_drain(window: &WindowAssessment) -> bool {
    if window
        .time_left_seconds
        .is_none_or(|time_left_seconds| time_left_seconds > DRAIN_POOL_RESET_HORIZON_SECONDS)
    {
        return false;
    }

    projected_runway_seconds(window)
        .is_some_and(|runway_seconds| runway_seconds >= REACTIVE_RECONNECT_MIN_RUNWAY_SECONDS)
}

fn weekly_window_is_drain_pool_candidate(window: &WindowAssessment) -> bool {
    if window.window_seconds != V1_WEEKLY_WINDOW_SECONDS {
        return false;
    }
    if window
        .time_left_seconds
        .is_none_or(|time_left_seconds| time_left_seconds > DRAIN_POOL_RESET_HORIZON_SECONDS)
    {
        return false;
    }
    if !matches!(
        window.burn_rate_confidence,
        QuotaRunRateConfidence::Normal | QuotaRunRateConfidence::Low
    ) {
        return false;
    }
    if window.per_connection_burn_basis_points_per_hour.is_none()
        && window.aggregate_burn_basis_points_per_hour.is_none()
    {
        return false;
    }

    projected_runway_seconds(window)
        .is_some_and(|runway_seconds| runway_seconds >= REACTIVE_RECONNECT_MIN_RUNWAY_SECONDS)
}

fn required_active_connections_to_drain(window: &WindowAssessment) -> Option<u32> {
    let per_connection_burn_basis_points_per_hour =
        u128::from(window.per_connection_burn_basis_points_per_hour?);
    let time_left_seconds = u128::from(window.time_left_seconds?);
    if per_connection_burn_basis_points_per_hour == 0 || time_left_seconds == 0 {
        return None;
    }

    let remaining_basis_points = u128::from(window.remaining_headroom).saturating_mul(100);
    let denominator = per_connection_burn_basis_points_per_hour.saturating_mul(time_left_seconds);
    let required_connections = remaining_basis_points
        .saturating_mul(3_600)
        .div_ceil(denominator);

    Some(clamp_u128_to_u32(required_connections))
}

fn projected_runway_seconds(window: &WindowAssessment) -> Option<u64> {
    let reset_unix_seconds = window.reset_unix_seconds?;
    let time_left_seconds = window.time_left_seconds?;
    let now_unix_seconds = reset_unix_seconds.saturating_sub(time_left_seconds);
    Some(
        window
            .projected_exhaustion_unix_seconds?
            .saturating_sub(now_unix_seconds),
    )
}

fn short_window_fails_guard(windows: &[WindowAssessment], policy: BurnDownRouteBandPolicy) -> bool {
    windows
        .iter()
        .filter(|window| is_short_window(window.window_seconds, policy))
        .any(short_window_fails_survival_guard)
}

fn short_window_fails_survival_guard(window: &WindowAssessment) -> bool {
    if let Some(survival_margin_basis_points) = window.survival_margin_basis_points {
        return survival_margin_basis_points < SHORT_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS;
    }

    match (
        window.projected_exhaustion_unix_seconds,
        window.reset_unix_seconds,
    ) {
        (Some(projected_exhaustion_unix_seconds), Some(reset_unix_seconds)) => {
            projected_exhaustion_unix_seconds < reset_unix_seconds && !window.near_reset
        }
        _ => false,
    }
}

fn freshness_for_windows(windows: &[WindowAssessment]) -> QuotaEvidenceFreshness {
    if windows
        .iter()
        .any(|window| window.status == QuotaWindowStatus::Unknown)
    {
        return QuotaEvidenceFreshness::Unknown;
    }
    if windows
        .iter()
        .any(|window| window.status == QuotaWindowStatus::Stale)
    {
        return QuotaEvidenceFreshness::Stale;
    }

    QuotaEvidenceFreshness::Fresh
}

fn limiting_window(windows: &[WindowAssessment]) -> Option<LimitingWindow> {
    windows
        .iter()
        .max_by(|left, right| {
            left.pressure
                .cmp(&right.pressure)
                .then_with(|| right.remaining_headroom.cmp(&left.remaining_headroom))
                .then_with(|| left.window_seconds.cmp(&right.window_seconds))
        })
        .map(|window| LimitingWindow {
            window_seconds: window.window_seconds,
            remaining_headroom: window.remaining_headroom,
            pressure: window.pressure,
            reset_unix_seconds: window.reset_unix_seconds,
        })
}

fn salvage_sort_key(
    windows: &[WindowAssessment],
    short_salvage: u32,
    long_salvage: u32,
    policy: BurnDownRouteBandPolicy,
) -> Option<SalvageSortKey> {
    if short_salvage.saturating_add(long_salvage) == 0 {
        return None;
    }

    windows
        .iter()
        .filter(|window| window.near_reset && window.surplus > 0)
        .filter(|window| {
            if is_short_window(window.window_seconds, policy) {
                short_salvage > 0
            } else {
                long_salvage > 0
            }
        })
        .filter_map(|window| {
            window
                .reset_unix_seconds
                .map(|reset_unix_seconds| SalvageSortKey {
                    reset_unix_seconds,
                    window_seconds: window.window_seconds,
                })
        })
        .min()
}

fn selected_pool_matches(selected_pool: SelectedPool, availability: AccountAvailability) -> bool {
    matches!(
        (selected_pool, availability),
        (SelectedPool::Usable, AccountAvailability::Usable)
            | (SelectedPool::Reserve, AccountAvailability::Reserve)
            | (SelectedPool::Unknown, AccountAvailability::Unknown)
    )
}

fn window_requires_near_zero_retirement(window: &WindowAssessment, now_unix_seconds: u64) -> bool {
    if window.window_seconds == V1_WEEKLY_WINDOW_SECONDS && long_window_can_controlled_drain(window)
    {
        return false;
    }

    if window.remaining_headroom < DEFAULT_NEAR_ZERO_HEADROOM_THRESHOLD {
        return true;
    }

    window
        .projected_exhaustion_unix_seconds
        .is_some_and(|projected_exhaustion_unix_seconds| {
            if window.reset_unix_seconds.is_some_and(|reset_unix_seconds| {
                projected_exhaustion_unix_seconds >= reset_unix_seconds
            }) {
                return false;
            }
            projected_exhaustion_unix_seconds
                <= now_unix_seconds.saturating_add(DEFAULT_NEAR_ZERO_PROJECTED_RUNOUT_SECONDS)
        })
}

fn selected_pool_weight(
    weight: u32,
    freshness: QuotaEvidenceFreshness,
    has_fresh_account_in_selected_pool: bool,
    policy: BurnDownRouteBandPolicy,
) -> u32 {
    let adjusted =
        if freshness == QuotaEvidenceFreshness::Stale && has_fresh_account_in_selected_pool {
            weight / 4
        } else {
            weight
        };

    clamp_u32(
        adjusted,
        policy.selectable_weight_min,
        policy.selectable_weight_max,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RoutingReasonContext {
    selected_pool: SelectedPool,
    preferred_long_pressure: u32,
    preferred_projected_burn_pressure: u32,
    has_worse_known_selected_pool_long_pressure: bool,
    has_worse_known_selected_pool_projected_burn_pressure: bool,
    has_held_reserve_account: bool,
}

impl RoutingReasonContext {
    fn from_accounts(accounts: &[BurnDownAccountAssessment], selected_pool: SelectedPool) -> Self {
        let preferred_long_pressure = accounts
            .iter()
            .find(|account| account.preferred_next)
            .map_or(0, |account| account.long_pressure);
        let preferred_projected_burn_pressure = accounts
            .iter()
            .find(|account| account.preferred_next)
            .map_or(0, |account| account.projected_burn_pressure);
        let has_worse_known_selected_pool_long_pressure = accounts.iter().any(|account| {
            selected_pool_matches(selected_pool, account.availability)
                && matches!(
                    account.availability,
                    AccountAvailability::Usable | AccountAvailability::Reserve
                )
                && !account.preferred_next
                && account.long_pressure > preferred_long_pressure
        });
        let has_worse_known_selected_pool_projected_burn_pressure =
            accounts.iter().any(|account| {
                selected_pool_matches(selected_pool, account.availability)
                    && matches!(
                        account.availability,
                        AccountAvailability::Usable | AccountAvailability::Reserve
                    )
                    && !account.preferred_next
                    && account.projected_burn_pressure > preferred_projected_burn_pressure
            });
        let has_held_reserve_account = selected_pool == SelectedPool::Usable
            && accounts
                .iter()
                .any(|account| account.availability == AccountAvailability::Reserve);

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

fn routing_reason_for_account(
    account: &BurnDownAccountAssessment,
    context: RoutingReasonContext,
) -> RoutingReason {
    match account.routing_exclusion {
        RoutingExclusion::Disabled => return RoutingReason::ExcludedDisabled,
        RoutingExclusion::MissingCredential => return RoutingReason::ExcludedMissingCredential,
        RoutingExclusion::None => {}
    }

    match account.quota_evidence_reason {
        QuotaEvidenceReason::WindowExhausted => return RoutingReason::BlockedWindowExhausted,
        QuotaEvidenceReason::WindowIneligible => return RoutingReason::BlockedWindowIneligible,
        QuotaEvidenceReason::ShortWindowGuard => return RoutingReason::HeldShortWindowGuard,
        QuotaEvidenceReason::Ok
        | QuotaEvidenceReason::NeedsQuotaProbe
        | QuotaEvidenceReason::MissingExpectedWindow
        | QuotaEvidenceReason::UnknownQuotaWindow
        | QuotaEvidenceReason::MissingResetTime
        | QuotaEvidenceReason::AccountDisabled
        | QuotaEvidenceReason::MissingCredential => {}
    }

    match account.availability {
        AccountAvailability::Unknown if context.selected_pool != SelectedPool::Unknown => {
            return RoutingReason::HeldUnknown;
        }
        AccountAvailability::Reserve if context.selected_pool == SelectedPool::Usable => {
            return RoutingReason::HeldReserve;
        }
        AccountAvailability::Retiring => return RoutingReason::RetiringNearZero,
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

    if account.weekly_in_drain_pool {
        if account
            .projected_drain_gap_after_selection
            .is_some_and(|gap| gap > 0)
        {
            return RoutingReason::PreferredNearResetDrainable;
        }

        return RoutingReason::PreferredNearResetControlledDrain;
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

fn projected_pressure(window: &QuotaWindowFact, now_unix_seconds: u64) -> u32 {
    let Some(projected_exhaustion_unix_seconds) = window.projected_exhaustion_unix_seconds else {
        return 0;
    };
    let Some(reset_unix_seconds) = window.reset_unix_seconds else {
        return 0;
    };
    if projected_exhaustion_unix_seconds <= now_unix_seconds
        || projected_exhaustion_unix_seconds >= reset_unix_seconds
    {
        return 0;
    }

    ceil_percent(
        reset_unix_seconds.saturating_sub(projected_exhaustion_unix_seconds),
        window.window_seconds,
    )
}

fn compare_salvage_key(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    match (left.salvage_sort_key, right.salvage_sort_key) {
        (Some(left_key), Some(right_key)) => left_key.cmp(&right_key),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn candidate_priority_cmp(
    left: &BurnDownAccountAssessment,
    left_weight: u32,
    right: &BurnDownAccountAssessment,
    right_weight: u32,
) -> std::cmp::Ordering {
    compare_weekly_drain_pool(left, right)
        .then_with(|| compare_drain_pool_confidence(left, right))
        .then_with(|| compare_projected_drain_gap(left, right))
        .then_with(|| compare_weekly_survival(left, right))
        .then_with(|| right_weight.cmp(&left_weight))
        .then_with(|| left.long_pressure.cmp(&right.long_pressure))
        .then_with(|| left.short_pressure.cmp(&right.short_pressure))
        .then_with(|| compare_salvage_key(left, right))
        .then_with(|| left.account_id.cmp(&right.account_id))
}

fn compare_weekly_drain_pool(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    match (left.weekly_in_drain_pool, right.weekly_in_drain_pool) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    }
}

fn compare_projected_drain_gap(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    if !(left.weekly_in_drain_pool && right.weekly_in_drain_pool) {
        return std::cmp::Ordering::Equal;
    }

    match (
        left.projected_drain_gap_after_selection,
        right.projected_drain_gap_after_selection,
    ) {
        (Some(left_gap), Some(right_gap)) if left_gap > 0 && right_gap > 0 => {
            right_gap.cmp(&left_gap)
        }
        (Some(left_gap), Some(right_gap)) if left_gap > 0 || right_gap > 0 => {
            right_gap.max(0).cmp(&left_gap.max(0))
        }
        (Some(_), Some(_)) => std::cmp::Ordering::Equal,
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn compare_drain_pool_confidence(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    if !(left.weekly_in_drain_pool && right.weekly_in_drain_pool) {
        return std::cmp::Ordering::Equal;
    }

    confidence_rank(right.weekly_burn_rate_confidence)
        .cmp(&confidence_rank(left.weekly_burn_rate_confidence))
}

fn compare_weekly_survival(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    match (
        left.weekly_survives_to_reset,
        right.weekly_survives_to_reset,
    ) {
        (true, true) => compare_surviving_weekly_accounts(left, right),
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => compare_weekly_non_survivors(left, right),
    }
}

fn compare_weekly_non_survivors(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    compare_material_projected_weekly_runout(left, right)
        .then_with(|| compare_same_pool_active_imbalance(left, right))
        .then_with(|| compare_latest_projected_weekly_runout(left, right))
        .then_with(|| compare_weekly_survival_margin(left, right))
        .then_with(|| {
            confidence_rank(right.weekly_burn_rate_confidence)
                .cmp(&confidence_rank(left.weekly_burn_rate_confidence))
        })
        .then_with(|| {
            left.current_active_sessions
                .cmp(&right.current_active_sessions)
        })
}

fn compare_material_projected_weekly_runout(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    if same_effective_weekly_pool(left, right)
        && let (Some(left_runout), Some(right_runout)) = (
            left.weekly_projected_exhaustion_unix_seconds,
            right.weekly_projected_exhaustion_unix_seconds,
        )
        && left_runout.abs_diff(right_runout) <= SAME_POOL_PROJECTED_RUNOUT_TOLERANCE_SECONDS
    {
        return std::cmp::Ordering::Equal;
    }

    compare_latest_projected_weekly_runout(left, right)
}

fn compare_latest_projected_weekly_runout(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    match (
        left.weekly_projected_exhaustion_unix_seconds,
        right.weekly_projected_exhaustion_unix_seconds,
    ) {
        (Some(left_runout), Some(right_runout)) => right_runout.cmp(&left_runout),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn compare_surviving_weekly_accounts(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    confidence_rank(right.weekly_burn_rate_confidence)
        .cmp(&confidence_rank(left.weekly_burn_rate_confidence))
        .then_with(|| compare_same_pool_active_imbalance(left, right))
        .then_with(|| {
            left.weekly_reset_unix_seconds
                .unwrap_or(u64::MAX)
                .cmp(&right.weekly_reset_unix_seconds.unwrap_or(u64::MAX))
        })
        .then_with(|| compare_weekly_survival_margin(left, right))
        .then_with(|| compare_known_margin_active_count(left, right))
}

fn compare_known_margin_active_count(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    if left.weekly_survival_margin_basis_points.is_none()
        || right.weekly_survival_margin_basis_points.is_none()
    {
        return std::cmp::Ordering::Equal;
    }

    left.current_active_sessions
        .cmp(&right.current_active_sessions)
}

fn compare_weekly_survival_margin(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    match (
        left.weekly_survival_margin_basis_points,
        right.weekly_survival_margin_basis_points,
    ) {
        (Some(left_margin), Some(right_margin)) => right_margin.cmp(&left_margin),
        _ => std::cmp::Ordering::Equal,
    }
}

fn compare_same_pool_active_imbalance(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    if !same_effective_weekly_pool(left, right) {
        return std::cmp::Ordering::Equal;
    }

    let active_delta = left
        .current_active_sessions
        .abs_diff(right.current_active_sessions);
    if active_delta == 0 {
        return std::cmp::Ordering::Equal;
    }
    if active_delta < ACTIVE_SESSION_IMBALANCE_THRESHOLD {
        return std::cmp::Ordering::Equal;
    }

    left.current_active_sessions
        .cmp(&right.current_active_sessions)
}

fn same_effective_weekly_pool(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> bool {
    if left.weekly_burn_rate_confidence != right.weekly_burn_rate_confidence {
        return false;
    }
    let Some(left_reset) = left.weekly_reset_unix_seconds else {
        return false;
    };
    let Some(right_reset) = right.weekly_reset_unix_seconds else {
        return false;
    };
    if left_reset.abs_diff(right_reset) > SAME_POOL_RESET_TOLERANCE_SECONDS {
        return false;
    }
    if left.weekly_in_drain_pool
        && right.weekly_in_drain_pool
        && !left.weekly_survives_to_reset
        && !right.weekly_survives_to_reset
        && let (Some(left_runout), Some(right_runout)) = (
            left.weekly_projected_exhaustion_unix_seconds,
            right.weekly_projected_exhaustion_unix_seconds,
        )
    {
        return left_runout.abs_diff(right_runout) <= SAME_POOL_PROJECTED_RUNOUT_TOLERANCE_SECONDS;
    }

    match (
        left.weekly_survival_margin_basis_points,
        right.weekly_survival_margin_basis_points,
    ) {
        (Some(left_margin), Some(right_margin)) => {
            left_margin.abs_diff(right_margin)
                <= SAME_POOL_SURVIVAL_MARGIN_TOLERANCE_BASIS_POINTS as u64
        }
        (None, None) => left.long_pressure == right.long_pressure,
        _ => false,
    }
}

const fn confidence_rank(confidence: QuotaRunRateConfidence) -> u8 {
    match confidence {
        QuotaRunRateConfidence::Normal => 4,
        QuotaRunRateConfidence::Low => 3,
        QuotaRunRateConfidence::Insufficient => 2,
        QuotaRunRateConfidence::Unknown => 1,
        QuotaRunRateConfidence::Stale => 0,
    }
}

fn weekly_window_survives_to_reset(window: &WindowAssessment) -> bool {
    if let Some(survival_margin_basis_points) = window.survival_margin_basis_points {
        return survival_margin_basis_points >= WEEKLY_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS;
    }

    match (
        window.projected_exhaustion_unix_seconds,
        window.reset_unix_seconds,
    ) {
        (Some(projected_exhaustion_unix_seconds), Some(reset_unix_seconds)) => {
            projected_exhaustion_unix_seconds >= reset_unix_seconds
        }
        (None, Some(_)) => true,
        _ => false,
    }
}

fn survival_margin_basis_points(
    window: &QuotaWindowFact,
    time_left_seconds: Option<u64>,
) -> Option<i64> {
    let burn_rate_basis_points_per_hour = u128::from(
        window
            .projected_candidate_burn_basis_points_per_hour
            .or(window.per_connection_burn_basis_points_per_hour)
            .or(window.aggregate_burn_basis_points_per_hour)?,
    );
    let time_left_seconds = u128::from(time_left_seconds?);
    let projected_burn_basis_points = burn_rate_basis_points_per_hour
        .saturating_mul(time_left_seconds)
        .div_ceil(3_600);
    let remaining_basis_points = i128::from(window.remaining_headroom) * 100;
    let margin = remaining_basis_points - i128::try_from(projected_burn_basis_points).ok()?;

    Some(margin.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64)
}

const fn is_short_window(window_seconds: u64, policy: BurnDownRouteBandPolicy) -> bool {
    window_seconds < policy.short_window_cutoff_seconds
}

const fn near_reset_seconds(window_seconds: u64, policy: BurnDownRouteBandPolicy) -> u64 {
    let tenth = window_seconds / 10;
    if is_short_window(window_seconds, policy) {
        min_u64(SHORT_NEAR_RESET_THRESHOLD_SECONDS, tenth)
    } else {
        min_u64(DEFAULT_LONG_NEAR_RESET_MAX_SECONDS, tenth)
    }
}

fn ceil_percent(numerator: u64, denominator: u64) -> u32 {
    if denominator == 0 {
        return 0;
    }
    let scaled = u128::from(numerator) * 100;
    scaled.div_ceil(u128::from(denominator)) as u32
}

const fn clamp_i64(value: i64, min: u32, max: u32) -> u32 {
    if value < min as i64 {
        min
    } else if value > max as i64 {
        max
    } else {
        value as u32
    }
}

const fn clamp_u32(value: u32, min: u32, max: u32) -> u32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

fn clamp_u128_to_u32(value: u128) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

const fn min_u64(left: u64, right: u64) -> u64 {
    if left < right { left } else { right }
}

const fn policy_for_route_band(_route_band: RouteBand) -> BurnDownRouteBandPolicy {
    BurnDownRouteBandPolicy {
        short_window_cutoff_seconds: DEFAULT_SHORT_WINDOW_CUTOFF_SECONDS,
        reserve_pressure_threshold: DEFAULT_RESERVE_PRESSURE_THRESHOLD,
        reserve_headroom_threshold: DEFAULT_RESERVE_HEADROOM_THRESHOLD,
        long_pressure_multiplier: DEFAULT_LONG_PRESSURE_MULTIPLIER,
        short_salvage_cap: DEFAULT_SHORT_SALVAGE_CAP,
        long_salvage_cap: DEFAULT_LONG_SALVAGE_CAP,
        risk_penalty_cap: DEFAULT_RISK_PENALTY_CAP,
        selectable_weight_min: DEFAULT_SELECTABLE_WEIGHT_MIN,
        selectable_weight_max: DEFAULT_SELECTABLE_WEIGHT_MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;
    const FIVE_HOURS: u64 = V1_SHORT_WINDOW_SECONDS;
    const WEEKLY: u64 = V1_WEEKLY_WINDOW_SECONDS;

    struct AccountSelectionScenario {
        id: &'static str,
        starts_to_simulate: usize,
        accounts: Vec<ScenarioAccountFixture>,
        expected_sequence: Vec<&'static str>,
        expected_final_active_sessions: Vec<(&'static str, u32)>,
        expected_final_account_states: Vec<ExpectedAccountState>,
        expected_selected_weekly_runouts: Option<Vec<(&'static str, Option<u64>)>>,
    }

    struct ScenarioAccountFixture {
        account_id: &'static str,
        initial_active_sessions: u32,
        build_account: fn(u32) -> BurnDownAccountInput,
    }

    struct ExpectedAccountState {
        account_id: &'static str,
        availability: AccountAvailability,
        routing_reason: RoutingReason,
    }

    #[derive(Debug)]
    struct ScenarioRunResult {
        selected_accounts: Vec<String>,
        selected_weekly_runouts: Vec<(String, Option<u64>)>,
        final_active_sessions: Vec<(String, u32)>,
        final_assessment: BurnDownRouteBandAssessmentResult,
    }

    fn run_account_selection_scenario(scenario: &AccountSelectionScenario) -> ScenarioRunResult {
        let mut active_sessions_by_account = scenario
            .accounts
            .iter()
            .map(|account| (account.account_id, account.initial_active_sessions))
            .collect::<Vec<_>>();
        let mut selected_accounts = Vec::new();
        let mut selected_weekly_runouts = Vec::new();

        for _session_start in 0..scenario.starts_to_simulate {
            let assessment =
                assess_scenario_accounts(scenario, active_sessions_by_account.as_slice());
            let selected_account = assessment
                .preferred_next()
                .unwrap_or_else(|| panic!("{} should have a quota candidate", scenario.id))
                .as_str()
                .to_owned();
            let selected_weekly_runout = account_assessment(&assessment, &selected_account)
                .weekly_projected_exhaustion_unix_seconds()
                .map(|projected_exhaustion_unix_seconds| {
                    projected_exhaustion_unix_seconds.saturating_sub(NOW)
                });
            let (_, selected_active_sessions) = active_sessions_by_account
                .iter_mut()
                .find(|(account_id, _)| *account_id == selected_account)
                .unwrap_or_else(|| {
                    panic!(
                        "{} selected account outside fixture: {}",
                        scenario.id, selected_account
                    )
                });
            *selected_active_sessions += 1;
            selected_weekly_runouts.push((selected_account.clone(), selected_weekly_runout));
            selected_accounts.push(selected_account);
        }

        let final_assessment =
            assess_scenario_accounts(scenario, active_sessions_by_account.as_slice());
        let final_active_sessions = active_sessions_by_account
            .into_iter()
            .map(|(account_id, active_sessions)| (account_id.to_owned(), active_sessions))
            .collect::<Vec<_>>();

        ScenarioRunResult {
            selected_accounts,
            selected_weekly_runouts,
            final_active_sessions,
            final_assessment,
        }
    }

    fn assess_scenario_accounts(
        scenario: &AccountSelectionScenario,
        active_sessions_by_account: &[(&'static str, u32)],
    ) -> BurnDownRouteBandAssessmentResult {
        assess_route_band(input(
            scenario
                .accounts
                .iter()
                .map(|account| {
                    let active_sessions = active_sessions_by_account
                        .iter()
                        .find(|(account_id, _)| *account_id == account.account_id)
                        .map(|(_, active_sessions)| *active_sessions)
                        .unwrap_or_else(|| {
                            panic!(
                                "{} fixture missing active counter for {}",
                                scenario.id, account.account_id
                            )
                        });
                    (account.build_account)(active_sessions)
                })
                .collect(),
        ))
    }

    fn assert_account_selection_scenario(scenario: &AccountSelectionScenario) -> ScenarioRunResult {
        let result = run_account_selection_scenario(scenario);

        assert_eq!(
            result.selected_accounts,
            scenario
                .expected_sequence
                .iter()
                .map(|account_id| (*account_id).to_owned())
                .collect::<Vec<_>>(),
            "{} selected sequence",
            scenario.id
        );
        assert_eq!(
            result.final_active_sessions,
            scenario
                .expected_final_active_sessions
                .iter()
                .map(|(account_id, active_sessions)| ((*account_id).to_owned(), *active_sessions))
                .collect::<Vec<_>>(),
            "{} final active sessions",
            scenario.id
        );
        for expected_state in &scenario.expected_final_account_states {
            let account = account_assessment(&result.final_assessment, expected_state.account_id);
            assert_eq!(
                account.availability(),
                expected_state.availability,
                "{} final availability for {}",
                scenario.id,
                expected_state.account_id
            );
            assert_eq!(
                account.routing_reason(),
                expected_state.routing_reason,
                "{} final routing reason for {}",
                scenario.id,
                expected_state.account_id
            );
        }
        if let Some(expected_selected_weekly_runouts) = &scenario.expected_selected_weekly_runouts {
            assert_eq!(
                result.selected_weekly_runouts,
                expected_selected_weekly_runouts
                    .iter()
                    .map(|(account_id, runout)| ((*account_id).to_owned(), *runout))
                    .collect::<Vec<_>>(),
                "{} selected weekly projection trace",
                scenario.id
            );
        }

        result
    }

    #[test]
    fn default_policy_constants_match_spec_r0() {
        assert_eq!(WEEKLY_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS, 200);
        assert_eq!(SHORT_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS, 100);
        assert_eq!(SHORT_NEAR_RESET_THRESHOLD_SECONDS, 1_800);
        assert_eq!(SAME_POOL_RESET_TOLERANCE_SECONDS, 7_200);
        assert_eq!(SAME_POOL_PROJECTED_RUNOUT_TOLERANCE_SECONDS, 7_200);
        assert_eq!(SAME_POOL_SURVIVAL_MARGIN_TOLERANCE_BASIS_POINTS, 500);
        assert_eq!(ACTIVE_SESSION_IMBALANCE_THRESHOLD, 1);
        assert_eq!(USAGE_LIMIT_SUSPECT_TTL_SECONDS, 300);
        assert_eq!(ACTIVE_SESSION_ROLLUP_BUCKET_SECONDS, 300);
    }

    #[test]
    fn scenario_a_uses_low_short_window_when_reset_is_near_and_weekly_is_healthy() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![window(FIVE_HOURS, 5, 120), window(WEEKLY, 80, 5 * 86_400)],
            ),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 90, 4 * 3_600),
                    window(WEEKLY, 20, 5 * 86_400),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(assessment.route_band(), RouteBand::Responses);
        assert_eq!(
            assessment.route_status(),
            RouteBandAssessmentStatus::Supported
        );
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a")
        );
        assert_eq!(assessment.weighted_candidates()[0].0.as_str(), "acct_a");
        assert_account(&assessment, "acct_a", AccountAvailability::Usable, Some(9));
        assert_account(&assessment, "acct_b", AccountAvailability::Reserve, Some(0));
        assert_eq!(
            account_assessment(&assessment, "acct_a").routing_reason(),
            RoutingReason::PreferredWeeklyHealthier
        );
        assert_eq!(
            account_assessment(&assessment, "acct_b").routing_reason(),
            RoutingReason::HeldReserve
        );
    }

    #[test]
    fn weekly_survival_prefers_soon_reset_survivor_over_far_reset_reserve_w1() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        20,
                        24 * 3_600,
                        50,
                    ),
                ],
            ),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        34,
                        96 * 3_600,
                        50,
                    ),
                ],
            ),
            account(
                "acct_c",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        80,
                        7 * 86_400,
                        50,
                    ),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a"),
            "W1: A survives its soon reset; B and C are far-reset reserve/failures"
        );
    }

    #[test]
    fn weekly_survival_prefers_earliest_reset_when_all_survive_w3() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        60,
                        48 * 3_600,
                        50,
                    ),
                ],
            ),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        70,
                        96 * 3_600,
                        50,
                    ),
                ],
            ),
            account(
                "acct_c",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        90,
                        7 * 86_400,
                        50,
                    ),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a"),
            "W3: all weekly windows survive, so the earliest reset should win"
        );
    }

    #[test]
    fn known_weekly_survivor_beats_unknown_burn_account_w4() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window(WEEKLY, 20, 24 * 3_600),
                ],
            ),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        34,
                        96 * 3_600,
                        20,
                    ),
                ],
            ),
            account(
                "acct_c",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        80,
                        7 * 86_400,
                        20,
                    ),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_b"),
            "W4: known survivor confidence beats unknown-burn soon reset"
        );
    }

    #[test]
    fn same_weekly_pool_uses_active_session_imbalance_before_far_reset_reserve_a5() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        19,
                        45 * 3_600,
                        0,
                    ),
                ],
            )
            .with_current_active_sessions(6),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        18,
                        46 * 3_600,
                        0,
                    ),
                ],
            )
            .with_current_active_sessions(0),
            account(
                "acct_c",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        34,
                        107 * 3_600,
                        20,
                    ),
                ],
            )
            .with_current_active_sessions(0),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_b"),
            "A5: same low-weekly reset pool shares sessions before far-reset reserve"
        );
    }

    #[test]
    fn confidence_tier_gates_before_active_count_tie_a6() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        40,
                        48 * 3_600,
                        50,
                    )
                    .with_burn_rate_confidence(QuotaRunRateConfidence::Normal),
                ],
            )
            .with_current_active_sessions(4),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        40,
                        48 * 3_600,
                        50,
                    )
                    .with_burn_rate_confidence(QuotaRunRateConfidence::Low),
                ],
            )
            .with_current_active_sessions(0),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a"),
            "A6: higher confidence tier gates before active-count balancing"
        );
    }

    #[test]
    fn short_window_guard_holds_account_projected_to_stall_before_reset_f1() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window_with_per_connection_burn_basis_points_per_hour(
                        FIVE_HOURS,
                        2,
                        4 * 3_600,
                        100,
                    ),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        80,
                        4 * 86_400,
                        20,
                    ),
                ],
            ),
            account(
                "acct_b",
                vec![
                    window_with_per_connection_burn_basis_points_per_hour(
                        FIVE_HOURS,
                        30,
                        4 * 3_600,
                        100,
                    ),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        40,
                        4 * 86_400,
                        20,
                    ),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_b"),
            "F1: A fails the 5h flow guard even though its weekly quota is healthier"
        );
        assert_eq!(
            account_assessment(&assessment, "acct_a").routing_reason(),
            RoutingReason::HeldShortWindowGuard
        );
        assert_account(&assessment, "acct_a", AccountAvailability::Blocked, None);
    }

    #[test]
    fn short_window_guard_allows_near_reset_within_buffer_f2() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window_with_per_connection_burn_basis_points_per_hour(
                        FIVE_HOURS,
                        2,
                        10 * 60,
                        100,
                    ),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        80,
                        4 * 86_400,
                        20,
                    ),
                ],
            ),
            account(
                "acct_b",
                vec![
                    window_with_per_connection_burn_basis_points_per_hour(
                        FIVE_HOURS,
                        30,
                        4 * 3_600,
                        100,
                    ),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        40,
                        4 * 86_400,
                        20,
                    ),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a"),
            "F2: A can remain eligible because the 5h reset is near and inside the safety buffer"
        );
    }

    #[test]
    fn weekly_non_survivor_above_reactive_floor_stays_in_drain_pool_w6() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        10,
                        20 * 3_600,
                        100,
                    ),
                ],
            ),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        20,
                        60 * 3_600,
                        67,
                    ),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a"),
            "W6: near-reset account above the reactive floor stays in the drain pool before later reset quota"
        );
        let preferred_account = account_assessment(&assessment, "acct_a");
        assert_eq!(
            preferred_account.routing_reason(),
            RoutingReason::PreferredNearResetControlledDrain,
            "W6 should explain that the chosen non-survivor remains in controlled drain"
        );
    }

    #[test]
    fn weekly_non_survivor_fallback_uses_projected_runout_before_active_count_w7() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_lasts_longer_busy",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    projected_window(WEEKLY, 10, 24 * 3_600, hours_minutes(16, 40))
                        .with_per_connection_burn_basis_points_per_hour(60),
                ],
            )
            .with_current_active_sessions(3),
            account(
                "acct_runs_out_sooner_idle",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    projected_window(WEEKLY, 9, 24 * 3_600, hours_minutes(12, 51))
                        .with_per_connection_burn_basis_points_per_hour(70),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_lasts_longer_busy"),
            "W7: when no same-pool account survives, latest projected runout beats active-count balancing"
        );
    }

    #[test]
    fn single_account_low_weekly_still_selects_when_no_alternative_exists_s3a() {
        let assessment = assess_route_band(input(vec![account(
            "acct_only",
            vec![
                window(FIVE_HOURS, 95, hours_minutes(4, 0)),
                projected_window(WEEKLY, 4, hours_minutes(23, 0), hours_minutes(5, 0))
                    .with_per_connection_burn_basis_points_per_hour(80),
            ],
        )]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_only"),
            "S3a: a single configured account should remain selectable until it is truly exhausted"
        );
    }

    #[test]
    fn two_account_soon_reset_drain_beats_far_reset_reserve_when_runway_is_safe_s3b() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_near_reset",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 8, hours_minutes(23, 30), hours_minutes(12, 0))
                        .with_per_connection_burn_basis_points_per_hour(67),
                ],
            ),
            account(
                "acct_far_reset",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 26, 84 * 3_600, 24 * 3_600)
                        .with_per_connection_burn_basis_points_per_hour(108),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_near_reset"),
            "S3b: safe near-reset quota should be drained before consuming far-reset reserve"
        );
    }

    #[test]
    fn two_account_near_reset_drain_above_reactive_floor_beats_far_reset_reserve_s3c() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_near_reset",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 8, hours_minutes(23, 30), hours_minutes(5, 30))
                        .with_per_connection_burn_basis_points_per_hour(145),
                ],
            ),
            account(
                "acct_far_reset",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 26, 84 * 3_600, 24 * 3_600)
                        .with_per_connection_burn_basis_points_per_hour(108),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_near_reset"),
            "S3c: a near-reset drain account above the reactive floor should take new starts"
        );
    }

    #[test]
    fn same_reset_drain_pool_balances_active_sessions_before_runout_tiebreak_s3d() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_busy_near_reset",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 19, hours_minutes(45, 0), hours_minutes(20, 0))
                        .with_per_connection_burn_basis_points_per_hour(95),
                ],
            )
            .with_current_active_sessions(6),
            account(
                "acct_idle_near_reset",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 18, hours_minutes(46, 0), hours_minutes(19, 0))
                        .with_per_connection_burn_basis_points_per_hour(95),
                ],
            )
            .with_current_active_sessions(0),
            account(
                "acct_far_reset",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 40, 5 * 86_400, 4 * 86_400)
                        .with_per_connection_burn_basis_points_per_hour(42),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_idle_near_reset"),
            "S3d: same reset drain pool should share active sessions before chasing a small runout edge"
        );
    }

    #[test]
    fn same_unknown_margin_pool_balances_one_active_session_s3e() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_busy",
                vec![
                    window(FIVE_HOURS, 50, hours_minutes(4, 0)),
                    window(WEEKLY, 50, 4 * 86_400),
                ],
            )
            .with_current_active_sessions(1),
            account(
                "acct_idle",
                vec![
                    window(FIVE_HOURS, 50, hours_minutes(4, 0)),
                    window(WEEKLY, 50, 4 * 86_400),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_idle"),
            "S3e: equal quota/reset accounts without burn history should still share active sessions"
        );
    }

    #[test]
    fn s3f_stale_unknown_peer_does_not_beat_known_drain_account() {
        let result = assert_account_selection_scenario(&AccountSelectionScenario {
            id: "S3f",
            starts_to_simulate: 5,
            accounts: vec![
                ScenarioAccountFixture {
                    account_id: "acct_known",
                    initial_active_sessions: 2,
                    build_account: |active_sessions| {
                        account(
                            "acct_known",
                            vec![
                                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                                projected_window(WEEKLY, 40, 24 * 3_600, 60 * 3_600)
                                    .with_burn_rate_confidence(QuotaRunRateConfidence::Normal),
                            ],
                        )
                        .with_current_active_sessions(active_sessions)
                    },
                },
                ScenarioAccountFixture {
                    account_id: "acct_stale",
                    initial_active_sessions: 0,
                    build_account: |active_sessions| {
                        account(
                            "acct_stale",
                            vec![
                                stale_window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                                stale_window(WEEKLY, 42, hours_minutes(25, 0)),
                            ],
                        )
                        .with_current_active_sessions(active_sessions)
                    },
                },
                ScenarioAccountFixture {
                    account_id: "acct_far_reset",
                    initial_active_sessions: 0,
                    build_account: |active_sessions| {
                        account(
                            "acct_far_reset",
                            vec![
                                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                                projected_window(WEEKLY, 40, 5 * 86_400, 10 * 86_400),
                            ],
                        )
                        .with_current_active_sessions(active_sessions)
                    },
                },
            ],
            expected_sequence: vec![
                "acct_known",
                "acct_known",
                "acct_known",
                "acct_known",
                "acct_known",
            ],
            expected_final_active_sessions: vec![
                ("acct_known", 7),
                ("acct_stale", 0),
                ("acct_far_reset", 0),
            ],
            expected_final_account_states: vec![
                ExpectedAccountState {
                    account_id: "acct_known",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::PreferredWeeklyHealthier,
                },
                ExpectedAccountState {
                    account_id: "acct_stale",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
                ExpectedAccountState {
                    account_id: "acct_far_reset",
                    availability: AccountAvailability::Reserve,
                    routing_reason: RoutingReason::HeldReserve,
                },
            ],
            expected_selected_weekly_runouts: None,
        });

        assert_eq!(
            account_assessment(&result.final_assessment, "acct_far_reset").routing_reason(),
            RoutingReason::HeldReserve
        );
    }

    #[test]
    fn s3g_hard_blocked_account_is_skipped_before_far_reset_reserve() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_blocked",
                vec![
                    QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible),
                    QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible),
                ],
            ),
            account(
                "acct_drain",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 25, 24 * 3_600, 60 * 3_600),
                ],
            ),
            account(
                "acct_far_reset",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 70, 5 * 86_400, 10 * 86_400),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_drain"),
            "S3g: hard-blocked account is skipped and far-reset reserve is held"
        );
        assert_eq!(
            account_assessment(&assessment, "acct_blocked").availability(),
            AccountAvailability::Blocked
        );
    }

    #[test]
    fn s3i_all_hard_blocked_accounts_have_no_selector_candidate() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible),
                    QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible),
                ],
            ),
            account(
                "acct_b",
                vec![
                    QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible),
                    QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible),
                ],
            ),
            account(
                "acct_c",
                vec![
                    QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible),
                    QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::None);
        assert_eq!(assessment.preferred_next(), None);
        assert!(assessment.weighted_candidates().is_empty());
    }

    #[test]
    fn s3k_near_reset_drain_account_is_used_before_later_resets() {
        let result = assert_account_selection_scenario(&AccountSelectionScenario {
            id: "S3k",
            starts_to_simulate: 5,
            accounts: vec![
                ScenarioAccountFixture {
                    account_id: "acct_reset_soon",
                    initial_active_sessions: 0,
                    build_account: |active_sessions| {
                        account(
                            "acct_reset_soon",
                            vec![
                                window(FIVE_HOURS, 100, hours_minutes(2, 0)),
                                projected_window(WEEKLY, 30, hours_minutes(2, 0), 20 * 3_600),
                            ],
                        )
                        .with_current_active_sessions(active_sessions)
                    },
                },
                ScenarioAccountFixture {
                    account_id: "acct_next_day",
                    initial_active_sessions: 0,
                    build_account: |active_sessions| {
                        account(
                            "acct_next_day",
                            vec![
                                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                                projected_window(WEEKLY, 32, hours_minutes(26, 0), 72 * 3_600),
                            ],
                        )
                        .with_current_active_sessions(active_sessions)
                    },
                },
                ScenarioAccountFixture {
                    account_id: "acct_far_reset",
                    initial_active_sessions: 0,
                    build_account: |active_sessions| {
                        account(
                            "acct_far_reset",
                            vec![
                                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                                projected_window(WEEKLY, 75, 5 * 86_400, 10 * 86_400),
                            ],
                        )
                        .with_current_active_sessions(active_sessions)
                    },
                },
            ],
            expected_sequence: vec![
                "acct_reset_soon",
                "acct_reset_soon",
                "acct_reset_soon",
                "acct_reset_soon",
                "acct_reset_soon",
            ],
            expected_final_active_sessions: vec![
                ("acct_reset_soon", 5),
                ("acct_next_day", 0),
                ("acct_far_reset", 0),
            ],
            expected_final_account_states: vec![
                ExpectedAccountState {
                    account_id: "acct_reset_soon",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::PreferredWeeklyResetSoon,
                },
                ExpectedAccountState {
                    account_id: "acct_next_day",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
                ExpectedAccountState {
                    account_id: "acct_far_reset",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
            ],
            expected_selected_weekly_runouts: None,
        });

        assert_eq!(
            account_assessment(&result.final_assessment, "acct_reset_soon").routing_reason(),
            RoutingReason::PreferredWeeklyResetSoon
        );
    }

    #[test]
    fn s3l_refreshed_reset_segment_does_not_create_fake_old_burn() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_refreshed_far_reset",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 95, 5 * 86_400, 20 * 86_400),
                ],
            ),
            account(
                "acct_current_drain",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 28, 24 * 3_600, 96 * 3_600),
                ],
            ),
            account(
                "acct_reserve",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 65, 5 * 86_400, 20 * 86_400),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_current_drain"),
            "S3l: old reset segment history must not make refreshed far-reset quota look preferable to current drain quota"
        );
    }

    #[test]
    fn s3m_projected_runway_beats_naive_active_count() {
        let result = assert_account_selection_scenario(&AccountSelectionScenario {
            id: "S3m",
            starts_to_simulate: 5,
            accounts: vec![
                ScenarioAccountFixture {
                    account_id: "acct_fast_burn_idle",
                    initial_active_sessions: 0,
                    build_account: |active_sessions| {
                        account(
                            "acct_fast_burn_idle",
                            vec![
                                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                                window_with_per_connection_burn_basis_points_per_hour(
                                    WEEKLY,
                                    18,
                                    24 * 3_600,
                                    80,
                                ),
                            ],
                        )
                        .with_current_active_sessions(active_sessions)
                    },
                },
                ScenarioAccountFixture {
                    account_id: "acct_slower_burn_busy",
                    initial_active_sessions: 1,
                    build_account: |active_sessions| {
                        account(
                            "acct_slower_burn_busy",
                            vec![
                                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                                window_with_per_connection_burn_basis_points_per_hour(
                                    WEEKLY,
                                    18,
                                    24 * 3_600,
                                    40,
                                ),
                            ],
                        )
                        .with_current_active_sessions(active_sessions)
                    },
                },
                ScenarioAccountFixture {
                    account_id: "acct_far_reset_low_burn",
                    initial_active_sessions: 0,
                    build_account: |active_sessions| {
                        account(
                            "acct_far_reset_low_burn",
                            vec![
                                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                                window_with_per_connection_burn_basis_points_per_hour(
                                    WEEKLY,
                                    50,
                                    5 * 86_400,
                                    20,
                                ),
                            ],
                        )
                        .with_current_active_sessions(active_sessions)
                    },
                },
            ],
            expected_sequence: vec![
                "acct_slower_burn_busy",
                "acct_slower_burn_busy",
                "acct_slower_burn_busy",
                "acct_slower_burn_busy",
                "acct_slower_burn_busy",
            ],
            expected_final_active_sessions: vec![
                ("acct_fast_burn_idle", 0),
                ("acct_slower_burn_busy", 6),
                ("acct_far_reset_low_burn", 0),
            ],
            expected_final_account_states: vec![
                ExpectedAccountState {
                    account_id: "acct_fast_burn_idle",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
                ExpectedAccountState {
                    account_id: "acct_slower_burn_busy",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::PreferredNearResetControlledDrain,
                },
                ExpectedAccountState {
                    account_id: "acct_far_reset_low_burn",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
            ],
            expected_selected_weekly_runouts: None,
        });

        assert_ne!(
            result.selected_accounts.first().map(String::as_str),
            Some("acct_fast_burn_idle"),
            "S3m: lower current active count must not beat materially safer projected runway"
        );
    }

    #[test]
    fn preferred_next_matches_first_strict_candidate_without_smooth_selector() {
        let source = include_str!("burn_down.rs");
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(source);

        assert!(
            !production_source.contains("WeightedDeficitSelector"),
            "burn-down preferred_next must be the first strict candidate, not a smooth weighted selector"
        );
    }

    #[test]
    fn scenario_b_allows_weekly_salvage_when_weekly_reset_is_near() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![window(FIVE_HOURS, 5, 120), window(WEEKLY, 80, 5 * 86_400)],
            ),
            account(
                "acct_b",
                vec![window(FIVE_HOURS, 90, 4 * 3_600), window(WEEKLY, 20, 600)],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_b")
        );
        assert_account(&assessment, "acct_a", AccountAvailability::Usable, Some(9));
        assert_account(&assessment, "acct_b", AccountAvailability::Usable, Some(39));
        assert_eq!(
            account_assessment(&assessment, "acct_b").routing_reason(),
            RoutingReason::PreferredWeeklyResetSoon
        );
    }

    #[test]
    fn scenario_c_blocks_empty_weekly_window() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 80, 4 * 3_600),
                    window(WEEKLY, 0, 5 * 86_400),
                ],
            ),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 42, 4 * 3_600),
                    window(WEEKLY, 42, 5 * 86_400),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Reserve);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_b")
        );
        assert_account(&assessment, "acct_a", AccountAvailability::Blocked, None);
        assert_account(&assessment, "acct_b", AccountAvailability::Reserve, Some(0));
        assert_eq!(
            account_assessment(&assessment, "acct_a").routing_reason(),
            RoutingReason::BlockedWindowExhausted
        );
    }

    #[test]
    fn scenario_d_prefers_short_window_near_reset_surplus() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![window(FIVE_HOURS, 30, 600), window(WEEKLY, 60, 3 * 86_400)],
            ),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 30, 4 * 3_600),
                    window(WEEKLY, 60, 3 * 86_400),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a")
        );
        assert_account(&assessment, "acct_a", AccountAvailability::Usable, Some(40));
        assert_account(&assessment, "acct_b", AccountAvailability::Usable, Some(0));
        assert_eq!(
            account_assessment(&assessment, "acct_a").routing_reason(),
            RoutingReason::PreferredShortResetSoon
        );
    }

    #[test]
    fn weak_quota_candidate_is_not_clamped_to_minimum_score_s1() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_weak",
                vec![
                    window(FIVE_HOURS, 30, 4 * 3_600),
                    window(WEEKLY, 60, 3 * 86_400),
                ],
            ),
            account(
                "acct_healthier",
                vec![
                    window(FIVE_HOURS, 80, 4 * 3_600),
                    window(WEEKLY, 80, 3 * 86_400),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_healthier")
        );
        assert_account(
            &assessment,
            "acct_weak",
            AccountAvailability::Usable,
            Some(0),
        );
        assert!(
            assessment
                .weighted_candidates()
                .iter()
                .any(|(account_id, weight)| account_id.as_str() == "acct_weak" && *weight == 0),
            "weak accounts may remain visible in the selected pool, but must not be manufactured as score 1"
        );
    }

    #[test]
    fn unknown_quota_is_fallback_only_when_known_pool_exists() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 50, 2 * 3_600),
                    window(WEEKLY, 50, 3 * 86_400),
                ],
            ),
            account(
                "acct_b",
                vec![
                    QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Unknown)
                        .with_remaining_headroom(90),
                    QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Unknown)
                        .with_remaining_headroom(90),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(assessment.weighted_candidates().len(), 1);
        assert_eq!(assessment.weighted_candidates()[0].0.as_str(), "acct_a");
        let unknown = account_assessment(&assessment, "acct_b");
        assert_eq!(unknown.availability(), AccountAvailability::Unknown);
        assert_eq!(unknown.routing_weight(), Some(1));
        assert_eq!(unknown.routing_reason(), RoutingReason::HeldUnknown);
        assert_eq!(
            unknown.quota_evidence_reason(),
            QuotaEvidenceReason::UnknownQuotaWindow
        );
    }

    #[test]
    fn all_unknown_accounts_use_fallback_pool_with_candidates() {
        let assessment = assess_route_band(input(vec![
            account("acct_a", Vec::new()),
            account(
                "acct_b",
                vec![
                    QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Unknown),
                    QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Unknown),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Unknown);
        assert_eq!(assessment.weighted_candidates().len(), 2);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a")
        );
        assert_account(&assessment, "acct_a", AccountAvailability::Unknown, Some(1));
        assert_account(&assessment, "acct_b", AccountAvailability::Unknown, Some(1));
        assert_eq!(
            account_assessment(&assessment, "acct_a").routing_reason(),
            RoutingReason::UnknownFallbackPreferred
        );
        assert_eq!(
            account_assessment(&assessment, "acct_b").routing_reason(),
            RoutingReason::UnknownFallbackAvailable
        );
    }

    #[test]
    fn missing_reset_or_expected_window_is_probe_required() {
        let missing_reset = assess_route_band(input(vec![account(
            "acct_missing_reset",
            vec![
                QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Eligible)
                    .with_remaining_headroom(50),
                window(WEEKLY, 50, 2 * 86_400),
            ],
        )]));
        let missing_expected = assess_route_band(input(vec![account(
            "acct_missing_expected",
            vec![window(FIVE_HOURS, 50, 2 * 3_600)],
        )]));

        assert_eq!(
            account_assessment(&missing_reset, "acct_missing_reset").quota_evidence_reason(),
            QuotaEvidenceReason::MissingResetTime
        );
        assert_eq!(
            account_assessment(&missing_expected, "acct_missing_expected").quota_evidence_reason(),
            QuotaEvidenceReason::MissingExpectedWindow
        );
        assert_eq!(missing_reset.selected_pool(), SelectedPool::Unknown);
        assert_eq!(missing_expected.selected_pool(), SelectedPool::Unknown);
        assert_eq!(missing_reset.weighted_candidates().len(), 1);
        assert_eq!(missing_expected.weighted_candidates().len(), 1);
    }

    #[test]
    fn stale_penalty_applies_only_inside_selected_pool() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_fresh",
                vec![
                    window(FIVE_HOURS, 80, 4 * 3_600),
                    window(WEEKLY, 80, 5 * 86_400),
                ],
            ),
            account(
                "acct_stale",
                vec![
                    stale_window(FIVE_HOURS, 80, 4 * 3_600),
                    stale_window(WEEKLY, 80, 5 * 86_400),
                ],
            ),
        ]));

        assert_account(
            &assessment,
            "acct_fresh",
            AccountAvailability::Usable,
            Some(80),
        );
        assert_account(
            &assessment,
            "acct_stale",
            AccountAvailability::Usable,
            Some(20),
        );
    }

    #[test]
    fn disabled_and_missing_credential_accounts_are_excluded() {
        let disabled = account(
            "acct_disabled",
            vec![
                window(FIVE_HOURS, 80, 4 * 3_600),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        )
        .with_account_enabled(false);
        let missing_credential = account(
            "acct_missing_credential",
            vec![
                window(FIVE_HOURS, 80, 4 * 3_600),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        )
        .with_active_credential(false);
        let assessment = assess_route_band(input(vec![disabled, missing_credential]));

        assert_eq!(assessment.selected_pool(), SelectedPool::None);
        assert_eq!(
            account_assessment(&assessment, "acct_disabled").routing_exclusion(),
            RoutingExclusion::Disabled
        );
        assert_eq!(
            account_assessment(&assessment, "acct_missing_credential").routing_exclusion(),
            RoutingExclusion::MissingCredential
        );
    }

    #[test]
    fn deterministic_order_uses_weight_pressure_salvage_and_account_id() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_b",
                vec![window(FIVE_HOURS, 30, 600), window(WEEKLY, 60, 3 * 86_400)],
            ),
            account(
                "acct_a",
                vec![window(FIVE_HOURS, 30, 600), window(WEEKLY, 60, 3 * 86_400)],
            ),
        ]));

        assert_eq!(assessment.weighted_candidates()[0].0.as_str(), "acct_a");
        assert_eq!(assessment.weighted_candidates()[1].0.as_str(), "acct_b");
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a")
        );
    }

    #[test]
    fn legacy_active_load_pressure_does_not_change_projected_burn() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 70, 4 * 3_600),
                    window(WEEKLY, 70, 5 * 86_400),
                ],
            )
            .with_active_load_pressure(30),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 70, 4 * 3_600),
                    window(WEEKLY, 70, 5 * 86_400),
                ],
            ),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_a")
        );
        assert_eq!(
            account_assessment(&assessment, "acct_a").projected_burn_pressure(),
            account_assessment(&assessment, "acct_b").projected_burn_pressure(),
            "legacy active pressure must not be treated as projected quota burn"
        );
    }

    #[test]
    fn legacy_active_load_pressure_does_not_change_selection_s2() {
        let without_legacy_pressure = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 70, 4 * 3_600),
                    window(WEEKLY, 70, 5 * 86_400),
                ],
            ),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 70, 4 * 3_600),
                    window(WEEKLY, 70, 5 * 86_400),
                ],
            ),
        ]));
        let with_legacy_pressure = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 70, 4 * 3_600),
                    window(WEEKLY, 70, 5 * 86_400),
                ],
            )
            .with_active_load_pressure(100),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 70, 4 * 3_600),
                    window(WEEKLY, 70, 5 * 86_400),
                ],
            ),
        ]));

        assert_eq!(
            with_legacy_pressure.preferred_next().map(AccountId::as_str),
            without_legacy_pressure
                .preferred_next()
                .map(AccountId::as_str),
            "S2: legacy active_pressure/headroom cost must not affect quota selection"
        );
    }

    #[test]
    fn unknown_survival_margin_does_not_route_to_weaker_account_for_lower_active_count() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_healthier",
                vec![
                    window(FIVE_HOURS, 60, 4 * 3_600),
                    window(WEEKLY, 60, 3 * 86_400),
                ],
            )
            .with_current_active_sessions(2),
            account(
                "acct_weaker_idle",
                vec![
                    window(FIVE_HOURS, 98, 4 * 3_600),
                    window(WEEKLY, 23, 3 * 86_400),
                ],
            )
            .with_current_active_sessions(0),
        ]));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_healthier"),
            "unknown-margin active count must not beat healthier raw weekly quota"
        );
    }

    #[test]
    fn near_zero_projected_short_runout_is_held_by_flow_guard() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_fast",
                vec![
                    projected_window(FIVE_HOURS, 20, 4 * 3_600, 600),
                    window(WEEKLY, 80, 5 * 86_400),
                ],
            ),
            account(
                "acct_slow",
                vec![
                    window(FIVE_HOURS, 20, 4 * 3_600),
                    window(WEEKLY, 80, 5 * 86_400),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_slow")
        );
        assert_account(&assessment, "acct_fast", AccountAvailability::Blocked, None);
        assert_eq!(
            account_assessment(&assessment, "acct_fast").routing_reason(),
            RoutingReason::HeldShortWindowGuard
        );
    }

    #[test]
    fn near_zero_projected_runout_survives_to_reset_stays_selectable() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_survives",
                vec![
                    projected_window(FIVE_HOURS, 20, 20 * 60, 25 * 60),
                    window(WEEKLY, 80, 5 * 86_400),
                ],
            ),
            account(
                "acct_worse_weekly",
                vec![
                    window(FIVE_HOURS, 80, 4 * 3_600),
                    window(WEEKLY, 10, 5 * 86_400),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_survives")
        );
        assert_account(
            &assessment,
            "acct_survives",
            AccountAvailability::Usable,
            Some(30),
        );
    }

    #[test]
    fn near_zero_headroom_stays_selectable_when_no_alternative_can_serve() {
        let assessment = assess_route_band(input(vec![account(
            "acct_near_empty",
            vec![
                window(FIVE_HOURS, 4, 4 * 3_600),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        )]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_near_empty")
        );
        assert_account(
            &assessment,
            "acct_near_empty",
            AccountAvailability::Usable,
            Some(0),
        );
    }

    #[test]
    fn near_zero_headroom_stays_selectable_when_all_alternatives_are_worse() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_near_empty",
                vec![
                    window(FIVE_HOURS, 4, 4 * 3_600),
                    window(WEEKLY, 90, 5 * 86_400),
                ],
            ),
            account(
                "acct_worse_weekly",
                vec![
                    window(FIVE_HOURS, 90, 4 * 3_600),
                    window(WEEKLY, 6, 5 * 86_400),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_near_empty")
        );
        assert_account(
            &assessment,
            "acct_near_empty",
            AccountAvailability::Usable,
            Some(0),
        );
        assert_account(
            &assessment,
            "acct_worse_weekly",
            AccountAvailability::Reserve,
            Some(0),
        );
    }

    #[test]
    fn near_zero_headroom_retires_when_not_worse_alternative_exists() {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_near_empty",
                vec![
                    window(FIVE_HOURS, 4, 4 * 3_600),
                    window(WEEKLY, 80, 5 * 86_400),
                ],
            ),
            account(
                "acct_healthy",
                vec![
                    window(FIVE_HOURS, 40, 4 * 3_600),
                    window(WEEKLY, 80, 5 * 86_400),
                ],
            ),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_healthy")
        );
        assert_account(
            &assessment,
            "acct_near_empty",
            AccountAvailability::Retiring,
            None,
        );
        assert_account(
            &assessment,
            "acct_healthy",
            AccountAvailability::Usable,
            Some(0),
        );
    }

    #[test]
    fn six_session_selection_stays_in_same_weekly_pool_before_far_reset_reserve_a5_s1() {
        let mut selected_accounts = Vec::new();

        for _session_start in 0..6 {
            let assessment = assess_route_band(input(vec![
                account(
                    "acct_askluna",
                    vec![
                        window(FIVE_HOURS, 98, 4 * 3_600),
                        window(WEEKLY, 23, 3 * 86_400),
                    ],
                ),
                account(
                    "acct_matches",
                    vec![
                        window(FIVE_HOURS, 99, 4 * 3_600),
                        window(WEEKLY, 34, 3 * 86_400),
                    ],
                ),
                account(
                    "acct_ssdev",
                    vec![
                        window(FIVE_HOURS, 78, 3 * 3_600),
                        window(WEEKLY, 76, 5 * 86_400),
                    ],
                ),
            ]));
            let selected = assessment
                .preferred_next()
                .unwrap_or_else(|| panic!("session start should have a quota candidate"))
                .as_str();
            selected_accounts.push(selected.to_owned());
        }

        assert_eq!(
            selected_accounts.first().map(String::as_str),
            Some("acct_matches")
        );
        assert_eq!(
            selected_accounts,
            vec![
                "acct_matches".to_owned(),
                "acct_matches".to_owned(),
                "acct_matches".to_owned(),
                "acct_matches".to_owned(),
                "acct_matches".to_owned(),
                "acct_matches".to_owned(),
            ],
            "without measured active-session input, strict quota selection is deterministic"
        );
        assert!(
            selected_accounts
                .iter()
                .any(|account| account == "acct_matches"),
            "same low-weekly reset pool should be used before far-reset reserve: {selected_accounts:?}"
        );
        assert!(
            selected_accounts
                .iter()
                .all(|account| account != "acct_askluna"),
            "weak weekly quota account must not be selected while healthier accounts exist: {selected_accounts:?}"
        );
        assert!(
            selected_accounts
                .iter()
                .all(|account| account != "acct_ssdev"),
            "far-reset reserve must not be selected while same-pool account can serve: {selected_accounts:?}"
        );
    }

    #[test]
    fn s4_low_weekly_pool_drains_b_b_a_b_a() {
        fn askluna(active_sessions: u32) -> BurnDownAccountInput {
            account(
                "acct_askluna",
                vec![
                    window(FIVE_HOURS, 99, hours_minutes(4, 46)),
                    projected_window(
                        WEEKLY,
                        4,
                        hours_minutes(22, 49),
                        askluna_projected_weekly_runout(active_sessions),
                    )
                    .with_per_connection_burn_basis_points_per_hour(39),
                ],
            )
            .with_current_active_sessions(active_sessions)
        }

        fn matches(active_sessions: u32) -> BurnDownAccountInput {
            account(
                "acct_matches",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 59)),
                    projected_window(
                        WEEKLY,
                        8,
                        hours_minutes(23, 56),
                        matches_projected_weekly_runout(active_sessions),
                    )
                    .with_per_connection_burn_basis_points_per_hour(53),
                ],
            )
            .with_current_active_sessions(active_sessions)
        }

        fn ssdev(active_sessions: u32) -> BurnDownAccountInput {
            account(
                "acct_ssdev",
                vec![
                    window(FIVE_HOURS, 97, hours_minutes(4, 36)),
                    projected_window(
                        WEEKLY,
                        26,
                        84 * 3_600,
                        ssdev_projected_weekly_runout(active_sessions),
                    )
                    .with_per_connection_burn_basis_points_per_hour(105),
                ],
            )
            .with_current_active_sessions(active_sessions)
        }

        let result = assert_account_selection_scenario(&AccountSelectionScenario {
            id: "S4",
            starts_to_simulate: 5,
            accounts: vec![
                ScenarioAccountFixture {
                    account_id: "acct_askluna",
                    initial_active_sessions: 1,
                    build_account: askluna,
                },
                ScenarioAccountFixture {
                    account_id: "acct_matches",
                    initial_active_sessions: 0,
                    build_account: matches,
                },
                ScenarioAccountFixture {
                    account_id: "acct_ssdev",
                    initial_active_sessions: 1,
                    build_account: ssdev,
                },
            ],
            expected_sequence: vec![
                "acct_matches",
                "acct_matches",
                "acct_askluna",
                "acct_matches",
                "acct_askluna",
            ],
            expected_final_active_sessions: vec![
                ("acct_askluna", 3),
                ("acct_matches", 3),
                ("acct_ssdev", 1),
            ],
            expected_final_account_states: vec![
                ExpectedAccountState {
                    account_id: "acct_askluna",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
                ExpectedAccountState {
                    account_id: "acct_matches",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::PreferredNearResetControlledDrain,
                },
                ExpectedAccountState {
                    account_id: "acct_ssdev",
                    availability: AccountAvailability::Reserve,
                    routing_reason: RoutingReason::HeldReserve,
                },
            ],
            expected_selected_weekly_runouts: Some(vec![
                ("acct_matches", Some(hours_minutes(15, 5))),
                ("acct_matches", Some(hours_minutes(7, 32))),
                ("acct_askluna", Some(hours_minutes(5, 7))),
                ("acct_matches", Some(hours_minutes(5, 2))),
                ("acct_askluna", Some(hours_minutes(3, 25))),
            ]),
        });

        assert_eq!(
            account_assessment(&result.final_assessment, "acct_askluna").availability(),
            AccountAvailability::Usable,
            "S4: A remains usable for controlled drain instead of being retired"
        );
    }

    #[test]
    fn s5_far_reset_reserve_is_preserved_while_near_reset_pool_can_serve() {
        fn acct_a(active_sessions: u32) -> BurnDownAccountInput {
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 18, 24 * 3_600, 180 * 3_600)
                        .with_per_connection_burn_basis_points_per_hour(10),
                ],
            )
            .with_current_active_sessions(active_sessions)
        }

        fn acct_b(active_sessions: u32) -> BurnDownAccountInput {
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 19, hours_minutes(25, 0), 190 * 3_600)
                        .with_per_connection_burn_basis_points_per_hour(10),
                ],
            )
            .with_current_active_sessions(active_sessions)
        }

        fn acct_c(active_sessions: u32) -> BurnDownAccountInput {
            account(
                "acct_c",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 60, 96 * 3_600, 600 * 3_600)
                        .with_per_connection_burn_basis_points_per_hour(10),
                ],
            )
            .with_current_active_sessions(active_sessions)
        }

        let result = assert_account_selection_scenario(&AccountSelectionScenario {
            id: "S5",
            starts_to_simulate: 5,
            accounts: vec![
                ScenarioAccountFixture {
                    account_id: "acct_a",
                    initial_active_sessions: 0,
                    build_account: acct_a,
                },
                ScenarioAccountFixture {
                    account_id: "acct_b",
                    initial_active_sessions: 0,
                    build_account: acct_b,
                },
                ScenarioAccountFixture {
                    account_id: "acct_c",
                    initial_active_sessions: 0,
                    build_account: acct_c,
                },
            ],
            expected_sequence: vec!["acct_a", "acct_b", "acct_a", "acct_b", "acct_a"],
            expected_final_active_sessions: vec![("acct_a", 3), ("acct_b", 2), ("acct_c", 0)],
            expected_final_account_states: vec![
                ExpectedAccountState {
                    account_id: "acct_a",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
                ExpectedAccountState {
                    account_id: "acct_b",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::PreferredNearResetDrainable,
                },
                ExpectedAccountState {
                    account_id: "acct_c",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
            ],
            expected_selected_weekly_runouts: None,
        });

        assert_ne!(
            result
                .final_assessment
                .preferred_next()
                .map(AccountId::as_str),
            Some("acct_c"),
            "S5: C must still not be the next account after the simulated starts"
        );
    }

    #[test]
    fn s3n_same_effective_weekly_pool_spreads_by_active_sessions() {
        fn acct_a(active_sessions: u32) -> BurnDownAccountInput {
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 18, 24 * 3_600, 45 * 3_600)
                        .with_per_connection_burn_basis_points_per_hour(40),
                ],
            )
            .with_current_active_sessions(active_sessions)
        }

        fn acct_b(active_sessions: u32) -> BurnDownAccountInput {
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 19, 24 * 3_600, hours_minutes(47, 30))
                        .with_per_connection_burn_basis_points_per_hour(40),
                ],
            )
            .with_current_active_sessions(active_sessions)
        }

        fn acct_c(active_sessions: u32) -> BurnDownAccountInput {
            account(
                "acct_c",
                vec![
                    window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                    projected_window(WEEKLY, 20, 24 * 3_600, 50 * 3_600)
                        .with_per_connection_burn_basis_points_per_hour(40),
                ],
            )
            .with_current_active_sessions(active_sessions)
        }

        assert_account_selection_scenario(&AccountSelectionScenario {
            id: "S3n",
            starts_to_simulate: 5,
            accounts: vec![
                ScenarioAccountFixture {
                    account_id: "acct_a",
                    initial_active_sessions: 0,
                    build_account: acct_a,
                },
                ScenarioAccountFixture {
                    account_id: "acct_b",
                    initial_active_sessions: 0,
                    build_account: acct_b,
                },
                ScenarioAccountFixture {
                    account_id: "acct_c",
                    initial_active_sessions: 0,
                    build_account: acct_c,
                },
            ],
            expected_sequence: vec!["acct_c", "acct_b", "acct_a", "acct_c", "acct_b"],
            expected_final_active_sessions: vec![("acct_a", 1), ("acct_b", 2), ("acct_c", 2)],
            expected_final_account_states: vec![
                ExpectedAccountState {
                    account_id: "acct_a",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::PreferredNearResetControlledDrain,
                },
                ExpectedAccountState {
                    account_id: "acct_b",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
                ExpectedAccountState {
                    account_id: "acct_c",
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                },
            ],
            expected_selected_weekly_runouts: None,
        });
    }

    #[test]
    fn account_assessment_uses_safe_display_label() {
        let assessment = assess_route_band(input(vec![BurnDownAccountInput::new(
            account_id("acct_secret"),
            "person@example.com",
            vec![
                window(FIVE_HOURS, 80, 4 * 3_600),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        )]));

        let account = account_assessment(&assessment, "acct_secret");
        assert!(account.account_label().starts_with("acct-"));
        assert!(!account.account_label().contains("person"));
        assert!(!account.account_label().contains('@'));
    }

    fn input(accounts: Vec<BurnDownAccountInput>) -> BurnDownRouteBandAssessmentInput {
        BurnDownRouteBandAssessmentInput::new(RouteBand::Responses, NOW, accounts)
    }

    fn account(account_id_value: &str, windows: Vec<QuotaWindowFact>) -> BurnDownAccountInput {
        BurnDownAccountInput::new(account_id(account_id_value), account_id_value, windows)
    }

    fn window(
        window_seconds: u64,
        remaining_headroom: u32,
        resets_in_seconds: u64,
    ) -> QuotaWindowFact {
        QuotaWindowFact::new(window_seconds, QuotaWindowStatus::Eligible)
            .with_remaining_headroom(remaining_headroom)
            .with_reset_unix_seconds(NOW + resets_in_seconds)
            .with_observed_unix_seconds(NOW)
    }

    fn stale_window(
        window_seconds: u64,
        remaining_headroom: u32,
        resets_in_seconds: u64,
    ) -> QuotaWindowFact {
        QuotaWindowFact::new(window_seconds, QuotaWindowStatus::Stale)
            .with_remaining_headroom(remaining_headroom)
            .with_reset_unix_seconds(NOW + resets_in_seconds)
            .with_observed_unix_seconds(NOW)
    }

    fn projected_window(
        window_seconds: u64,
        remaining_headroom: u32,
        resets_in_seconds: u64,
        projected_runout_in_seconds: u64,
    ) -> QuotaWindowFact {
        window(window_seconds, remaining_headroom, resets_in_seconds)
            .with_projected_exhaustion_unix_seconds(NOW + projected_runout_in_seconds)
    }

    fn window_with_per_connection_burn_basis_points_per_hour(
        window_seconds: u64,
        remaining_headroom: u32,
        resets_in_seconds: u64,
        burn_rate_basis_points_per_hour: u32,
    ) -> QuotaWindowFact {
        if burn_rate_basis_points_per_hour == 0 {
            return window(window_seconds, remaining_headroom, resets_in_seconds)
                .with_per_connection_burn_basis_points_per_hour(burn_rate_basis_points_per_hour);
        }

        let remaining_basis_points = u64::from(remaining_headroom) * 100;
        let runout_seconds = remaining_basis_points
            .saturating_mul(3_600)
            .checked_div(u64::from(burn_rate_basis_points_per_hour))
            .unwrap_or(u64::MAX);
        projected_window(
            window_seconds,
            remaining_headroom,
            resets_in_seconds,
            runout_seconds,
        )
        .with_per_connection_burn_basis_points_per_hour(burn_rate_basis_points_per_hour)
    }

    const fn hours_minutes(hours: u64, minutes: u64) -> u64 {
        hours * 3_600 + minutes * 60
    }

    const fn matches_projected_weekly_runout(active_sessions: u32) -> u64 {
        match active_sessions {
            0 => hours_minutes(15, 5),
            1 => hours_minutes(7, 32),
            2 => hours_minutes(5, 2),
            _ => hours_minutes(3, 46),
        }
    }

    const fn askluna_projected_weekly_runout(active_sessions: u32) -> u64 {
        match active_sessions {
            0 | 1 => hours_minutes(5, 7),
            _ => hours_minutes(3, 25),
        }
    }

    const fn ssdev_projected_weekly_runout(active_sessions: u32) -> u64 {
        match active_sessions {
            0 => 24 * 3_600,
            1 => hours_minutes(17, 20),
            2 => hours_minutes(13, 0),
            _ => hours_minutes(10, 24),
        }
    }

    fn account_assessment<'a>(
        assessment: &'a BurnDownRouteBandAssessmentResult,
        account_id_value: &str,
    ) -> &'a BurnDownAccountAssessment {
        assessment
            .accounts()
            .iter()
            .find(|account| account.account_id().as_str() == account_id_value)
            .unwrap_or_else(|| panic!("missing account assessment: {account_id_value}"))
    }

    fn assert_account(
        assessment: &BurnDownRouteBandAssessmentResult,
        account_id_value: &str,
        availability: AccountAvailability,
        routing_weight: Option<u32>,
    ) {
        let account = account_assessment(assessment, account_id_value);
        assert_eq!(account.availability(), availability, "{account_id_value}");
        assert_eq!(
            account.routing_weight(),
            routing_weight,
            "{account_id_value}"
        );
    }

    fn account_id(value: &str) -> AccountId {
        AccountId::new(value).unwrap_or_else(|error| panic!("account id should parse: {error}"))
    }
}
