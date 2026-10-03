//! Per-window quota pressure, survival, drain, and salvage evidence.
use super::assessment_result::{LimitingWindow, QuotaEvidenceFreshness, SalvageSortKey};
use super::routing_policy::{
    BurnDownRouteBandPolicy, DEFAULT_LONG_NEAR_RESET_MAX_SECONDS, ceil_percent, clamp_u128_to_u32,
    min_u64,
};
use super::window_facts::{QuotaWindowFact, QuotaWindowStatus};
use super::{
    DRAIN_POOL_RESET_HORIZON_SECONDS, REACTIVE_RECONNECT_MIN_RUNWAY_SECONDS,
    SHORT_NEAR_RESET_THRESHOLD_SECONDS, SHORT_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS,
    V1_SHORT_WINDOW_SECONDS, V1_WEEKLY_WINDOW_SECONDS, WEEKLY_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS,
};
use crate::run_rate::QuotaRunRateConfidence;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::{WindowKind, WindowPolicy, WindowRule};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WindowAssessment {
    pub(super) window_seconds: u64,
    pub(super) remaining_headroom: u32,
    pub(super) remaining_basis_points: u32,
    pub(super) reset_unix_seconds: Option<u64>,
    pub(super) status: QuotaWindowStatus,
    pub(super) pressure: u32,
    pub(super) projected_pressure: u32,
    pub(super) projected_exhaustion_unix_seconds: Option<u64>,
    pub(super) surplus: u32,
    pub(super) time_left_seconds: Option<u64>,
    pub(super) near_reset: bool,
    pub(super) per_connection_burn_basis_points_per_hour: Option<u32>,
    pub(super) aggregate_burn_basis_points_per_hour: Option<u32>,
    pub(super) projected_candidate_burn_basis_points_per_hour: Option<u32>,
    pub(super) burn_rate_confidence: QuotaRunRateConfidence,
    pub(super) survival_margin_basis_points: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AccountDisplayMetrics {
    pub(super) short_pressure: u32,
    pub(super) long_pressure: u32,
    pub(super) short_salvage: u32,
    pub(super) long_salvage: u32,
    pub(super) projected_burn_pressure: u32,
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
    pub(super) salvage_sort_key: Option<SalvageSortKey>,
}

pub(super) fn assess_window(
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
        remaining_basis_points: window.remaining_basis_points,
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

pub(super) fn missing_profile_window(
    windows: &[WindowAssessment],
    policies: &[WindowPolicy],
) -> bool {
    policies.iter().any(|window_policy| {
        !matches!(window_policy.rule, WindowRule::LegacyOpenAi)
            && !windows.iter().any(|window| {
                window_kind_for_seconds(window.window_seconds) == Some(window_policy.kind)
            })
    })
}

fn window_kind_for_seconds(window_seconds: u64) -> Option<WindowKind> {
    match window_seconds {
        V1_SHORT_WINDOW_SECONDS => Some(WindowKind::FiveHour),
        V1_WEEKLY_WINDOW_SECONDS => Some(WindowKind::Weekly),
        _ => None,
    }
}

pub(super) fn claude_near_full_reserve(
    provider: Provider,
    windows: &[WindowAssessment],
    policies: &[WindowPolicy],
) -> bool {
    if provider != Provider::Claude {
        return false;
    }

    policies.iter().any(|window_policy| {
        let WindowRule::NearFullReserve { percent } = window_policy.rule else {
            return false;
        };
        windows
            .iter()
            .find(|window| {
                window_kind_for_seconds(window.window_seconds) == Some(window_policy.kind)
            })
            .is_some_and(|window| {
                window.status == QuotaWindowStatus::Eligible
                    && 10_000_u32.saturating_sub(window.remaining_basis_points)
                        >= u32::from(percent.get()).saturating_mul(100)
            })
    })
}

pub(super) fn missing_required_weekly_window(windows: &[WindowAssessment]) -> bool {
    !windows
        .iter()
        .any(|window| window.window_seconds == V1_WEEKLY_WINDOW_SECONDS)
}

pub(super) fn long_window_requires_reserve(
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

pub(super) fn weekly_window_is_drain_pool_candidate(window: &WindowAssessment) -> bool {
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

pub(super) fn required_active_connections_to_drain(window: &WindowAssessment) -> Option<u32> {
    let per_connection_burn_basis_points_per_hour =
        u128::from(window.per_connection_burn_basis_points_per_hour?);
    let time_left_seconds = u128::from(window.time_left_seconds?);
    if per_connection_burn_basis_points_per_hour == 0 || time_left_seconds == 0 {
        return None;
    }

    let remaining_basis_points = u128::from(window.remaining_basis_points);
    let denominator = per_connection_burn_basis_points_per_hour.saturating_mul(time_left_seconds);
    let required_connections = remaining_basis_points
        .saturating_mul(3_600)
        .div_ceil(denominator);

    Some(clamp_u128_to_u32(required_connections))
}

pub(super) fn projected_runway_seconds(window: &WindowAssessment) -> Option<u64> {
    let reset_unix_seconds = window.reset_unix_seconds?;
    let time_left_seconds = window.time_left_seconds?;
    let now_unix_seconds = reset_unix_seconds.saturating_sub(time_left_seconds);
    Some(
        window
            .projected_exhaustion_unix_seconds?
            .saturating_sub(now_unix_seconds),
    )
}

pub(super) fn short_window_fails_guard(
    windows: &[WindowAssessment],
    policy: BurnDownRouteBandPolicy,
) -> bool {
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

pub(super) fn freshness_for_windows(windows: &[WindowAssessment]) -> QuotaEvidenceFreshness {
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

pub(super) fn limiting_window(windows: &[WindowAssessment]) -> Option<LimitingWindow> {
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

pub(super) fn salvage_sort_key(
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

pub(super) fn weekly_window_survives_to_reset(window: &WindowAssessment) -> bool {
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

pub(super) fn survival_margin_basis_points(
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
    let remaining_basis_points = i128::from(window.remaining_basis_points);
    let margin = remaining_basis_points - i128::try_from(projected_burn_basis_points).ok()?;

    Some(margin.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64)
}

pub(super) const fn is_short_window(window_seconds: u64, policy: BurnDownRouteBandPolicy) -> bool {
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
