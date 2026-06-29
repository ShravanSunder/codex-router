//! Historical quota run-rate estimation.

/// Minimum span required for normal-confidence slope.
pub const NORMAL_CONFIDENCE_MIN_SPAN_SECONDS: u64 = 900;

/// Confidence state for historical quota burn estimates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaRunRateConfidence {
    /// No same-segment observations exist.
    Unknown,
    /// One same-segment observation exists.
    Insufficient,
    /// Two same-segment observations, or three observations below the normal span.
    Low,
    /// Three or more same-segment observations span at least fifteen minutes.
    Normal,
    /// Latest same-segment observation is outside the configured freshness window.
    Stale,
}

/// One persisted quota observation for a single account/route/window segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaRunRateObservation {
    observed_unix_seconds: u64,
    reset_unix_seconds: u64,
    remaining_headroom_percent: u32,
}

impl QuotaRunRateObservation {
    /// Creates a quota run-rate observation.
    #[must_use]
    pub const fn new(
        observed_unix_seconds: u64,
        reset_unix_seconds: u64,
        remaining_headroom_percent: u32,
    ) -> Self {
        Self {
            observed_unix_seconds,
            reset_unix_seconds,
            remaining_headroom_percent: clamp_percent(remaining_headroom_percent),
        }
    }

    /// Returns the observation time.
    #[must_use]
    pub const fn observed_unix_seconds(self) -> u64 {
        self.observed_unix_seconds
    }

    /// Returns the reset segment.
    #[must_use]
    pub const fn reset_unix_seconds(self) -> u64 {
        self.reset_unix_seconds
    }

    /// Returns remaining headroom percent.
    #[must_use]
    pub const fn remaining_headroom_percent(self) -> u32 {
        self.remaining_headroom_percent
    }
}

/// Historical quota burn estimate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaRunRateEstimate {
    confidence: QuotaRunRateConfidence,
    burn_rate_basis_points_per_hour: Option<u32>,
    latest_remaining_headroom_percent: Option<u32>,
}

impl QuotaRunRateEstimate {
    /// Creates an unknown estimate.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            confidence: QuotaRunRateConfidence::Unknown,
            burn_rate_basis_points_per_hour: None,
            latest_remaining_headroom_percent: None,
        }
    }

    /// Creates an insufficient estimate.
    #[must_use]
    pub const fn insufficient() -> Self {
        Self {
            confidence: QuotaRunRateConfidence::Insufficient,
            burn_rate_basis_points_per_hour: None,
            latest_remaining_headroom_percent: None,
        }
    }

    /// Creates a stale estimate.
    #[must_use]
    pub const fn stale() -> Self {
        Self {
            confidence: QuotaRunRateConfidence::Stale,
            burn_rate_basis_points_per_hour: None,
            latest_remaining_headroom_percent: None,
        }
    }

    /// Creates an estimate with burn rate and latest headroom.
    #[must_use]
    pub const fn with_rate(
        confidence: QuotaRunRateConfidence,
        burn_rate_percent_per_hour: u32,
        latest_remaining_headroom_percent: u32,
    ) -> Self {
        Self::with_rate_basis_points_per_hour(
            confidence,
            burn_rate_percent_per_hour.saturating_mul(100),
            latest_remaining_headroom_percent,
        )
    }

    /// Creates an estimate with a basis-point burn rate and latest headroom.
    #[must_use]
    pub const fn with_rate_basis_points_per_hour(
        confidence: QuotaRunRateConfidence,
        burn_rate_basis_points_per_hour: u32,
        latest_remaining_headroom_percent: u32,
    ) -> Self {
        Self {
            confidence,
            burn_rate_basis_points_per_hour: Some(burn_rate_basis_points_per_hour),
            latest_remaining_headroom_percent: Some(clamp_percent(
                latest_remaining_headroom_percent,
            )),
        }
    }

    /// Returns confidence.
    #[must_use]
    pub const fn confidence(self) -> QuotaRunRateConfidence {
        self.confidence
    }

    /// Returns burn rate percent per hour when available.
    #[must_use]
    pub const fn burn_rate_percent_per_hour(self) -> Option<u32> {
        match self.burn_rate_basis_points_per_hour {
            Some(burn_rate_basis_points_per_hour) => Some(burn_rate_basis_points_per_hour / 100),
            None => None,
        }
    }

    /// Returns burn rate basis points per hour when available.
    #[must_use]
    pub const fn burn_rate_basis_points_per_hour(self) -> Option<u32> {
        self.burn_rate_basis_points_per_hour
    }

    /// Returns latest remaining headroom percent when available.
    #[must_use]
    pub const fn latest_remaining_headroom_percent(self) -> Option<u32> {
        self.latest_remaining_headroom_percent
    }

    /// Projects when the window reaches zero headroom.
    #[must_use]
    pub fn projected_exhaustion_unix_seconds(self, now_unix_seconds: u64) -> Option<u64> {
        let burn_rate_basis_points_per_hour = u64::from(self.burn_rate_basis_points_per_hour?);
        if burn_rate_basis_points_per_hour == 0 {
            return None;
        }
        let latest_remaining_headroom_percent = u64::from(self.latest_remaining_headroom_percent?);
        let latest_remaining_headroom_basis_points =
            latest_remaining_headroom_percent.saturating_mul(100);
        let seconds_until_exhaustion = latest_remaining_headroom_basis_points
            .saturating_mul(3_600)
            .checked_div(burn_rate_basis_points_per_hour)?;

        Some(now_unix_seconds.saturating_add(seconds_until_exhaustion))
    }
}

/// Estimates historical quota burn from same-reset-segment observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaRunRateEstimator {
    freshness_window_seconds: u64,
}

impl QuotaRunRateEstimator {
    /// Creates an estimator.
    #[must_use]
    pub const fn new(freshness_window_seconds: u64) -> Self {
        Self {
            freshness_window_seconds,
        }
    }

    /// Estimates quota burn using observations from the current reset segment only.
    #[must_use]
    pub fn estimate(
        self,
        now_unix_seconds: u64,
        reset_unix_seconds: u64,
        observations: &[QuotaRunRateObservation],
    ) -> QuotaRunRateEstimate {
        let mut segment_observations = observations
            .iter()
            .copied()
            .filter(|observation| observation.reset_unix_seconds == reset_unix_seconds)
            .filter(|observation| observation.observed_unix_seconds <= now_unix_seconds)
            .collect::<Vec<_>>();
        segment_observations.sort_by_key(|observation| observation.observed_unix_seconds);

        let Some(latest_observation) = segment_observations.last().copied() else {
            return QuotaRunRateEstimate::unknown();
        };
        if now_unix_seconds.saturating_sub(latest_observation.observed_unix_seconds)
            > self.freshness_window_seconds
        {
            return QuotaRunRateEstimate::stale();
        }
        if segment_observations.len() == 1 {
            return QuotaRunRateEstimate::insufficient();
        }

        let first_observation = segment_observations[0];
        let span_seconds = latest_observation
            .observed_unix_seconds
            .saturating_sub(first_observation.observed_unix_seconds);
        if span_seconds == 0 {
            return QuotaRunRateEstimate::with_rate(
                QuotaRunRateConfidence::Low,
                0,
                latest_observation.remaining_headroom_percent,
            );
        }

        let burned_percent = first_observation
            .remaining_headroom_percent
            .saturating_sub(latest_observation.remaining_headroom_percent);
        let burn_rate_basis_points_per_hour = u64::from(burned_percent)
            .saturating_mul(100)
            .saturating_mul(3_600)
            .checked_div(span_seconds)
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32;
        let confidence = if segment_observations.len() >= 3
            && span_seconds >= NORMAL_CONFIDENCE_MIN_SPAN_SECONDS
        {
            QuotaRunRateConfidence::Normal
        } else {
            QuotaRunRateConfidence::Low
        };

        QuotaRunRateEstimate::with_rate_basis_points_per_hour(
            confidence,
            burn_rate_basis_points_per_hour,
            latest_observation.remaining_headroom_percent,
        )
    }
}

const fn clamp_percent(value: u32) -> u32 {
    if value > 100 { 100 } else { value }
}
