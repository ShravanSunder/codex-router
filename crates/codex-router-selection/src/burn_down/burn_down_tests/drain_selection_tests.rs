use super::*;

#[test]
fn weekly_non_survivor_above_reactive_floor_stays_in_drain_pool_w6() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 10, 20 * 3_600, 100),
            ],
        ),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                window_with_per_connection_burn_basis_points_per_hour(WEEKLY, 20, 60 * 3_600, 67),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a"),
        "W6: near-reset account above the reactive floor stays in the drain pool before later reset quota"
    );
    let preferred_account = account_assessment(&assessment, "acct_a");
    assert_eq!(
        preferred_account.routing_reason(),
        RoutingReason::PreferredNearResetControlledDrain,
        "W6 should explain that the chosen non-survivor remains in controlled drain"
    );
}

#[test]
fn weekly_non_survivor_fallback_uses_projected_runout_before_active_count_w7() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_lasts_longer_busy",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                projected_window(WEEKLY, 10, 24 * 3_600, hours_minutes(16, 40))
                    .with_per_connection_burn_basis_points_per_hour(60),
            ],
        )
        .with_current_active_sessions(3),
        account(
            "acct_runs_out_sooner_idle",
            vec![
                window(FIVE_HOURS, 100, 4 * 3_600),
                projected_window(WEEKLY, 9, 24 * 3_600, hours_minutes(12, 51))
                    .with_per_connection_burn_basis_points_per_hour(70),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_lasts_longer_busy"),
        "W7: when no same-pool account survives, latest projected runout beats active-count balancing"
    );
}

#[test]
fn single_account_low_weekly_still_selects_when_no_alternative_exists_s3a() {
    let assessment = assess_route_band(input(vec![account(
        "acct_only",
        vec![
            window(FIVE_HOURS, 95, hours_minutes(4, 0)),
            projected_window(WEEKLY, 4, hours_minutes(23, 0), hours_minutes(5, 0))
                .with_per_connection_burn_basis_points_per_hour(80),
        ],
    )]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_only"),
        "S3a: a single configured account should remain selectable until it is truly exhausted"
    );
}

#[test]
fn two_account_soon_reset_drain_beats_far_reset_reserve_when_runway_is_safe_s3b() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_near_reset",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 8, hours_minutes(23, 30), hours_minutes(12, 0))
                    .with_per_connection_burn_basis_points_per_hour(67),
            ],
        ),
        account(
            "acct_far_reset",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 26, 84 * 3_600, 24 * 3_600)
                    .with_per_connection_burn_basis_points_per_hour(108),
            ],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_near_reset"),
        "S3b: safe near-reset quota should be drained before consuming far-reset reserve"
    );
}

#[test]
fn two_account_near_reset_drain_above_reactive_floor_beats_far_reset_reserve_s3c() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_near_reset",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 8, hours_minutes(23, 30), hours_minutes(5, 30))
                    .with_per_connection_burn_basis_points_per_hour(145),
            ],
        ),
        account(
            "acct_far_reset",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 26, 84 * 3_600, 24 * 3_600)
                    .with_per_connection_burn_basis_points_per_hour(108),
            ],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_near_reset"),
        "S3c: a near-reset drain account above the reactive floor should take new starts"
    );
}

#[test]
fn same_reset_drain_pool_balances_active_sessions_before_runout_tiebreak_s3d() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_busy_near_reset",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 19, hours_minutes(45, 0), hours_minutes(20, 0))
                    .with_per_connection_burn_basis_points_per_hour(95),
            ],
        )
        .with_current_active_sessions(6),
        account(
            "acct_idle_near_reset",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 18, hours_minutes(46, 0), hours_minutes(19, 0))
                    .with_per_connection_burn_basis_points_per_hour(95),
            ],
        )
        .with_current_active_sessions(0),
        account(
            "acct_far_reset",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 40, 5 * 86_400, 4 * 86_400)
                    .with_per_connection_burn_basis_points_per_hour(42),
            ],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_idle_near_reset"),
        "S3d: same reset drain pool should share active sessions before chasing a small runout edge"
    );
}

#[test]
fn same_unknown_margin_pool_balances_one_active_session_s3e() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_busy",
            vec![
                window(FIVE_HOURS, 50, hours_minutes(4, 0)),
                window(WEEKLY, 50, 4 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
        account(
            "acct_idle",
            vec![
                window(FIVE_HOURS, 50, hours_minutes(4, 0)),
                window(WEEKLY, 50, 4 * 86_400),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_idle"),
        "S3e: equal quota/reset accounts without burn history should still share active sessions"
    );
}

#[test]
fn s3f_stale_unknown_peer_does_not_beat_known_drain_account() {
    let result = assert_account_selection_scenario(&AccountSelectionScenario {
        id: "S3f",
        starts_to_simulate: 5,
        accounts: vec![
            ScenarioAccountFixture {
                account_id: "acct_known",
                initial_active_sessions: 2,
                build_account: |active_sessions| {
                    account(
                        "acct_known",
                        vec![
                            window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                            projected_window(WEEKLY, 40, 24 * 3_600, 60 * 3_600)
                                .with_burn_rate_confidence(QuotaRunRateConfidence::Normal),
                        ],
                    )
                    .with_current_active_sessions(active_sessions)
                },
            },
            ScenarioAccountFixture {
                account_id: "acct_stale",
                initial_active_sessions: 0,
                build_account: |active_sessions| {
                    account(
                        "acct_stale",
                        vec![
                            stale_window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                            stale_window(WEEKLY, 42, hours_minutes(25, 0)),
                        ],
                    )
                    .with_current_active_sessions(active_sessions)
                },
            },
            ScenarioAccountFixture {
                account_id: "acct_far_reset",
                initial_active_sessions: 0,
                build_account: |active_sessions| {
                    account(
                        "acct_far_reset",
                        vec![
                            window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                            projected_window(WEEKLY, 40, 5 * 86_400, 10 * 86_400),
                        ],
                    )
                    .with_current_active_sessions(active_sessions)
                },
            },
        ],
        expected_sequence: vec![
            "acct_known",
            "acct_known",
            "acct_known",
            "acct_known",
            "acct_known",
        ],
        expected_final_active_sessions: vec![
            ("acct_known", 7),
            ("acct_stale", 0),
            ("acct_far_reset", 0),
        ],
        expected_final_account_states: vec![
            ExpectedAccountState {
                account_id: "acct_known",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::PreferredWeeklyHealthier,
            },
            ExpectedAccountState {
                account_id: "acct_stale",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
            ExpectedAccountState {
                account_id: "acct_far_reset",
                availability: AccountAvailability::Reserve,
                routing_reason: RoutingReason::HeldReserve,
            },
        ],
        expected_selected_weekly_runouts: None,
    });

    assert_eq!(
        account_assessment(&result.final_assessment, "acct_far_reset").routing_reason(),
        RoutingReason::HeldReserve
    );
}

#[test]
fn s3g_hard_blocked_account_is_skipped_before_far_reset_reserve() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_blocked",
            vec![
                QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible),
            ],
        ),
        account(
            "acct_drain",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 25, 24 * 3_600, 60 * 3_600),
            ],
        ),
        account(
            "acct_far_reset",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 70, 5 * 86_400, 10 * 86_400),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_drain"),
        "S3g: hard-blocked account is skipped and far-reset reserve is held"
    );
    assert_eq!(
        account_assessment(&assessment, "acct_blocked").availability(),
        AccountAvailability::Blocked
    );
}

#[test]
fn s3i_all_hard_blocked_accounts_have_no_selector_candidate() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible),
            ],
        ),
        account(
            "acct_b",
            vec![
                QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible),
            ],
        ),
        account(
            "acct_c",
            vec![
                QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Ineligible),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Ineligible),
            ],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::None);
    assert_eq!(assessment.preferred_next(), None);
    assert!(assessment.weighted_candidates().is_empty());
}
