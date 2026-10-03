//! Credit-backed reserve admission after known included Responses quota exhaustion.

use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;

use super::BurnDownAccountInput;
use super::CreditBackedEligibility;
use super::QuotaWindowStatus;
use super::V1_SHORT_WINDOW_SECONDS;
use super::V1_WEEKLY_WINDOW_SECONDS;
use super::WindowAssessment;
use super::assess_window;
use super::missing_required_weekly_window;

pub(super) fn canonical_responses_compact_window_assessments(
    input: &BurnDownAccountInput,
    route_band: RouteBand,
    own_windows: &[WindowAssessment],
    now_unix_seconds: u64,
    policy: super::BurnDownRouteBandPolicy,
) -> Option<Vec<WindowAssessment>> {
    if route_band != RouteBand::ResponsesCompact
        || input.provider != Provider::Openai
        || !compact_own_windows_need_canonical_responses(own_windows)
    {
        return None;
    }

    let canonical_windows = input.canonical_responses_windows.as_deref()?;
    let assessed_windows = canonical_windows
        .iter()
        .map(|window| assess_window(window, now_unix_seconds, policy))
        .collect::<Vec<_>>();
    if !canonical_responses_windows_have_known_included_or_exhausted_quota(&assessed_windows) {
        return None;
    }

    Some(assessed_windows)
}

fn compact_own_windows_need_canonical_responses(windows: &[WindowAssessment]) -> bool {
    windows.is_empty()
        || windows.iter().any(|window| {
            matches!(
                window.status,
                QuotaWindowStatus::Unknown | QuotaWindowStatus::Stale
            )
        })
        || (missing_required_weekly_window(windows)
            && !windows.iter().any(|window| {
                window.status == QuotaWindowStatus::Eligible && window.remaining_basis_points > 0
            }))
}

pub(super) fn credit_backed_candidate_is_eligible(
    input: &BurnDownAccountInput,
    route_band: RouteBand,
    windows: &[WindowAssessment],
    canonical_compact_windows_used: bool,
) -> bool {
    (route_band == RouteBand::Responses
        || (route_band == RouteBand::ResponsesCompact && canonical_compact_windows_used))
        && input.provider == Provider::Openai
        && input.credit_backed_eligibility == CreditBackedEligibility::Eligible
        && input.weekly_quota_floor_basis_points.is_none()
        && !windows.is_empty()
        && windows.iter().any(|window| {
            matches!(
                window.window_seconds,
                V1_SHORT_WINDOW_SECONDS | V1_WEEKLY_WINDOW_SECONDS
            ) && window.remaining_basis_points == 0
                && matches!(
                    window.status,
                    QuotaWindowStatus::Eligible | QuotaWindowStatus::Ineligible
                )
        })
        && windows.iter().all(|window| match window.status {
            QuotaWindowStatus::Eligible => true,
            QuotaWindowStatus::Ineligible => {
                matches!(
                    window.window_seconds,
                    V1_SHORT_WINDOW_SECONDS | V1_WEEKLY_WINDOW_SECONDS
                ) && window.remaining_basis_points == 0
            }
            QuotaWindowStatus::Stale | QuotaWindowStatus::Unknown => false,
        })
        && windows
            .iter()
            .all(|window| window.reset_unix_seconds.is_some())
}

fn canonical_responses_windows_have_known_included_or_exhausted_quota(
    windows: &[WindowAssessment],
) -> bool {
    !windows.is_empty()
        && !missing_required_weekly_window(windows)
        && windows.iter().all(|window| match window.status {
            QuotaWindowStatus::Eligible => true,
            QuotaWindowStatus::Ineligible => window.remaining_basis_points == 0,
            QuotaWindowStatus::Stale | QuotaWindowStatus::Unknown => false,
        })
}
