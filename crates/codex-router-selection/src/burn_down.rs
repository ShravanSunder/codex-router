//! Reset-aware quota burn-down assessment.

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

/// Largest configurable per-account weekly quota floor in basis points.
pub const MAX_WEEKLY_QUOTA_FLOOR_BASIS_POINTS: u32 = 1_500;

/// Protective space above an explicitly configured weekly floor.
pub const WEEKLY_QUOTA_FLOOR_CUSHION_BASIS_POINTS: u32 = 300;

#[path = "burn_down/account_assessment.rs"]
mod account_assessment;
#[path = "burn_down/assessment_input.rs"]
mod assessment_input;
#[path = "burn_down/assessment_result.rs"]
mod assessment_result;
#[path = "burn_down/candidate_priority.rs"]
mod candidate_priority;
#[path = "burn_down/credit_assessment.rs"]
mod credit_assessment;
#[path = "burn_down/route_assessment.rs"]
mod route_assessment;
#[path = "burn_down/routing_policy.rs"]
mod routing_policy;
#[path = "burn_down/routing_reasons.rs"]
mod routing_reasons;
#[path = "burn_down/window_assessment.rs"]
mod window_assessment;
#[path = "burn_down/window_facts.rs"]
mod window_facts;

pub use assessment_input::{
    BurnDownAccountInput, BurnDownRouteBandAssessmentInput, CreditBackedEligibility,
    QuotaWindowRejectionFact,
};
pub use assessment_result::{
    AccountAvailability, BurnDownAccountAssessment, BurnDownRouteBandAssessmentResult,
    LimitingWindow, QuotaEvidenceFreshness, RouteBandAssessmentStatus, RoutingExclusion,
    SelectedPool,
};
pub use route_assessment::assess_route_band;
pub use routing_policy::{BurnDownRouteBandPolicy, weekly_quota_switch_at_basis_points};
pub use routing_reasons::{QuotaEvidenceReason, RoutingReason};
pub use window_facts::{QuotaWindowFact, QuotaWindowStatus};

#[cfg(test)]
#[path = "burn_down/burn_down_tests.rs"]
mod tests;
