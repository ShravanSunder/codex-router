use super::*;

#[test]
fn account_assessment_uses_safe_display_label() {
    let assessment = assess_route_band(input(vec![BurnDownAccountInput::new(
        account_id("acct_secret"),
        "person@example.com",
        Provider::Openai,
        vec![
            window(FIVE_HOURS, 80, 4 * 3_600),
            window(WEEKLY, 80, 5 * 86_400),
        ],
    )]));

    let account = account_assessment(&assessment, "acct_secret");
    assert!(account.account_label().starts_with("acct-"));
    assert!(!account.account_label().contains("person"));
    assert!(!account.account_label().contains('@'));
}

#[test]
fn weekly_quota_floor_blocks_at_or_below_configured_floor() {
    let cases = [
        (4, RoutingExclusion::WeeklyQuotaFloor, false),
        (5, RoutingExclusion::WeeklyQuotaFloor, false),
        (8, RoutingExclusion::None, true),
        (9, RoutingExclusion::None, true),
    ];

    for (remaining_percent, expected_exclusion, expected_selectable) in cases {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_protected",
                vec![
                    window(FIVE_HOURS, 90, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        remaining_percent,
                        3_600,
                        0,
                    ),
                ],
            )
            .with_weekly_quota_floor_basis_points(500),
        ]));
        let protected = account_assessment(&assessment, "acct_protected");

        assert_eq!(protected.routing_exclusion(), expected_exclusion);
        assert_eq!(assessment.preferred_next().is_some(), expected_selectable);
    }
}

#[test]
fn weekly_quota_floor_supports_fifteen_percent_and_rejects_above_maximum() {
    for (remaining_percent, expected_exclusion) in [
        (15, RoutingExclusion::WeeklyQuotaFloor),
        (18, RoutingExclusion::None),
        (19, RoutingExclusion::None),
    ] {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_fifteen_percent_floor",
                vec![
                    window(FIVE_HOURS, 90, 4 * 3_600),
                    window(WEEKLY, remaining_percent, 3_600),
                ],
            )
            .with_weekly_quota_floor_basis_points(1_500),
        ]));

        assert_eq!(
            account_assessment(&assessment, "acct_fifteen_percent_floor").routing_exclusion(),
            expected_exclusion
        );
    }

    let invalid = assess_route_band(input(vec![
        account(
            "acct_invalid_above_fifteen",
            vec![
                window(FIVE_HOURS, 90, 4 * 3_600),
                window(WEEKLY, 100, 3_600),
            ],
        )
        .with_weekly_quota_floor_basis_points(1_501),
    ]));
    assert_eq!(
        account_assessment(&invalid, "acct_invalid_above_fifteen").routing_exclusion(),
        RoutingExclusion::WeeklyQuotaFloor
    );
}

#[test]
fn weekly_quota_floor_ignores_projected_margin_above_current_threshold() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_protected",
            vec![
                window(FIVE_HOURS, 90, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 48, 5 * 86_400, 115),
            ],
        )
        .with_weekly_quota_floor_basis_points(1_000),
    ]));
    let protected = account_assessment(&assessment, "acct_protected");

    assert!(
        protected
            .weekly_survival_margin_basis_points()
            .is_some_and(|margin| margin < 1_000)
    );
    assert_eq!(protected.routing_exclusion(), RoutingExclusion::None);
    assert!(assessment.preferred_next().is_some());
}

#[test]
fn weekly_quota_floor_fails_closed_for_missing_stale_or_unknown_weekly_evidence() {
    let protected_accounts = vec![
        account("acct_missing", vec![window(FIVE_HOURS, 90, 4 * 3_600)])
            .with_weekly_quota_floor_basis_points(500),
        account(
            "acct_stale",
            vec![
                window(FIVE_HOURS, 90, 4 * 3_600),
                stale_window(WEEKLY, 80, 3_600).with_per_connection_burn_basis_points_per_hour(0),
            ],
        )
        .with_weekly_quota_floor_basis_points(500),
        account(
            "acct_unknown",
            vec![
                window(FIVE_HOURS, 90, 4 * 3_600),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Unknown)
                    .with_remaining_headroom(80)
                    .with_reset_unix_seconds(NOW + 3_600)
                    .with_observed_unix_seconds(NOW)
                    .with_per_connection_burn_basis_points_per_hour(0),
            ],
        )
        .with_weekly_quota_floor_basis_points(500),
    ];

    let assessment = assess_route_band(input(protected_accounts));

    assert_eq!(assessment.selected_pool(), SelectedPool::None);
    assert!(assessment.weighted_candidates().is_empty());
    for account_id_value in ["acct_missing", "acct_stale", "acct_unknown"] {
        let protected = account_assessment(&assessment, account_id_value);
        assert_eq!(
            protected.routing_exclusion(),
            RoutingExclusion::WeeklyQuotaFloor
        );
        assert_eq!(
            protected.routing_reason(),
            RoutingReason::ExcludedWeeklyQuotaFloor
        );
    }
}

#[test]
fn missing_or_zero_weekly_quota_floor_preserves_existing_two_percent_assessment() {
    let unprotected = account(
        "acct_legacy",
        vec![
            window(FIVE_HOURS, 90, 4 * 3_600),
            window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 2, 3_600, 50),
        ],
    );
    let without_policy = assess_route_band(input(vec![unprotected.clone()]));
    let disabled_policy = assess_route_band(input(vec![
        unprotected.with_weekly_quota_floor_basis_points(0),
    ]));

    assert_eq!(without_policy, disabled_policy);
}

#[test]
fn configured_floor_switch_band_keeps_two_percent_without_a_peer() {
    let account_input = account(
        "acct_protected",
        vec![
            window(FIVE_HOURS, 90, 4 * 3_600),
            window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 2, 3_600, 50),
        ],
    );
    let without_policy = assess_route_band(input(vec![account_input.clone()]));
    let protected_at_two_percent = assess_route_band(input(vec![
        account_input.with_weekly_quota_floor_basis_points(100),
    ]));

    assert_eq!(
        account_assessment(&without_policy, "acct_protected").routing_exclusion(),
        RoutingExclusion::None
    );
    assert_eq!(
        account_assessment(&protected_at_two_percent, "acct_protected").routing_exclusion(),
        RoutingExclusion::None
    );
    assert!(without_policy.preferred_next().is_some());
    assert!(protected_at_two_percent.preferred_next().is_some());
}

#[test]
fn weekly_quota_floor_exclusion_cannot_reenter_reserve_or_last_resort_pools() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_protected",
            vec![
                window(FIVE_HOURS, 1, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 4, 5 * 86_400, 0),
            ],
        )
        .with_weekly_quota_floor_basis_points(500),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::None);
    assert!(assessment.weighted_candidates().is_empty());
    assert_eq!(assessment.preferred_next(), None);
}

#[test]
fn weekly_quota_floor_input_preserves_invalid_value_and_zero_is_canonical_disabled() {
    let account = account("acct_policy", vec![]);

    assert_eq!(account.weekly_quota_floor_basis_points(), None);
    assert_eq!(
        account
            .clone()
            .with_weekly_quota_floor_basis_points(0)
            .weekly_quota_floor_basis_points(),
        None
    );
    assert_eq!(
        account
            .with_weekly_quota_floor_basis_points(u32::MAX)
            .weekly_quota_floor_basis_points(),
        Some(u32::MAX)
    );
}

#[test]
fn invalid_weekly_quota_floor_input_fails_closed_without_becoming_ten_percent() {
    for invalid_floor in [MAX_WEEKLY_QUOTA_FLOOR_BASIS_POINTS + 1, u32::MAX] {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_invalid_policy",
                vec![
                    window(FIVE_HOURS, 90, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 100, 3_600, 0),
                ],
            )
            .with_weekly_quota_floor_basis_points(invalid_floor),
        ]));

        assert_eq!(assessment.selected_pool(), SelectedPool::None);
        assert_eq!(
            account_assessment(&assessment, "acct_invalid_policy").routing_exclusion(),
            RoutingExclusion::WeeklyQuotaFloor
        );
    }
}

#[test]
fn weekly_quota_floor_does_not_depend_on_burn_confidence() {
    for confidence in [
        QuotaRunRateConfidence::Stale,
        QuotaRunRateConfidence::Unknown,
    ] {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_untrusted_burn",
                vec![
                    window(FIVE_HOURS, 90, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 80, 3_600, 100)
                        .with_burn_rate_confidence(confidence),
                ],
            )
            .with_weekly_quota_floor_basis_points(500),
        ]));
        let protected = account_assessment(&assessment, "acct_untrusted_burn");

        assert!(
            protected
                .weekly_survival_margin_basis_points()
                .is_some_and(|margin| margin > 500)
        );
        assert_eq!(protected.routing_exclusion(), RoutingExclusion::None);
    }
}

#[test]
fn exhausted_or_ineligible_window_reason_precedes_weekly_quota_floor() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_exhausted",
            vec![
                window(FIVE_HOURS, 90, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 0, 3_600, 0),
            ],
        )
        .with_weekly_quota_floor_basis_points(500),
        account(
            "acct_ineligible",
            vec![
                window(FIVE_HOURS, 90, 4 * 3_600),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible)
                    .with_remaining_headroom(80)
                    .with_reset_unix_seconds(NOW + 3_600)
                    .with_observed_unix_seconds(NOW)
                    .with_per_connection_burn_basis_points_per_hour(0),
            ],
        )
        .with_weekly_quota_floor_basis_points(500),
    ]));

    let exhausted = account_assessment(&assessment, "acct_exhausted");
    assert_eq!(exhausted.routing_exclusion(), RoutingExclusion::None);
    assert_eq!(
        exhausted.quota_evidence_reason(),
        QuotaEvidenceReason::WindowExhausted
    );
    assert_eq!(
        exhausted.routing_reason(),
        RoutingReason::BlockedWindowExhausted
    );
    let ineligible = account_assessment(&assessment, "acct_ineligible");
    assert_eq!(ineligible.routing_exclusion(), RoutingExclusion::None);
    assert_eq!(
        ineligible.quota_evidence_reason(),
        QuotaEvidenceReason::WindowIneligible
    );
    assert_eq!(
        ineligible.routing_reason(),
        RoutingReason::BlockedWindowIneligible
    );
}

#[test]
fn weekly_quota_floor_stays_excluded_across_mutating_peer_starts() {
    let protected = account(
        "acct_protected",
        vec![
            window(FIVE_HOURS, 90, 4 * 3_600),
            window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 4, 3_600, 0),
        ],
    )
    .with_weekly_quota_floor_basis_points(500);
    let peer_windows = vec![
        window(FIVE_HOURS, 90, 4 * 3_600),
        window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 80, 5 * 86_400, 20),
    ];

    for peer_active_sessions in 0..5 {
        let assessment = assess_route_band(input(vec![
            protected.clone(),
            account("acct_peer", peer_windows.clone())
                .with_current_active_sessions(peer_active_sessions),
        ]));

        assert_eq!(assessment.preferred_next(), Some(&account_id("acct_peer")));
        assert_eq!(
            account_assessment(&assessment, "acct_protected").routing_exclusion(),
            RoutingExclusion::WeeklyQuotaFloor
        );
        assert!(
            assessment
                .weighted_candidates()
                .iter()
                .all(|(candidate, _weight)| candidate.as_str() != "acct_protected")
        );
    }
}
