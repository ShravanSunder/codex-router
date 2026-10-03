//! Shared account and quota-window test builders.

use super::*;

pub(super) const NOW: u64 = 1_700_000_000;

pub(super) const FIVE_HOURS: u64 = V1_SHORT_WINDOW_SECONDS;

pub(super) const WEEKLY: u64 = V1_WEEKLY_WINDOW_SECONDS;

pub(super) fn input(accounts: Vec<BurnDownAccountInput>) -> BurnDownRouteBandAssessmentInput {
    BurnDownRouteBandAssessmentInput::new(RouteBand::Responses, NOW, RESPONSES_HTTP, accounts)
}

pub(super) fn account(
    account_id_value: &str,
    windows: Vec<QuotaWindowFact>,
) -> BurnDownAccountInput {
    BurnDownAccountInput::new(
        account_id(account_id_value),
        account_id_value,
        Provider::Openai,
        windows,
    )
}

pub(super) fn window(
    window_seconds: u64,
    remaining_headroom: u32,
    resets_in_seconds: u64,
) -> QuotaWindowFact {
    QuotaWindowFact::new(window_seconds, QuotaWindowStatus::Eligible)
        .with_remaining_headroom(remaining_headroom)
        .with_reset_unix_seconds(NOW + resets_in_seconds)
        .with_observed_unix_seconds(NOW)
}

pub(super) fn stale_window(
    window_seconds: u64,
    remaining_headroom: u32,
    resets_in_seconds: u64,
) -> QuotaWindowFact {
    QuotaWindowFact::new(window_seconds, QuotaWindowStatus::Stale)
        .with_remaining_headroom(remaining_headroom)
        .with_reset_unix_seconds(NOW + resets_in_seconds)
        .with_observed_unix_seconds(NOW)
}

pub(super) fn projected_window(
    window_seconds: u64,
    remaining_headroom: u32,
    resets_in_seconds: u64,
    projected_runout_in_seconds: u64,
) -> QuotaWindowFact {
    window(window_seconds, remaining_headroom, resets_in_seconds)
        .with_projected_exhaustion_unix_seconds(NOW + projected_runout_in_seconds)
}

pub(super) fn window_with_per_connection_burn_basis_points_per_hour(
    window_seconds: u64,
    remaining_headroom: u32,
    resets_in_seconds: u64,
    burn_rate_basis_points_per_hour: u32,
) -> QuotaWindowFact {
    if burn_rate_basis_points_per_hour == 0 {
        return window(window_seconds, remaining_headroom, resets_in_seconds)
            .with_per_connection_burn_basis_points_per_hour(burn_rate_basis_points_per_hour);
    }

    let remaining_basis_points = u64::from(remaining_headroom) * 100;
    let runout_seconds = remaining_basis_points
        .saturating_mul(3_600)
        .checked_div(u64::from(burn_rate_basis_points_per_hour))
        .unwrap_or(u64::MAX);
    projected_window(
        window_seconds,
        remaining_headroom,
        resets_in_seconds,
        runout_seconds,
    )
    .with_per_connection_burn_basis_points_per_hour(burn_rate_basis_points_per_hour)
}

pub(super) fn window_with_projected_burn_for_proposed_connection(
    window_seconds: u64,
    remaining_headroom: u32,
    resets_in_seconds: u64,
    burn_rate_basis_points_per_hour: u32,
    current_active_sessions: u32,
) -> QuotaWindowFact {
    let projected_connections = u64::from(current_active_sessions).saturating_add(1);
    let aggregate_burn_basis_points_per_hour =
        u64::from(burn_rate_basis_points_per_hour).saturating_mul(projected_connections);
    let remaining_basis_points = u64::from(remaining_headroom).saturating_mul(100);
    let runout_seconds = remaining_basis_points
        .saturating_mul(3_600)
        .checked_div(aggregate_burn_basis_points_per_hour)
        .unwrap_or(u64::MAX);
    projected_window(
        window_seconds,
        remaining_headroom,
        resets_in_seconds,
        runout_seconds,
    )
    .with_per_connection_burn_basis_points_per_hour(burn_rate_basis_points_per_hour)
    .with_projected_candidate_burn_basis_points_per_hour(
        u32::try_from(aggregate_burn_basis_points_per_hour).unwrap_or(u32::MAX),
    )
}

pub(super) const fn hours_minutes(hours: u64, minutes: u64) -> u64 {
    hours * 3_600 + minutes * 60
}

pub(super) const fn matches_projected_weekly_runout(active_sessions: u32) -> u64 {
    match active_sessions {
        0 => hours_minutes(15, 5),
        1 => hours_minutes(7, 32),
        2 => hours_minutes(5, 2),
        _ => hours_minutes(3, 46),
    }
}

pub(super) const fn askluna_projected_weekly_runout(active_sessions: u32) -> u64 {
    match active_sessions {
        0 | 1 => hours_minutes(5, 7),
        _ => hours_minutes(3, 25),
    }
}

pub(super) const fn ssdev_projected_weekly_runout(active_sessions: u32) -> u64 {
    match active_sessions {
        0 => 24 * 3_600,
        1 => hours_minutes(17, 20),
        2 => hours_minutes(13, 0),
        _ => hours_minutes(10, 24),
    }
}

pub(super) fn account_assessment<'a>(
    assessment: &'a BurnDownRouteBandAssessmentResult,
    account_id_value: &str,
) -> &'a BurnDownAccountAssessment {
    assessment
        .accounts()
        .iter()
        .find(|account| account.account_id().as_str() == account_id_value)
        .unwrap_or_else(|| panic!("missing account assessment: {account_id_value}"))
}

pub(super) fn assert_account(
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

pub(super) fn account_id(value: &str) -> AccountId {
    AccountId::new(value).unwrap_or_else(|error| panic!("account id should parse: {error}"))
}
