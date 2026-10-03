use super::*;

#[test]
fn initial_admission_restores_zero_weight_below_builtin_retirement_threshold() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_four_percent",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 4, 20 * 3_600, 0),
            ],
        ),
        account(
            "acct_healthy_peer",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 80, 5 * 86_400, 20),
            ],
        ),
    ]));
    let admitted = account_assessment(&assessment, "acct_four_percent");

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(admitted.availability(), AccountAvailability::Usable);
    assert_eq!(admitted.routing_weight(), Some(0));
    assert!(admitted.has_initial_admission_priority());
    assert_eq!(
        admitted.routing_reason(),
        RoutingReason::PreferredNearResetInitialAdmission
    );
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_four_percent")
    );
}

#[test]
fn far_idle_accounts_rank_by_reset_and_lose_priority_after_first_reservation() {
    let build = |seven_active| {
        assess_route_band(input(vec![
            account(
                "acct_seven_53h",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        7,
                        53 * 3_600,
                        82,
                    ),
                ],
            )
            .with_current_active_sessions(seven_active),
            account(
                "acct_two_97h",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        2,
                        97 * 3_600,
                        14,
                    ),
                ],
            ),
            account(
                "acct_busy_healthy",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        80,
                        5 * 86_400,
                        30,
                    ),
                ],
            )
            .with_current_active_sessions(1),
        ]))
    };
    let first = build(0);
    assert_eq!(
        first.preferred_next().map(AccountId::as_str),
        Some("acct_seven_53h")
    );
    assert_eq!(
        account_assessment(&first, "acct_seven_53h")
            .routing_reason()
            .as_str(),
        "preferred_idle_far_reset"
    );
    assert_eq!(
        account_assessment(&first, "acct_two_97h").availability(),
        AccountAvailability::Reserve
    );
    let second = build(1);
    assert_eq!(
        second.preferred_next().map(AccountId::as_str),
        Some("acct_two_97h")
    );
}

#[test]
fn far_idle_same_reset_uses_balance_confidence_then_stable_identity() {
    let candidate = |id, remaining, confidence| {
        account(
            id,
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(
                    WEEKLY,
                    remaining,
                    97 * 3_600,
                    14,
                )
                .with_burn_rate_confidence(confidence),
            ],
        )
    };
    let lower_balance = assess_route_band(input(vec![
        candidate("acct_z_lower_balance", 2, QuotaRunRateConfidence::Low),
        candidate("acct_a_higher_balance", 3, QuotaRunRateConfidence::Normal),
    ]));
    assert_eq!(
        lower_balance.preferred_next().map(AccountId::as_str),
        Some("acct_z_lower_balance")
    );

    let higher_confidence = assess_route_band(input(vec![
        candidate("acct_a_low_confidence", 2, QuotaRunRateConfidence::Low),
        candidate(
            "acct_z_normal_confidence",
            2,
            QuotaRunRateConfidence::Normal,
        ),
    ]));
    assert_eq!(
        higher_confidence.preferred_next().map(AccountId::as_str),
        Some("acct_z_normal_confidence")
    );

    let stable_identity = assess_route_band(input(vec![
        candidate("acct_z_equal", 2, QuotaRunRateConfidence::Normal),
        candidate("acct_a_equal", 2, QuotaRunRateConfidence::Normal),
    ]));
    assert_eq!(
        stable_identity.preferred_next().map(AccountId::as_str),
        Some("acct_a_equal")
    );
}

#[test]
fn far_idle_excludes_known_runway_below_nine_hundred_seconds() {
    let low_runway = account(
        "acct_a_899_second_runway",
        vec![
            window(FIVE_HOURS, 100, 4 * 3_600),
            window(WEEKLY, 2, 97 * 3_600)
                .with_projected_exhaustion_unix_seconds(NOW + 899)
                .with_per_connection_burn_basis_points_per_hour(800)
                .with_burn_rate_confidence(QuotaRunRateConfidence::Normal),
        ],
    );
    let safe_runway = account(
        "acct_z_safe_runway",
        vec![
            window(FIVE_HOURS, 100, 4 * 3_600),
            window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 3, 97 * 3_600, 14),
        ],
    );
    let assessment = assess_route_band(input(vec![low_runway, safe_runway]));
    assert!(!account_assessment(&assessment, "acct_a_899_second_runway").far_idle_priority);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_z_safe_runway")
    );
}

#[test]
fn initial_admission_keeps_floor_failed_guard_and_known_short_runway_binding() {
    let short_guard_failure = window(FIVE_HOURS, 20, 4 * 3_600)
        .with_projected_candidate_burn_basis_points_per_hour(600)
        .with_burn_rate_confidence(QuotaRunRateConfidence::Normal);
    let known_low_weekly_runway = window(WEEKLY, 20, 24 * 3_600)
        .with_per_connection_burn_basis_points_per_hour(100)
        .with_projected_exhaustion_unix_seconds(NOW + 899);
    let assessment = assess_route_band(input(vec![
        account(
            "acct_floor",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window(WEEKLY, 4, 20 * 3_600),
            ],
        )
        .with_weekly_quota_floor_basis_points(400),
        account(
            "acct_failed_short_guard",
            vec![short_guard_failure, window(WEEKLY, 20, 24 * 3_600)],
        ),
        account(
            "acct_known_899_second_runway",
            vec![window(FIVE_HOURS, 100, 4 * 3_600), known_low_weekly_runway],
        ),
        account(
            "acct_disabled",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window(WEEKLY, 20, 24 * 3_600),
            ],
        )
        .with_account_enabled(false),
        account(
            "acct_exhausted",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible)
                    .with_remaining_headroom(0)
                    .with_reset_unix_seconds(NOW + 24 * 3_600)
                    .with_observed_unix_seconds(NOW),
            ],
        ),
    ]));

    assert_eq!(
        account_assessment(&assessment, "acct_floor").routing_exclusion(),
        RoutingExclusion::WeeklyQuotaFloor
    );
    assert_eq!(
        account_assessment(&assessment, "acct_failed_short_guard").quota_evidence_reason(),
        QuotaEvidenceReason::ShortWindowGuard
    );
    assert!(
        !account_assessment(&assessment, "acct_known_899_second_runway")
            .has_initial_admission_priority()
    );
    assert!(
        assessment
            .accounts()
            .iter()
            .all(|account| !account.has_initial_admission_priority())
    );
    assert_eq!(
        account_assessment(&assessment, "acct_disabled").routing_exclusion(),
        RoutingExclusion::Disabled
    );
    assert_eq!(
        account_assessment(&assessment, "acct_exhausted").quota_evidence_reason(),
        QuotaEvidenceReason::WindowIneligible
    );
}

#[test]
fn initial_admission_horizon_is_inclusive_and_orders_reset_remaining_then_identity() {
    let candidate = |account_id_value, remaining, reset_in_seconds| {
        account(
            account_id_value,
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window(WEEKLY, remaining, reset_in_seconds),
            ],
        )
    };
    let assessment = assess_route_band(input(vec![
        candidate("acct_later", 90, DRAIN_POOL_RESET_HORIZON_SECONDS),
        candidate("acct_same_reset_lower", 20, 24 * 3_600),
        candidate("acct_same_reset_z", 30, 24 * 3_600),
        candidate("acct_same_reset_a", 30, 24 * 3_600),
        candidate(
            "acct_outside_horizon",
            100,
            DRAIN_POOL_RESET_HORIZON_SECONDS + 1,
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_same_reset_a")
    );
    assert!(account_assessment(&assessment, "acct_later").has_initial_admission_priority());
    assert!(
        !account_assessment(&assessment, "acct_outside_horizon").has_initial_admission_priority()
    );
}
