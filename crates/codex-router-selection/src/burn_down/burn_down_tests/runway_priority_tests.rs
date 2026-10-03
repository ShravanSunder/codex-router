use super::*;

#[test]
fn s3k_near_reset_drain_account_is_used_before_later_resets() {
    let result = assert_account_selection_scenario(&AccountSelectionScenario {
        id: "S3k",
        starts_to_simulate: 5,
        accounts: vec![
            ScenarioAccountFixture {
                account_id: "acct_reset_soon",
                initial_active_sessions: 0,
                build_account: |active_sessions| {
                    account(
                        "acct_reset_soon",
                        vec![
                            window(FIVE_HOURS, 100, hours_minutes(2, 0)),
                            window_with_projected_burn_for_proposed_connection(
                                WEEKLY,
                                30,
                                hours_minutes(2, 0),
                                480,
                                active_sessions,
                            ),
                        ],
                    )
                    .with_current_active_sessions(active_sessions)
                },
            },
            ScenarioAccountFixture {
                account_id: "acct_next_day",
                initial_active_sessions: 0,
                build_account: |active_sessions| {
                    account(
                        "acct_next_day",
                        vec![
                            window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                            window_with_projected_burn_for_proposed_connection(
                                WEEKLY,
                                32,
                                hours_minutes(26, 0),
                                100,
                                active_sessions,
                            ),
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
                            window_with_projected_burn_for_proposed_connection(
                                WEEKLY,
                                75,
                                5 * 86_400,
                                20,
                                active_sessions,
                            ),
                        ],
                    )
                    .with_current_active_sessions(active_sessions)
                },
            },
        ],
        expected_sequence: vec![
            "acct_reset_soon",
            "acct_reset_soon",
            "acct_next_day",
            "acct_reset_soon",
            "acct_next_day",
        ],
        expected_final_active_sessions: vec![
            ("acct_reset_soon", 3),
            ("acct_next_day", 2),
            ("acct_far_reset", 0),
        ],
        expected_final_account_states: vec![
            ExpectedAccountState {
                account_id: "acct_reset_soon",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
            ExpectedAccountState {
                account_id: "acct_next_day",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::PreferredNearResetControlledDrain,
            },
            ExpectedAccountState {
                account_id: "acct_far_reset",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
        ],
        expected_selected_weekly_runouts: Some(vec![
            ("acct_reset_soon", Some(22_500)),
            ("acct_reset_soon", Some(11_250)),
            ("acct_next_day", Some(115_200)),
            ("acct_reset_soon", Some(7_500)),
            ("acct_next_day", Some(57_600)),
        ]),
    });

    assert_eq!(
        account_assessment(&result.final_assessment, "acct_next_day").routing_reason(),
        RoutingReason::PreferredNearResetControlledDrain
    );
}

#[test]
fn s3l_refreshed_reset_segment_does_not_create_fake_old_burn() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_refreshed_far_reset",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 95, 5 * 86_400, 20 * 86_400),
            ],
        ),
        account(
            "acct_current_drain",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 28, 24 * 3_600, 96 * 3_600),
            ],
        ),
        account(
            "acct_reserve",
            vec![
                window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                projected_window(WEEKLY, 65, 5 * 86_400, 20 * 86_400),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_current_drain"),
        "S3l: old reset segment history must not make refreshed far-reset quota look preferable to current drain quota"
    );
}

#[test]
fn s3m_projected_runway_beats_naive_active_count() {
    let result = assert_account_selection_scenario(&AccountSelectionScenario {
        id: "S3m",
        starts_to_simulate: 5,
        accounts: vec![
            ScenarioAccountFixture {
                account_id: "acct_fast_burn_idle",
                initial_active_sessions: 0,
                build_account: |active_sessions| {
                    account(
                        "acct_fast_burn_idle",
                        vec![
                            window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                            window_with_projected_burn_for_proposed_connection(
                                WEEKLY,
                                18,
                                24 * 3_600,
                                80,
                                active_sessions,
                            ),
                        ],
                    )
                    .with_current_active_sessions(active_sessions)
                },
            },
            ScenarioAccountFixture {
                account_id: "acct_slower_burn_busy",
                initial_active_sessions: 1,
                build_account: |active_sessions| {
                    account(
                        "acct_slower_burn_busy",
                        vec![
                            window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                            window_with_projected_burn_for_proposed_connection(
                                WEEKLY,
                                18,
                                24 * 3_600,
                                40,
                                active_sessions,
                            ),
                        ],
                    )
                    .with_current_active_sessions(active_sessions)
                },
            },
            ScenarioAccountFixture {
                account_id: "acct_far_reset_low_burn",
                initial_active_sessions: 0,
                build_account: |active_sessions| {
                    account(
                        "acct_far_reset_low_burn",
                        vec![
                            window(FIVE_HOURS, 100, hours_minutes(4, 0)),
                            window_with_projected_burn_for_proposed_connection(
                                WEEKLY,
                                50,
                                5 * 86_400,
                                20,
                                active_sessions,
                            ),
                        ],
                    )
                    .with_current_active_sessions(active_sessions)
                },
            },
        ],
        expected_sequence: vec![
            "acct_fast_burn_idle",
            "acct_slower_burn_busy",
            "acct_slower_burn_busy",
            "acct_fast_burn_idle",
            "acct_slower_burn_busy",
        ],
        expected_final_active_sessions: vec![
            ("acct_fast_burn_idle", 2),
            ("acct_slower_burn_busy", 4),
            ("acct_far_reset_low_burn", 0),
        ],
        expected_final_account_states: vec![
            ExpectedAccountState {
                account_id: "acct_fast_burn_idle",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::PreferredNearResetControlledDrain,
            },
            ExpectedAccountState {
                account_id: "acct_slower_burn_busy",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
            ExpectedAccountState {
                account_id: "acct_far_reset_low_burn",
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            },
        ],
        expected_selected_weekly_runouts: Some(vec![
            ("acct_fast_burn_idle", Some(hours_minutes(22, 30))),
            ("acct_slower_burn_busy", Some(hours_minutes(22, 30))),
            ("acct_slower_burn_busy", Some(hours_minutes(15, 0))),
            ("acct_fast_burn_idle", Some(hours_minutes(11, 15))),
            ("acct_slower_burn_busy", Some(hours_minutes(11, 15))),
        ]),
    });

    assert_eq!(
        result.selected_accounts.first().map(String::as_str),
        Some("acct_fast_burn_idle"),
        "S3m: equal initial runway preserves the lower-active tie-break"
    );
}
