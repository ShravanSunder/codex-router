//! Fixed route-band quota policy and bounded numeric calculations.
use super::WEEKLY_QUOTA_FLOOR_CUSHION_BASIS_POINTS;
use codex_router_core::route_profile::{RouteProfile, WindowKind, WindowRule};
use codex_router_core::routes::RouteBand;

/// Graceful switch point above a configured hard floor.
#[must_use]
pub fn weekly_quota_switch_at_basis_points(configured_floor: Option<u32>) -> Option<u32> {
    configured_floor.map(|floor| {
        floor
            .saturating_add(WEEKLY_QUOTA_FLOOR_CUSHION_BASIS_POINTS)
            .min(10_000)
    })
}

pub(super) fn weekly_floor_switch_at_basis_points(
    configured_floor: u32,
    route_profile: &RouteProfile,
) -> Option<u32> {
    let weekly_rule = route_profile
        .windows
        .iter()
        .find(|window_policy| window_policy.kind == WindowKind::Weekly)?
        .rule;
    match weekly_rule {
        WindowRule::LegacyOpenAi => weekly_quota_switch_at_basis_points(Some(configured_floor)),
        WindowRule::WeeklyFloor { early_switch_bps } => Some(
            configured_floor
                .saturating_add(u32::from(early_switch_bps))
                .min(10_000),
        ),
        WindowRule::NearFullReserve { .. } => None,
    }
}

/// Fixed v1 route-band policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BurnDownRouteBandPolicy {
    pub(super) short_window_cutoff_seconds: u64,
    pub(super) reserve_pressure_threshold: u32,
    pub(super) reserve_headroom_threshold: u32,
    pub(super) long_pressure_multiplier: u32,
    pub(super) short_salvage_cap: u32,
    pub(super) long_salvage_cap: u32,
    pub(super) risk_penalty_cap: u32,
    pub(super) selectable_weight_min: u32,
    pub(super) selectable_weight_max: u32,
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

pub(super) fn ceil_percent(numerator: u64, denominator: u64) -> u32 {
    if denominator == 0 {
        return 0;
    }
    let scaled = u128::from(numerator) * 100;
    scaled.div_ceil(u128::from(denominator)) as u32
}

pub(super) const fn clamp_i64(value: i64, min: u32, max: u32) -> u32 {
    if value < min as i64 {
        min
    } else if value > max as i64 {
        max
    } else {
        value as u32
    }
}

pub(super) const fn clamp_u32(value: u32, min: u32, max: u32) -> u32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

pub(super) fn clamp_u128_to_u32(value: u128) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub(super) const fn min_u64(left: u64, right: u64) -> u64 {
    if left < right { left } else { right }
}

pub(super) const fn policy_for_route_band(_route_band: RouteBand) -> BurnDownRouteBandPolicy {
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

pub(super) const DEFAULT_SHORT_WINDOW_CUTOFF_SECONDS: u64 = 86_400;

pub(super) const DEFAULT_LONG_NEAR_RESET_MAX_SECONDS: u64 = 43_200;

pub(super) const DEFAULT_RESERVE_PRESSURE_THRESHOLD: u32 = 25;

pub(super) const DEFAULT_RESERVE_HEADROOM_THRESHOLD: u32 = 10;

pub(super) const DEFAULT_LONG_PRESSURE_MULTIPLIER: u32 = 3;

pub(super) const DEFAULT_SHORT_SALVAGE_CAP: u32 = 10;

pub(super) const DEFAULT_LONG_SALVAGE_CAP: u32 = 20;

pub(super) const DEFAULT_RISK_PENALTY_CAP: u32 = 90;

pub(super) const DEFAULT_SELECTABLE_WEIGHT_MIN: u32 = 0;

pub(super) const DEFAULT_SELECTABLE_WEIGHT_MAX: u32 = 100;

pub(super) const DEFAULT_UNKNOWN_FALLBACK_WEIGHT: u32 = 1;
