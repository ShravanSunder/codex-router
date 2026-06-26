//! Reset-aware quota burn-down assessment.

use codex_router_core::ids::AccountId;
use codex_router_core::redaction::safe_account_label;
use codex_router_core::routes::RouteBand;

/// Fixed v1 short quota window in seconds.
pub const V1_SHORT_WINDOW_SECONDS: u64 = 18_000;
/// Fixed v1 weekly quota window in seconds.
pub const V1_WEEKLY_WINDOW_SECONDS: u64 = 604_800;

const DEFAULT_SHORT_WINDOW_CUTOFF_SECONDS: u64 = 86_400;
const DEFAULT_SHORT_NEAR_RESET_MAX_SECONDS: u64 = 1_800;
const DEFAULT_LONG_NEAR_RESET_MAX_SECONDS: u64 = 43_200;
const DEFAULT_RESERVE_PRESSURE_THRESHOLD: u32 = 25;
const DEFAULT_RESERVE_HEADROOM_THRESHOLD: u32 = 10;
const DEFAULT_LONG_PRESSURE_MULTIPLIER: u32 = 3;
const DEFAULT_SHORT_SALVAGE_CAP: u32 = 10;
const DEFAULT_LONG_SALVAGE_CAP: u32 = 20;
const DEFAULT_RISK_PENALTY_CAP: u32 = 90;
const DEFAULT_SELECTABLE_WEIGHT_MIN: u32 = 1;
const DEFAULT_SELECTABLE_WEIGHT_MAX: u32 = 100;
const DEFAULT_UNKNOWN_FALLBACK_WEIGHT: u32 = 1;

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

    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
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

    /// Returns window seconds.
    #[must_use]
    pub const fn window_seconds(&self) -> u64 {
        self.window_seconds
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
    /// Account is disabled.
    AccountDisabled,
    /// Account lacks active credentials.
    MissingCredential,
}

/// Public routing reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutingReason {
    /// Preferred because weekly quota is healthier than alternatives.
    PreferredWeeklyHealthier,
    /// Preferred because weekly reset is near.
    PreferredWeeklyResetSoon,
    /// Preferred because the short window reset is near.
    PreferredShortResetSoon,
    /// Preferred because projected burn lasts longer than alternatives.
    PreferredProjectedBurn,
    /// Preferred by neutral quota weight.
    PreferredHighestWeight,
    /// Same-pool selectable account.
    AvailableSamePool,
    /// Reserve account held behind usable accounts.
    HeldReserve,
    /// Unknown account held behind known accounts.
    HeldUnknown,
    /// Preferred fallback account that needs refresh.
    UnknownFallbackPreferred,
    /// Non-preferred fallback account in the unknown pool.
    UnknownFallbackAvailable,
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
            Self::PreferredWeeklyHealthier => "preferred_weekly_healthier",
            Self::PreferredWeeklyResetSoon => "preferred_weekly_reset_soon",
            Self::PreferredShortResetSoon => "preferred_short_reset_soon",
            Self::PreferredProjectedBurn => "preferred_projected_burn",
            Self::PreferredHighestWeight => "preferred_highest_weight",
            Self::AvailableSamePool => "available_same_pool",
            Self::HeldReserve => "held_reserve",
            Self::HeldUnknown => "held_unknown",
            Self::UnknownFallbackPreferred => "unknown_fallback_preferred",
            Self::UnknownFallbackAvailable => "unknown_fallback_available",
            Self::ExcludedDisabled => "excluded_disabled",
            Self::ExcludedMissingCredential => "excluded_missing_credential",
            Self::BlockedWindowExhausted => "blocked_window_exhausted",
            Self::BlockedWindowIneligible => "blocked_window_ineligible",
        }
    }

    /// Returns the stable human phrase for this public routing reason.
    #[must_use]
    pub const fn human_phrase(self) -> &'static str {
        match self {
            Self::PreferredWeeklyHealthier => "preferred next: weekly healthier",
            Self::PreferredWeeklyResetSoon => "preferred next: weekly reset soon",
            Self::PreferredShortResetSoon => "preferred next: 5h reset soon",
            Self::PreferredProjectedBurn => "preferred next: projected burn",
            Self::PreferredHighestWeight => "preferred next: safest quota",
            Self::AvailableSamePool => "available: same pool",
            Self::HeldReserve => "held: reserve",
            Self::HeldUnknown => "held: needs refresh",
            Self::UnknownFallbackPreferred => "fallback: needs refresh",
            Self::UnknownFallbackAvailable => "fallback: same unknown pool",
            Self::ExcludedDisabled => "blocked: disabled",
            Self::ExcludedMissingCredential => "blocked: missing credential",
            Self::BlockedWindowExhausted => "blocked: quota empty",
            Self::BlockedWindowIneligible => "blocked: quota ineligible",
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
    surplus: u32,
    time_left_seconds: Option<u64>,
    near_reset: bool,
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
        right_weight
            .cmp(left_weight)
            .then_with(|| left.long_pressure.cmp(&right.long_pressure))
            .then_with(|| left.short_pressure.cmp(&right.short_pressure))
            .then_with(|| compare_salvage_key(left, right))
            .then_with(|| left.account_id.cmp(&right.account_id))
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
            .saturating_add(short_pressure)
            .saturating_add(input.active_load_pressure),
    );
    let projected_burn_pressure = windows
        .iter()
        .map(|window| window.projected_pressure)
        .max()
        .unwrap_or(0)
        .saturating_add(input.active_load_pressure)
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
        salvage_sort_key: salvage_sort_key(&windows, short_salvage, long_salvage, policy),
        routing_reason: RoutingReason::AvailableSamePool,
        ..base
    }
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

    WindowAssessment {
        window_seconds: window.window_seconds,
        remaining_headroom,
        reset_unix_seconds: window.reset_unix_seconds,
        status: window.status,
        pressure,
        projected_pressure,
        surplus,
        time_left_seconds,
        near_reset,
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
                && (window.pressure >= policy.reserve_pressure_threshold
                    || window.remaining_headroom <= policy.reserve_headroom_threshold)
        })
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

    if account.long_salvage > 0 {
        return RoutingReason::PreferredWeeklyResetSoon;
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

    RoutingReason::PreferredHighestWeight
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

const fn is_short_window(window_seconds: u64, policy: BurnDownRouteBandPolicy) -> bool {
    window_seconds < policy.short_window_cutoff_seconds
}

const fn near_reset_seconds(window_seconds: u64, policy: BurnDownRouteBandPolicy) -> u64 {
    let tenth = window_seconds / 10;
    if is_short_window(window_seconds, policy) {
        min_u64(DEFAULT_SHORT_NEAR_RESET_MAX_SECONDS, tenth)
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
        assert_account(&assessment, "acct_b", AccountAvailability::Reserve, Some(1));
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
        assert_account(&assessment, "acct_b", AccountAvailability::Reserve, Some(1));
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
        assert_account(&assessment, "acct_b", AccountAvailability::Usable, Some(1));
        assert_eq!(
            account_assessment(&assessment, "acct_a").routing_reason(),
            RoutingReason::PreferredShortResetSoon
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
    fn active_load_pressure_shifts_selection_away_from_loaded_account() {
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
            Some("acct_b")
        );
        assert_eq!(
            account_assessment(&assessment, "acct_b").routing_reason(),
            RoutingReason::PreferredProjectedBurn
        );
        assert!(
            account_assessment(&assessment, "acct_a").projected_burn_pressure()
                > account_assessment(&assessment, "acct_b").projected_burn_pressure()
        );
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
