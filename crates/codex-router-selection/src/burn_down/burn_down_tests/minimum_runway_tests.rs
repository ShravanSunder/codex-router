use super::*;

#[test]
fn unknown_survival_margin_does_not_route_to_weaker_account_for_lower_active_count() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_healthier",
            vec![
                window(FIVE_HOURS, 60, 4 * 3_600),
                window(WEEKLY, 60, 3 * 86_400),
            ],
        )
        .with_current_active_sessions(2),
        account(
            "acct_weaker_idle",
            vec![
                window(FIVE_HOURS, 98, 4 * 3_600),
                window(WEEKLY, 23, 3 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_healthier"),
        "unknown-margin active count must not beat healthier raw weekly quota"
    );
}

#[test]
fn near_zero_projected_short_runout_is_held_by_flow_guard() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_fast",
            vec![
                projected_window(FIVE_HOURS, 20, 4 * 3_600, 600),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        ),
        account(
            "acct_slow",
            vec![
                window(FIVE_HOURS, 20, 4 * 3_600),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_slow")
    );
    assert_account(&assessment, "acct_fast", AccountAvailability::Blocked, None);
    assert_eq!(
        account_assessment(&assessment, "acct_fast").routing_reason(),
        RoutingReason::HeldShortWindowGuard
    );
}

#[test]
fn near_zero_projected_runout_survives_to_reset_stays_selectable() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_survives",
            vec![
                projected_window(FIVE_HOURS, 20, 20 * 60, 25 * 60),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
        account(
            "acct_worse_weekly",
            vec![
                window(FIVE_HOURS, 80, 4 * 3_600),
                window(WEEKLY, 10, 5 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_survives")
    );
    assert_account(
        &assessment,
        "acct_survives",
        AccountAvailability::Usable,
        Some(30),
    );
}

#[test]
fn near_zero_headroom_stays_selectable_when_no_alternative_can_serve() {
    let assessment = assess_route_band(input(vec![account(
        "acct_near_empty",
        vec![
            window(FIVE_HOURS, 4, 4 * 3_600),
            window(WEEKLY, 80, 5 * 86_400),
        ],
    )]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_near_empty")
    );
    assert_account(
        &assessment,
        "acct_near_empty",
        AccountAvailability::Usable,
        Some(0),
    );
}

#[test]
fn near_zero_headroom_stays_selectable_when_all_alternatives_are_worse() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_near_empty",
            vec![
                window(FIVE_HOURS, 4, 4 * 3_600),
                window(WEEKLY, 90, 5 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
        account(
            "acct_worse_weekly",
            vec![
                window(FIVE_HOURS, 90, 4 * 3_600),
                window(WEEKLY, 6, 5 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_near_empty")
    );
    assert_account(
        &assessment,
        "acct_near_empty",
        AccountAvailability::Usable,
        Some(0),
    );
    assert_account(
        &assessment,
        "acct_worse_weekly",
        AccountAvailability::Reserve,
        Some(0),
    );
}

#[test]
fn near_zero_headroom_remains_eligible_when_no_configured_floor_exists() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_near_empty",
            vec![
                window(FIVE_HOURS, 4, 4 * 3_600),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        ),
        account(
            "acct_healthy",
            vec![
                window(FIVE_HOURS, 40, 4 * 3_600),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_healthy")
    );
    assert_account(
        &assessment,
        "acct_near_empty",
        AccountAvailability::Usable,
        Some(0),
    );
    assert_account(
        &assessment,
        "acct_healthy",
        AccountAvailability::Usable,
        Some(0),
    );
}
