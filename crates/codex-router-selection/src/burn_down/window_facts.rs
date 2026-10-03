//! Public quota-window input facts and status.
use super::routing_policy::clamp_u32;
use crate::run_rate::QuotaRunRateConfidence;

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

/// Pure fact for one provider quota window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaWindowFact {
    pub(super) window_seconds: u64,
    pub(super) status: QuotaWindowStatus,
    pub(super) remaining_headroom: u32,
    pub(super) remaining_basis_points: u32,
    pub(super) reset_unix_seconds: Option<u64>,
    pub(super) observed_unix_seconds: u64,
    pub(super) effective: bool,
    pub(super) projected_exhaustion_unix_seconds: Option<u64>,
    pub(super) per_connection_burn_basis_points_per_hour: Option<u32>,
    pub(super) aggregate_burn_basis_points_per_hour: Option<u32>,
    pub(super) projected_candidate_burn_basis_points_per_hour: Option<u32>,
    pub(super) burn_rate_confidence: QuotaRunRateConfidence,
}

impl QuotaWindowFact {
    /// Creates a quota window fact.
    #[must_use]
    pub const fn new(window_seconds: u64, status: QuotaWindowStatus) -> Self {
        Self {
            window_seconds,
            status,
            remaining_headroom: 0,
            remaining_basis_points: 0,
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
        self.remaining_basis_points = self.remaining_headroom.saturating_mul(100);
        self
    }

    /// Sets exact remaining quota in basis points.
    #[must_use]
    pub const fn with_remaining_basis_points(mut self, remaining_basis_points: u32) -> Self {
        self.remaining_basis_points = clamp_u32(remaining_basis_points, 0, 10_000);
        self.remaining_headroom = self.remaining_basis_points / 100;
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

    /// Returns quota evidence status.
    #[must_use]
    pub const fn status(&self) -> QuotaWindowStatus {
        self.status
    }

    /// Returns remaining headroom.
    #[must_use]
    pub const fn remaining_headroom(&self) -> u32 {
        self.remaining_headroom
    }

    /// Returns exact remaining quota in basis points.
    #[must_use]
    pub const fn remaining_basis_points(&self) -> u32 {
        self.remaining_basis_points
    }

    /// Returns reset time.
    #[must_use]
    pub const fn reset_unix_seconds(&self) -> Option<u64> {
        self.reset_unix_seconds
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
