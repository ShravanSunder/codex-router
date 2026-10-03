use super::*;

#[test]
fn default_policy_constants_match_spec_r0() {
    assert_eq!(WEEKLY_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS, 200);
    assert_eq!(SHORT_SURVIVAL_SAFETY_BUFFER_BASIS_POINTS, 100);
    assert_eq!(SHORT_NEAR_RESET_THRESHOLD_SECONDS, 1_800);
    assert_eq!(SAME_POOL_RESET_TOLERANCE_SECONDS, 7_200);
    assert_eq!(SAME_POOL_PROJECTED_RUNOUT_TOLERANCE_SECONDS, 7_200);
    assert_eq!(SAME_POOL_SURVIVAL_MARGIN_TOLERANCE_BASIS_POINTS, 500);
    assert_eq!(ACTIVE_SESSION_IMBALANCE_THRESHOLD, 1);
    assert_eq!(USAGE_LIMIT_SUSPECT_TTL_SECONDS, 300);
    assert_eq!(ACTIVE_SESSION_ROLLUP_BUCKET_SECONDS, 300);
}

#[test]
fn scenario_a_uses_low_short_window_when_reset_is_near_and_weekly_is_healthy() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![window(FIVE_HOURS, 5, 120), window(WEEKLY, 80, 5 * 86_400)],
        )
        .with_current_active_sessions(1),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 90, 4 * 3_600),
                window(WEEKLY, 20, 5 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(assessment.route_band(), RouteBand::Responses);
    assert_eq!(
        assessment.route_status(),
        RouteBandAssessmentStatus::Supported
    );
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a")
    );
    assert_eq!(assessment.weighted_candidates()[0].0.as_str(), "acct_a");
    assert_account(&assessment, "acct_a", AccountAvailability::Usable, Some(9));
    assert_account(&assessment, "acct_b", AccountAvailability::Reserve, Some(0));
    assert_eq!(
        account_assessment(&assessment, "acct_a").routing_reason(),
        RoutingReason::PreferredWeeklyHealthier
    );
    assert_eq!(
        account_assessment(&assessment, "acct_b").routing_reason(),
        RoutingReason::HeldReserve
    );
}

#[test]
fn weekly_survival_prefers_soon_reset_survivor_over_far_reset_reserve_w1() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 20, 24 * 3_600, 50),
            ],
        ),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 34, 96 * 3_600, 50),
            ],
        ),
        account(
            "acct_c",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 80, 7 * 86_400, 50),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a"),
        "W1: A survives its soon reset; B and C are far-reset reserve/failures"
    );
}

#[test]
fn weekly_survival_prefers_earliest_reset_when_all_survive_w3() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 60, 48 * 3_600, 50),
            ],
        ),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 70, 96 * 3_600, 50),
            ],
        ),
        account(
            "acct_c",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 90, 7 * 86_400, 50),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a"),
        "W3: all weekly windows survive, so the earliest reset should win"
    );
}

#[test]
fn fresh_idle_unknown_burn_account_gets_initial_admission_w4() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window(WEEKLY, 20, 24 * 3_600),
            ],
        ),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 34, 96 * 3_600, 20),
            ],
        ),
        account(
            "acct_c",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 80, 7 * 86_400, 20),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a"),
        "W4 supersession: fresh idle near-reset quota receives one initial client"
    );
}

#[test]
fn fresh_idle_unknown_burn_beats_loaded_known_account_w8() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window(WEEKLY, 21, 24 * 3_600),
            ],
        ),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 36, 72 * 3_600, 30),
            ],
        )
        .with_current_active_sessions(3),
        account(
            "acct_c",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 80, 7 * 86_400, 30),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a")
    );
    assert_eq!(
        account_assessment(&assessment, "acct_a").routing_reason(),
        RoutingReason::PreferredNearResetInitialAdmission
    );
}

#[test]
fn fresh_quota_stale_burn_gets_one_initial_client_then_loses_exception() {
    let mut active_a = 2;
    let mut active_b = 0;
    let mut active_c = 0;
    let mut selected = Vec::new();
    for _ in 0..5 {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_a",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    projected_window(WEEKLY, 40, 24 * 3_600, 60 * 3_600)
                        .with_burn_rate_confidence(QuotaRunRateConfidence::Normal),
                ],
            )
            .with_current_active_sessions(active_a),
            account(
                "acct_b",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window(WEEKLY, 42, 25 * 3_600)
                        .with_burn_rate_confidence(QuotaRunRateConfidence::Stale),
                ],
            )
            .with_current_active_sessions(active_b),
            account(
                "acct_c",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    projected_window(WEEKLY, 65, 5 * 86_400, 10 * 86_400),
                ],
            )
            .with_current_active_sessions(active_c),
        ]));
        let account = assessment
            .preferred_next()
            .unwrap_or_else(|| panic!("scenario should select an account"))
            .as_str();
        selected.push(account.to_owned());
        match account {
            "acct_a" => active_a += 1,
            "acct_b" => active_b += 1,
            "acct_c" => active_c += 1,
            other => panic!("unexpected account {other}"),
        }
    }

    assert_eq!(selected, ["acct_b", "acct_a", "acct_a", "acct_a", "acct_a"]);
}

#[test]
fn initial_admission_is_one_client_then_ordinary_policy_resumes() {
    let build_assessment = |near_reset_active_sessions| {
        assess_route_band(input(vec![
            account(
                "acct_near_reset",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window(WEEKLY, 20, 24 * 3_600),
                ],
            )
            .with_current_active_sessions(near_reset_active_sessions),
            account(
                "acct_known_reserve",
                vec![
                    window(FIVE_HOURS, 100, 4 * 3_600),
                    window_with_per_connection_burn_basis_points_per_hour(
                        WEEKLY,
                        34,
                        96 * 3_600,
                        20,
                    ),
                ],
            ),
        ]))
    };

    assert_eq!(
        build_assessment(0).preferred_next().map(AccountId::as_str),
        Some("acct_near_reset")
    );
    assert_eq!(
        build_assessment(1).preferred_next().map(AccountId::as_str),
        Some("acct_known_reserve")
    );
}
