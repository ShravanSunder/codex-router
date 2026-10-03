//! Deterministic weekly-quota candidate ordering and same-pool ties.
use super::assessment_result::BurnDownAccountAssessment;
use super::{
    ACTIVE_SESSION_IMBALANCE_THRESHOLD, SAME_POOL_PROJECTED_RUNOUT_TOLERANCE_SECONDS,
    SAME_POOL_RESET_TOLERANCE_SECONDS, SAME_POOL_SURVIVAL_MARGIN_TOLERANCE_BASIS_POINTS,
};
use crate::run_rate::QuotaRunRateConfidence;

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

pub(super) fn candidate_priority_cmp(
    left: &BurnDownAccountAssessment,
    left_weight: u32,
    right: &BurnDownAccountAssessment,
    right_weight: u32,
) -> std::cmp::Ordering {
    left.credit_backed
        .cmp(&right.credit_backed)
        .then_with(|| compare_initial_admission(left, right))
        .then_with(|| compare_weekly_drain_pool(left, right))
        .then_with(|| right.far_idle_priority.cmp(&left.far_idle_priority))
        .then_with(|| compare_drain_pool_confidence(left, right))
        .then_with(|| compare_projected_drain_gap(left, right))
        .then_with(|| compare_weekly_survival(left, right))
        .then_with(|| right_weight.cmp(&left_weight))
        .then_with(|| left.long_pressure.cmp(&right.long_pressure))
        .then_with(|| left.short_pressure.cmp(&right.short_pressure))
        .then_with(|| compare_salvage_key(left, right))
        .then_with(|| left.account_id.cmp(&right.account_id))
}

fn compare_initial_admission(
    left: &BurnDownAccountAssessment,
    right: &BurnDownAccountAssessment,
) -> std::cmp::Ordering {
    match (
        left.initial_admission_priority,
        right.initial_admission_priority,
    ) {
        (true, false) => return std::cmp::Ordering::Less,
        (false, true) => return std::cmp::Ordering::Greater,
        (false, false) => return std::cmp::Ordering::Equal,
        (true, true) => {}
    }

    left.weekly_reset_unix_seconds
        .cmp(&right.weekly_reset_unix_seconds)
        .then_with(|| {
            right
                .weekly_remaining_headroom
                .cmp(&left.weekly_remaining_headroom)
        })
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

pub(super) const fn confidence_rank(confidence: QuotaRunRateConfidence) -> u8 {
    match confidence {
        QuotaRunRateConfidence::Normal => 4,
        QuotaRunRateConfidence::Low => 3,
        QuotaRunRateConfidence::Insufficient => 2,
        QuotaRunRateConfidence::Unknown => 1,
        QuotaRunRateConfidence::Stale => 0,
    }
}
