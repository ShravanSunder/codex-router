use super::*;

#[test]
fn same_weekly_pool_uses_active_session_imbalance_before_far_reset_reserve_a5() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 19, 45 * 3_600, 0),
            ],
        )
        .with_current_active_sessions(6),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 18, 46 * 3_600, 0),
            ],
        )
        .with_current_active_sessions(0),
        account(
            "acct_c",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 34, 107 * 3_600, 20),
            ],
        )
        .with_current_active_sessions(0),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_b"),
        "A5: same low-weekly reset pool shares sessions before far-reset reserve"
    );
}

#[test]
fn confidence_tier_gates_before_active_count_tie_a6() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 40, 48 * 3_600, 50)
                    .with_burn_rate_confidence(QuotaRunRateConfidence::Normal),
            ],
        )
        .with_current_active_sessions(4),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 40, 48 * 3_600, 50)
                    .with_burn_rate_confidence(QuotaRunRateConfidence::Low),
            ],
        )
        .with_current_active_sessions(0),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a"),
        "A6: higher confidence tier gates before active-count balancing"
    );
}

#[test]
fn short_window_guard_holds_account_projected_to_stall_before_reset_f1() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window_with_per_connection_burn_basis_points_per_hour(
                    FIVE_HOURS,
                    2,
                    4 * 3_600,
                    100,
                ),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 80, 4 * 86_400, 20),
            ],
        ),
        account(
            "acct_b",
            vec![
                window_with_per_connection_burn_basis_points_per_hour(
                    FIVE_HOURS,
                    30,
                    4 * 3_600,
                    100,
                ),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 40, 4 * 86_400, 20),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_b"),
        "F1: A fails the 5h flow guard even though its weekly quota is healthier"
    );
    assert_eq!(
        account_assessment(&assessment, "acct_a").routing_reason(),
        RoutingReason::HeldShortWindowGuard
    );
    assert_account(&assessment, "acct_a", AccountAvailability::Blocked, None);
}

#[test]
fn short_window_guard_is_last_resort_when_no_better_candidate_exists() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_guarded",
            vec![
                window_with_per_connection_burn_basis_points_per_hour(
                    FIVE_HOURS,
                    2,
                    4 * 3_600,
                    100,
                ),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 80, 4 * 86_400, 20),
            ],
        ),
        account(
            "acct_empty",
            vec![
                QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Eligible)
                    .with_remaining_headroom(0)
                    .with_reset_unix_seconds(NOW + 4 * 3_600),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Eligible)
                    .with_remaining_headroom(0)
                    .with_reset_unix_seconds(NOW + 4 * 86_400),
            ],
        ),
        account(
            "acct_ineligible",
            vec![
                QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible),
            ],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::LastResort);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_guarded")
    );
    assert_eq!(assessment.weighted_candidates().len(), 1);
    assert_eq!(
        assessment.weighted_candidates()[0].0.as_str(),
        "acct_guarded"
    );
    assert_account(
        &assessment,
        "acct_guarded",
        AccountAvailability::Blocked,
        Some(0),
    );
    assert_eq!(
        account_assessment(&assessment, "acct_guarded").routing_reason(),
        RoutingReason::PreferredLastResortShortWindowGuard
    );
    assert_eq!(
        account_assessment(&assessment, "acct_empty").routing_reason(),
        RoutingReason::BlockedWindowExhausted
    );
    assert_eq!(
        account_assessment(&assessment, "acct_ineligible").routing_reason(),
        RoutingReason::BlockedWindowIneligible
    );
}

#[test]
fn short_window_guard_allows_near_reset_within_buffer_f2() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window_with_per_connection_burn_basis_points_per_hour(FIVE_HOURS, 2, 10 * 60, 100),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 80, 4 * 86_400, 20),
            ],
        )
        .with_current_active_sessions(1),
        account(
            "acct_b",
            vec![
                window_with_per_connection_burn_basis_points_per_hour(
                    FIVE_HOURS,
                    30,
                    4 * 3_600,
                    100,
                ),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 40, 4 * 86_400, 20),
            ],
        )
        .with_current_active_sessions(1),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a"),
        "F2: A can remain eligible because the 5h reset is near and inside the safety buffer"
    );
}
