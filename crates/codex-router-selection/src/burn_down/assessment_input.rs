//! Public route/account assessment inputs.
use super::routing_policy::{BurnDownRouteBandPolicy, clamp_u32, policy_for_route_band};
use super::window_facts::QuotaWindowFact;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::{RouteProfile, WindowKind};
use codex_router_core::routes::RouteBand;

/// Input for one route-band assessment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BurnDownRouteBandAssessmentInput {
    pub(super) route_band: RouteBand,
    pub(super) now_unix_seconds: u64,
    pub(super) accounts: Vec<BurnDownAccountInput>,
    pub(super) policy: BurnDownRouteBandPolicy,
    pub(super) route_profile: RouteProfile,
}

/// Whether the current coherent account projection permits credit-backed Responses routing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CreditBackedEligibility {
    /// Current credit authority is missing or one of the routing guards denies its use.
    #[default]
    Ineligible,
    /// Current provider authority permits credit backing after included quota exhaustion.
    Eligible,
}

impl BurnDownRouteBandAssessmentInput {
    /// Creates route-band assessment input.
    #[must_use]
    pub fn new(
        route_band: RouteBand,
        now_unix_seconds: u64,
        route_profile: RouteProfile,
        accounts: Vec<BurnDownAccountInput>,
    ) -> Self {
        Self {
            route_band,
            now_unix_seconds,
            accounts,
            policy: policy_for_route_band(route_band),
            route_profile,
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
    pub(super) account_id: AccountId,
    pub(super) account_label: String,
    pub(super) provider: Provider,
    pub(super) windows: Vec<QuotaWindowFact>,
    pub(super) canonical_responses_windows: Option<Vec<QuotaWindowFact>>,
    pub(super) rejected_windows: Vec<QuotaWindowRejectionFact>,
    pub(super) account_enabled: bool,
    pub(super) has_active_credential: bool,
    pub(super) active_load_pressure: u32,
    pub(super) current_active_sessions: u32,
    pub(super) weekly_quota_floor_basis_points: Option<u32>,
    pub(super) credit_backed_eligibility: CreditBackedEligibility,
}

impl BurnDownAccountInput {
    /// Creates an account input.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        account_label: impl Into<String>,
        provider: Provider,
        windows: Vec<QuotaWindowFact>,
    ) -> Self {
        Self {
            account_id,
            account_label: account_label.into(),
            provider,
            windows,
            canonical_responses_windows: None,
            rejected_windows: Vec::new(),
            account_enabled: true,
            has_active_credential: true,
            active_load_pressure: 0,
            current_active_sessions: 0,
            weekly_quota_floor_basis_points: None,
            credit_backed_eligibility: CreditBackedEligibility::Ineligible,
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

    /// Supplies outstanding Claude window rejections from durable state.
    #[must_use]
    pub fn with_rejected_windows(
        mut self,
        rejected_windows: Vec<QuotaWindowRejectionFact>,
    ) -> Self {
        self.rejected_windows = rejected_windows;
        self
    }

    /// Supplies optional canonical Responses windows for compact credit assessment.
    #[must_use]
    pub fn with_canonical_responses_windows(
        mut self,
        windows: Option<Vec<QuotaWindowFact>>,
    ) -> Self {
        self.canonical_responses_windows = windows;
        self
    }

    /// Sets an optional hard weekly quota floor. Zero disables the floor.
    #[must_use]
    pub const fn with_weekly_quota_floor_basis_points(
        mut self,
        weekly_quota_floor_basis_points: u32,
    ) -> Self {
        self.weekly_quota_floor_basis_points = if weekly_quota_floor_basis_points == 0 {
            None
        } else {
            Some(weekly_quota_floor_basis_points)
        };
        self
    }

    /// Supplies prevalidated credit authority from one coherent state snapshot.
    #[must_use]
    pub const fn with_credit_backed_eligibility(
        mut self,
        eligibility: CreditBackedEligibility,
    ) -> Self {
        self.credit_backed_eligibility = eligibility;
        self
    }

    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the provider that owns this account.
    #[must_use]
    pub const fn provider(&self) -> Provider {
        self.provider
    }

    /// Returns the account windows.
    #[must_use]
    pub fn windows(&self) -> &[QuotaWindowFact] {
        &self.windows
    }

    /// Returns durable rejected-window facts.
    #[must_use]
    pub fn rejected_windows(&self) -> &[QuotaWindowRejectionFact] {
        &self.rejected_windows
    }

    /// Returns current active sessions.
    #[must_use]
    pub const fn current_active_sessions(&self) -> u32 {
        self.current_active_sessions
    }

    /// Returns the configured hard weekly quota floor in basis points.
    #[must_use]
    pub const fn weekly_quota_floor_basis_points(&self) -> Option<u32> {
        self.weekly_quota_floor_basis_points
    }

    /// Returns whether account metadata permits routing.
    #[must_use]
    pub const fn routing_enabled(&self) -> bool {
        self.account_enabled && self.has_active_credential
    }
}

/// Durable provider rejection fact for one quota window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaWindowRejectionFact {
    window_kind: WindowKind,
    rejected_at_unix_seconds: u64,
    reported_reset_unix_seconds: Option<u64>,
}

impl QuotaWindowRejectionFact {
    /// Creates one rejected-window fact from the D9 state projection.
    #[must_use]
    pub const fn new(
        window_kind: WindowKind,
        rejected_at_unix_seconds: u64,
        reported_reset_unix_seconds: Option<u64>,
    ) -> Self {
        Self {
            window_kind,
            rejected_at_unix_seconds,
            reported_reset_unix_seconds,
        }
    }

    /// Returns the rejected quota window.
    #[must_use]
    pub const fn window_kind(&self) -> WindowKind {
        self.window_kind
    }

    /// Returns when the provider rejection occurred.
    #[must_use]
    pub const fn rejected_at_unix_seconds(&self) -> u64 {
        self.rejected_at_unix_seconds
    }

    /// Returns the reset time reported with the rejection.
    #[must_use]
    pub const fn reported_reset_unix_seconds(&self) -> Option<u64> {
        self.reported_reset_unix_seconds
    }
}
