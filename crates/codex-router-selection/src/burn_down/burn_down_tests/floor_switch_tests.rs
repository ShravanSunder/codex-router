use super::*;

#[test]
fn weekly_floor_switch_prefers_peer_but_hard_stop_waits_for_configured_floor() {
    let assess = |protected_remaining, include_peer| {
        let mut accounts = vec![
            account(
                "acct_protected",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window(WEEKLY, protected_remaining, 20 * 3_600),
                ],
            )
            .with_weekly_quota_floor_basis_points(500),
        ];
        if include_peer {
            accounts.push(
                account(
                    "acct_healthy_peer",
                    vec![
                        window(FIVE_HOURS, 100, 4 * 3_600),
                        window_with_per_connection_burn_basis_points_per_hour(
                            WEEKLY,
                            80,
                            5 * 86_400,
                            20,
                        ),
                    ],
                )
                .with_current_active_sessions(1),
            );
        }
        assess_route_band(input(accounts))
    };

    let above_switch = assess(9, true);
    assert_eq!(
        above_switch.preferred_next().map(AccountId::as_str),
        Some("acct_protected")
    );
    let switching = assess(8, true);
    let protected = account_assessment(&switching, "acct_protected");
    assert_eq!(protected.routing_exclusion(), RoutingExclusion::None);
    assert_eq!(protected.routing_reason().as_str(), "held_floor_switch");
    assert_eq!(
        switching.preferred_next().map(AccountId::as_str),
        Some("acct_healthy_peer")
    );
    let no_peer = assess(8, false);
    assert_eq!(
        no_peer.preferred_next().map(AccountId::as_str),
        Some("acct_protected")
    );
    let hard_stop = assess(5, true);
    assert_eq!(
        account_assessment(&hard_stop, "acct_protected").routing_exclusion(),
        RoutingExclusion::WeeklyQuotaFloor
    );
    assert_eq!(
        hard_stop.preferred_next().map(AccountId::as_str),
        Some("acct_healthy_peer")
    );
}

#[test]
fn included_quota_keeps_credit_reserve_out_of_floor_switch_peer_selection() {
    let protected = account(
        "acct_protected_credit_peer",
        vec![
            window(FIVE_HOURS, 100, 4 * 3_600),
            window(WEEKLY, 8, 20 * 3_600),
        ],
    )
    .with_weekly_quota_floor_basis_points(500);
    let credit_peer = account(
        "acct_credit_peer",
        vec![
            QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible)
                .with_remaining_headroom(0)
                .with_reset_unix_seconds(NOW + 4 * 3_600)
                .with_observed_unix_seconds(NOW),
            window(WEEKLY, 20, 6 * 86_400),
        ],
    )
    .with_credit_backed_eligibility(CreditBackedEligibility::Eligible);

    let switching = assess_route_band(input(vec![protected, credit_peer]));
    let protected_assessment = account_assessment(&switching, "acct_protected_credit_peer");
    let credit_assessment = account_assessment(&switching, "acct_credit_peer");

    assert_ne!(
        protected_assessment.routing_reason(),
        RoutingReason::HeldFloorSwitch
    );
    assert_eq!(
        switching.preferred_next().map(AccountId::as_str),
        Some("acct_protected_credit_peer")
    );
    assert_eq!(
        credit_assessment.availability(),
        AccountAvailability::Reserve
    );
    assert_eq!(
        credit_assessment.routing_reason(),
        RoutingReason::HeldForIncludedQuota
    );
    assert_eq!(credit_assessment.routing_weight(), None);
    assert!(!credit_assessment.is_healthy_floor_switch_peer());
    assert_eq!(credit_assessment.projected_weekly_runway_seconds(), None);
}

#[test]
fn switch_band_falls_back_without_a_healthy_peer_or_with_only_short_runway() {
    let band_account = |account_id_value| {
        account(
            account_id_value,
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window(WEEKLY, 8, 20 * 3_600),
            ],
        )
        .with_weekly_quota_floor_basis_points(500)
    };
    let both_in_band = assess_route_band(input(vec![
        band_account("acct_first_band"),
        band_account("acct_second_band"),
    ]));
    assert_eq!(both_in_band.weighted_candidates().len(), 2);
    assert!(both_in_band.accounts().iter().all(|account| {
        account.routing_exclusion() == RoutingExclusion::None
            && account.routing_reason() != RoutingReason::HeldFloorSwitch
    }));

    let short_runway_peer = account(
        "acct_short_runway_peer",
        vec![
            window(FIVE_HOURS, 100, 4 * 3_600),
            window(WEEKLY, 50, 5 * 86_400)
                .with_projected_exhaustion_unix_seconds(NOW + 600)
                .with_per_connection_burn_basis_points_per_hour(100)
                .with_burn_rate_confidence(QuotaRunRateConfidence::Normal),
        ],
    );
    let no_safe_peer = assess_route_band(input(vec![
        band_account("acct_protected"),
        short_runway_peer,
    ]));
    assert_eq!(
        no_safe_peer.preferred_next().map(AccountId::as_str),
        Some("acct_protected")
    );
    assert_ne!(
        account_assessment(&no_safe_peer, "acct_protected").routing_reason(),
        RoutingReason::HeldFloorSwitch
    );
}

#[test]
fn switch_band_filters_before_near_and_far_idle_promotion() {
    for protected_reset in [20 * 3_600, 53 * 3_600] {
        let protected = account(
            "acct_yielding_early_reset",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window(WEEKLY, 8, protected_reset),
            ],
        )
        .with_weekly_quota_floor_basis_points(500);
        let healthy_far_idle = account(
            "acct_healthy_far_idle",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 60, 97 * 3_600, 14),
            ],
        );
        let assessment = assess_route_band(input(vec![protected, healthy_far_idle]));
        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_healthy_far_idle")
        );
        assert_eq!(
            account_assessment(&assessment, "acct_healthy_far_idle").routing_reason(),
            RoutingReason::PreferredIdleFarResetAdmission
        );
        assert_eq!(
            account_assessment(&assessment, "acct_yielding_early_reset").routing_reason(),
            RoutingReason::HeldFloorSwitch
        );
    }
}

#[test]
fn configured_floor_is_hard_stop_and_switch_band_is_not_excluded() {
    for (remaining, excluded) in [(5, true), (7, false), (8, false), (9, false)] {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_floor_threshold",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window(WEEKLY, remaining, 97 * 3_600),
                ],
            )
            .with_weekly_quota_floor_basis_points(500),
        ]));
        assert_eq!(
            account_assessment(&assessment, "acct_floor_threshold").routing_exclusion()
                == RoutingExclusion::WeeklyQuotaFloor,
            excluded,
        );
    }
    let without_floor = assess_route_band(input(vec![account(
        "acct_no_floor",
        vec![
            window(FIVE_HOURS, 100, 4 * 3_600),
            window(WEEKLY, 2, 97 * 3_600),
        ],
    )]));
    assert_eq!(
        account_assessment(&without_floor, "acct_no_floor").availability(),
        AccountAvailability::Usable
    );
}
