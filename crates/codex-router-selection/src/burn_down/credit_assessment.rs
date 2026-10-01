//! Credit-backed reserve admission after known included Responses quota exhaustion.

use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;

use super::BurnDownAccountInput;
use super::CreditBackedEligibility;
use super::QuotaWindowStatus;
use super::V1_SHORT_WINDOW_SECONDS;
use super::V1_WEEKLY_WINDOW_SECONDS;
use super::WindowAssessment;

pub(super) fn credit_backed_candidate_is_eligible(
    input: &BurnDownAccountInput,
    route_band: RouteBand,
    windows: &[WindowAssessment],
) -> bool {
    route_band == RouteBand::Responses
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
