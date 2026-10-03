use super::*;

#[test]
fn six_session_selection_stays_in_same_weekly_pool_before_far_reset_reserve_a5_s1() {
    let mut selected_accounts = Vec::new();

    for _session_start in 0..6 {
        let assessment = assess_route_band(input(vec![
            account(
                "acct_askluna",
                vec![
                    window(FIVE_HOURS, 98, 4 * 3_600),
                    window(WEEKLY, 23, 3 * 86_400),
                ],
            )
            .with_current_active_sessions(1),
            account(
                "acct_matches",
                vec![
                    window(FIVE_HOURS, 99, 4 * 3_600),
                    window(WEEKLY, 34, 3 * 86_400),
                ],
            )
            .with_current_active_sessions(1),
            account(
                "acct_ssdev",
                vec![
                    window(FIVE_HOURS, 78, 3 * 3_600),
                    window(WEEKLY, 76, 5 * 86_400),
                ],
            )
            .with_current_active_sessions(1),
        ]));
        let selected = assessment
            .preferred_next()
            .unwrap_or_else(|| panic!("session start should have a quota candidate"))
            .as_str();
        selected_accounts.push(selected.to_owned());
    }

    assert_eq!(
        selected_accounts.first().map(String::as_str),
        Some("acct_matches")
    );
    assert_eq!(
        selected_accounts,
        vec![
            "acct_matches".to_owned(),
            "acct_matches".to_owned(),
            "acct_matches".to_owned(),
            "acct_matches".to_owned(),
            "acct_matches".to_owned(),
            "acct_matches".to_owned(),
        ],
        "without measured active-session input, strict quota selection is deterministic"
    );
    assert!(
        selected_accounts
            .iter()
            .any(|account| account == "acct_matches"),
        "same low-weekly reset pool should be used before far-reset reserve: {selected_accounts:?}"
    );
    assert!(
        selected_accounts
            .iter()
            .all(|account| account != "acct_askluna"),
        "weak weekly quota account must not be selected while healthier accounts exist: {selected_accounts:?}"
    );
    assert!(
        selected_accounts
            .iter()
            .all(|account| account != "acct_ssdev"),
        "far-reset reserve must not be selected while same-pool account can serve: {selected_accounts:?}"
    );
}

#[test]
fn s4_low_weekly_pool_drains_b_b_a_b_a() {
    fn askluna(active_sessions: u32) -> BurnDownAccountInput {
        account(
            "acct_askluna",
            vec![
                window(FIVE_HOURS, 99, hours_minutes(4, 46)),
                projected_window(
                    WEEKLY,
                    4,
                    hours_minutes(22, 49),
                    askluna_projected_weekly_runout(active_sessions),
                )
                .with_per_connection_burn_basis_points_per_hour(39),
            ],
        )
        .with_current_active_sessions(active_sessions)
    }

    fn matches(active_sessions: u32) -> BurnDownAccountInput {
        account(
            "acct_matches",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 59)),
                projected_window(
                    WEEKLY,
                    8,
                    hours_minutes(23, 56),
                    matches_projected_weekly_runout(active_sessions),
                )
                .with_per_connection_burn_basis_points_per_hour(53),
            ],
        )
        .with_current_active_sessions(active_sessions)
    }

    fn ssdev(active_sessions: u32) -> BurnDownAccountInput {
        account(
            "acct_ssdev",
            vec![
                window(FIVE_HOURS, 97, hours_minutes(4, 36)),
                projected_window(
                    WEEKLY,
                    26,
                    84 * 3_600,
                    ssdev_projected_weekly_runout(active_sessions),
                )
                .with_per_connection_burn_basis_points_per_hour(105),
            ],
        )
        .with_current_active_sessions(active_sessions)
    }

    let result = assert_account_selection_scenario(&AccountSelectionScenario {
        id: "S4",
        starts_to_simulate: 5,
        accounts: vec![
            ScenarioAccountFixture {
                account_id: "acct_askluna",
                initial_active_sessions: 1,
                build_account: askluna,
            },
            ScenarioAccountFixture {
                account_id: "acct_matches",
                initial_active_sessions: 0,
                build_account: matches,
            },
            ScenarioAccountFixture {
                account_id: "acct_ssdev",
                initial_active_sessions: 1,
                build_account: ssdev,
            },
        ],
        expected_sequence: vec![
            "acct_matches",
            "acct_matches",
            "acct_askluna",
            "acct_matches",
            "acct_askluna",
        ],
        expected_final_active_sessions: vec![
            ("acct_askluna", 3),
            ("acct_matches", 3),
            ("acct_ssdev", 1),
        ],
        expected_final_account_states: vec![
            ExpectedAccountState {
                account_id: "acct_askluna",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
            ExpectedAccountState {
                account_id: "acct_matches",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::PreferredNearResetControlledDrain,
            },
            ExpectedAccountState {
                account_id: "acct_ssdev",
                availability: AccountAvailability::Reserve,
                routing_reason: RoutingReason::HeldReserve,
            },
        ],
        expected_selected_weekly_runouts: Some(vec![
            ("acct_matches", Some(hours_minutes(15, 5))),
            ("acct_matches", Some(hours_minutes(7, 32))),
            ("acct_askluna", Some(hours_minutes(5, 7))),
            ("acct_matches", Some(hours_minutes(5, 2))),
            ("acct_askluna", Some(hours_minutes(3, 25))),
        ]),
    });

    assert_eq!(
        account_assessment(&result.final_assessment, "acct_askluna").availability(),
        AccountAvailability::Usable,
        "S4: A remains usable for controlled drain instead of being retired"
    );
}

#[test]
fn s5_far_reset_reserve_is_preserved_while_near_reset_pool_can_serve() {
    fn acct_a(active_sessions: u32) -> BurnDownAccountInput {
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 18, 24 * 3_600, 180 * 3_600)
                    .with_per_connection_burn_basis_points_per_hour(10),
            ],
        )
        .with_current_active_sessions(active_sessions)
    }

    fn acct_b(active_sessions: u32) -> BurnDownAccountInput {
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 19, hours_minutes(25, 0), 190 * 3_600)
                    .with_per_connection_burn_basis_points_per_hour(10),
            ],
        )
        .with_current_active_sessions(active_sessions)
    }

    fn acct_c(active_sessions: u32) -> BurnDownAccountInput {
        account(
            "acct_c",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 60, 96 * 3_600, 600 * 3_600)
                    .with_per_connection_burn_basis_points_per_hour(10),
            ],
        )
        .with_current_active_sessions(active_sessions)
    }

    let result = assert_account_selection_scenario(&AccountSelectionScenario {
        id: "S5",
        starts_to_simulate: 5,
        accounts: vec![
            ScenarioAccountFixture {
                account_id: "acct_a",
                initial_active_sessions: 0,
                build_account: acct_a,
            },
            ScenarioAccountFixture {
                account_id: "acct_b",
                initial_active_sessions: 0,
                build_account: acct_b,
            },
            ScenarioAccountFixture {
                account_id: "acct_c",
                initial_active_sessions: 0,
                build_account: acct_c,
            },
        ],
        expected_sequence: vec!["acct_a", "acct_b", "acct_a", "acct_b", "acct_a"],
        expected_final_active_sessions: vec![("acct_a", 3), ("acct_b", 2), ("acct_c", 0)],
        expected_final_account_states: vec![
            ExpectedAccountState {
                account_id: "acct_a",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
            ExpectedAccountState {
                account_id: "acct_b",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::PreferredNearResetDrainable,
            },
            ExpectedAccountState {
                account_id: "acct_c",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
        ],
        expected_selected_weekly_runouts: None,
    });

    assert_ne!(
        result
            .final_assessment
            .preferred_next()
            .map(AccountId::as_str),
        Some("acct_c"),
        "S5: C must still not be the next account after the simulated starts"
    );
}

#[test]
fn s3n_same_effective_weekly_pool_spreads_by_active_sessions() {
    fn acct_a(active_sessions: u32) -> BurnDownAccountInput {
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 18, 24 * 3_600, 45 * 3_600)
                    .with_per_connection_burn_basis_points_per_hour(40),
            ],
        )
        .with_current_active_sessions(active_sessions)
    }

    fn acct_b(active_sessions: u32) -> BurnDownAccountInput {
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 19, 24 * 3_600, hours_minutes(47, 30))
                    .with_per_connection_burn_basis_points_per_hour(40),
            ],
        )
        .with_current_active_sessions(active_sessions)
    }

    fn acct_c(active_sessions: u32) -> BurnDownAccountInput {
        account(
            "acct_c",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 20, 24 * 3_600, 50 * 3_600)
                    .with_per_connection_burn_basis_points_per_hour(40),
            ],
        )
        .with_current_active_sessions(active_sessions)
    }

    assert_account_selection_scenario(&AccountSelectionScenario {
        id: "S3n",
        starts_to_simulate: 5,
        accounts: vec![
            ScenarioAccountFixture {
                account_id: "acct_a",
                initial_active_sessions: 0,
                build_account: acct_a,
            },
            ScenarioAccountFixture {
                account_id: "acct_b",
                initial_active_sessions: 0,
                build_account: acct_b,
            },
            ScenarioAccountFixture {
                account_id: "acct_c",
                initial_active_sessions: 0,
                build_account: acct_c,
            },
        ],
        expected_sequence: vec!["acct_c", "acct_b", "acct_a", "acct_c", "acct_b"],
        expected_final_active_sessions: vec![("acct_a", 1), ("acct_b", 2), ("acct_c", 2)],
        expected_final_account_states: vec![
            ExpectedAccountState {
                account_id: "acct_a",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::PreferredNearResetControlledDrain,
            },
            ExpectedAccountState {
                account_id: "acct_b",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
            ExpectedAccountState {
                account_id: "acct_c",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
        ],
        expected_selected_weekly_runouts: None,
    });
}
