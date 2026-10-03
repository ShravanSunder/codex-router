use super::*;

#[test]
fn scenario_b_allows_weekly_salvage_when_weekly_reset_is_near() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![window(FIVE_HOURS, 5, 120), window(WEEKLY, 80, 5 * 86_400)],
        ),
        account(
            "acct_b",
            vec![window(FIVE_HOURS, 90, 4 * 3_600), window(WEEKLY, 20, 600)],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_b")
    );
    assert_account(&assessment, "acct_a", AccountAvailability::Usable, Some(9));
    assert_account(&assessment, "acct_b", AccountAvailability::Usable, Some(39));
    assert_eq!(
        account_assessment(&assessment, "acct_b").routing_reason(),
        RoutingReason::PreferredNearResetInitialAdmission
    );
}

#[test]
fn scenario_c_blocks_empty_weekly_window() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 80, 4 * 3_600),
                projected_window(WEEKLY, 0, 5 * 86_400, 3_600),
            ],
        ),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 42, 4 * 3_600),
                window(WEEKLY, 42, 5 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Reserve);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_b")
    );
    assert_account(&assessment, "acct_a", AccountAvailability::Blocked, None);
    assert_account(&assessment, "acct_b", AccountAvailability::Reserve, Some(0));
    assert_eq!(
        account_assessment(&assessment, "acct_a").routing_reason(),
        RoutingReason::BlockedWindowExhausted
    );
    let empty_account = account_assessment(&assessment, "acct_a");
    assert_eq!(
        empty_account.long_pressure(),
        72,
        "empty accounts should still calculate weekly pressure for quota display"
    );
    assert_eq!(
        empty_account.weekly_projected_exhaustion_unix_seconds(),
        Some(NOW + 3_600),
        "empty accounts should still carry projected runout for quota display"
    );
}

#[test]
fn scenario_d_prefers_short_window_near_reset_surplus() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![window(FIVE_HOURS, 30, 600), window(WEEKLY, 60, 3 * 86_400)],
        )
        .with_current_active_sessions(1),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 30, 4 * 3_600),
                window(WEEKLY, 60, 3 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a")
    );
    assert_account(&assessment, "acct_a", AccountAvailability::Usable, Some(40));
    assert_account(&assessment, "acct_b", AccountAvailability::Usable, Some(0));
    assert_eq!(
        account_assessment(&assessment, "acct_a").routing_reason(),
        RoutingReason::PreferredShortResetSoon
    );
}

#[test]
fn weak_quota_candidate_is_not_clamped_to_minimum_score_s1() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_weak",
            vec![
                window(FIVE_HOURS, 30, 4 * 3_600),
                window(WEEKLY, 60, 3 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
        account(
            "acct_healthier",
            vec![
                window(FIVE_HOURS, 80, 4 * 3_600),
                window(WEEKLY, 80, 3 * 86_400),
            ],
        )
        .with_current_active_sessions(1),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_healthier")
    );
    assert_account(
        &assessment,
        "acct_weak",
        AccountAvailability::Usable,
        Some(0),
    );
    assert!(
        assessment
            .weighted_candidates()
            .iter()
            .any(|(account_id, weight)| account_id.as_str() == "acct_weak" && *weight == 0),
        "weak accounts may remain visible in the selected pool, but must not be manufactured as score 1"
    );
}

#[test]
fn unknown_quota_is_fallback_only_when_known_pool_exists() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 50, 2 * 3_600),
                window(WEEKLY, 50, 3 * 86_400),
            ],
        ),
        account(
            "acct_b",
            vec![
                QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Unknown)
                    .with_remaining_headroom(90),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Unknown)
                    .with_remaining_headroom(90),
            ],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(assessment.weighted_candidates().len(), 1);
    assert_eq!(assessment.weighted_candidates()[0].0.as_str(), "acct_a");
    let unknown = account_assessment(&assessment, "acct_b");
    assert_eq!(unknown.availability(), AccountAvailability::Unknown);
    assert_eq!(unknown.routing_weight(), Some(1));
    assert_eq!(unknown.routing_reason(), RoutingReason::HeldUnknown);
    assert_eq!(
        unknown.quota_evidence_reason(),
        QuotaEvidenceReason::UnknownQuotaWindow
    );
}

#[test]
fn all_unknown_accounts_use_fallback_pool_with_candidates() {
    let assessment = assess_route_band(input(vec![
        account("acct_a", Vec::new()),
        account(
            "acct_b",
            vec![
                QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Unknown),
                QuotaWindowFact::new(WEEKLY, QuotaWindowStatus::Unknown),
            ],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Unknown);
    assert_eq!(assessment.weighted_candidates().len(), 2);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a")
    );
    assert_account(&assessment, "acct_a", AccountAvailability::Unknown, Some(1));
    assert_account(&assessment, "acct_b", AccountAvailability::Unknown, Some(1));
    assert_eq!(
        account_assessment(&assessment, "acct_a").routing_reason(),
        RoutingReason::UnknownFallbackPreferred
    );
    assert_eq!(
        account_assessment(&assessment, "acct_b").routing_reason(),
        RoutingReason::UnknownFallbackAvailable
    );
}

#[test]
fn missing_reset_or_expected_window_is_probe_required() {
    let missing_reset = assess_route_band(input(vec![account(
        "acct_missing_reset",
        vec![
            QuotaWindowFact::new(FIVE_HOURS, QuotaWindowStatus::Eligible)
                .with_remaining_headroom(50),
            window(WEEKLY, 50, 2 * 86_400),
        ],
    )]));
    let missing_expected = assess_route_band(input(vec![account(
        "acct_missing_expected",
        vec![window(FIVE_HOURS, 50, 2 * 3_600)],
    )]));

    assert_eq!(
        account_assessment(&missing_reset, "acct_missing_reset").quota_evidence_reason(),
        QuotaEvidenceReason::MissingResetTime
    );
    assert_eq!(
        account_assessment(&missing_expected, "acct_missing_expected").quota_evidence_reason(),
        QuotaEvidenceReason::MissingExpectedWindow
    );
    assert_eq!(missing_reset.selected_pool(), SelectedPool::Unknown);
    assert_eq!(missing_expected.selected_pool(), SelectedPool::Unknown);
    assert_eq!(missing_reset.weighted_candidates().len(), 1);
    assert_eq!(missing_expected.weighted_candidates().len(), 1);
}

#[test]
fn weekly_only_quota_is_known_and_ranked_while_five_hour_only_stays_unknown() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_weekly_healthier",
            vec![window(WEEKLY, 90, 5 * 86_400)],
        )
        .with_current_active_sessions(1),
        account("acct_weekly_lower", vec![window(WEEKLY, 70, 5 * 86_400)])
            .with_current_active_sessions(1),
        account(
            "acct_five_hour_only",
            vec![window(FIVE_HOURS, 95, 4 * 3_600)],
        ),
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_weekly_healthier")
    );
    assert_account(
        &assessment,
        "acct_weekly_healthier",
        AccountAvailability::Usable,
        Some(90),
    );
    assert_account(
        &assessment,
        "acct_five_hour_only",
        AccountAvailability::Unknown,
        Some(1),
    );
}

#[test]
fn returned_exhausted_five_hour_window_becomes_binding() {
    let weekly_only = assess_route_band(input(vec![account(
        "acct_dynamic",
        vec![window(WEEKLY, 90, 5 * 86_400)],
    )]));
    let with_exhausted_five_hour = assess_route_band(input(vec![account(
        "acct_dynamic",
        vec![
            window(FIVE_HOURS, 0, 4 * 3_600),
            window(WEEKLY, 90, 5 * 86_400),
        ],
    )]));

    assert_eq!(weekly_only.selected_pool(), SelectedPool::Usable);
    assert_eq!(
        weekly_only.preferred_next().map(AccountId::as_str),
        Some("acct_dynamic")
    );
    assert_eq!(with_exhausted_five_hour.selected_pool(), SelectedPool::None);
    assert_eq!(with_exhausted_five_hour.preferred_next(), None);
    assert_eq!(
        account_assessment(&with_exhausted_five_hour, "acct_dynamic").availability(),
        AccountAvailability::Blocked
    );
    assert_eq!(
        account_assessment(&with_exhausted_five_hour, "acct_dynamic").quota_evidence_reason(),
        QuotaEvidenceReason::WindowExhausted
    );
}

#[test]
fn stale_penalty_applies_only_inside_selected_pool() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_fresh",
            vec![
                window(FIVE_HOURS, 80, 4 * 3_600),
                window(WEEKLY, 80, 5 * 86_400),
            ],
        ),
        account(
            "acct_stale",
            vec![
                stale_window(FIVE_HOURS, 80, 4 * 3_600),
                stale_window(WEEKLY, 80, 5 * 86_400),
            ],
        ),
    ]));

    assert_account(
        &assessment,
        "acct_fresh",
        AccountAvailability::Usable,
        Some(80),
    );
    assert_account(
        &assessment,
        "acct_stale",
        AccountAvailability::Usable,
        Some(20),
    );
}

#[test]
fn disabled_and_missing_credential_accounts_are_excluded() {
    let disabled = account(
        "acct_disabled",
        vec![
            window(FIVE_HOURS, 80, 4 * 3_600),
            window(WEEKLY, 80, 5 * 86_400),
        ],
    )
    .with_account_enabled(false);
    let missing_credential = account(
        "acct_missing_credential",
        vec![
            window(FIVE_HOURS, 80, 4 * 3_600),
            window(WEEKLY, 80, 5 * 86_400),
        ],
    )
    .with_active_credential(false);
    let assessment = assess_route_band(input(vec![disabled, missing_credential]));

    assert_eq!(assessment.selected_pool(), SelectedPool::None);
    assert_eq!(
        account_assessment(&assessment, "acct_disabled").routing_exclusion(),
        RoutingExclusion::Disabled
    );
    assert_eq!(
        account_assessment(&assessment, "acct_missing_credential").routing_exclusion(),
        RoutingExclusion::MissingCredential
    );
}

#[test]
fn deterministic_order_uses_weight_pressure_salvage_and_account_id() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_b",
            vec![window(FIVE_HOURS, 30, 600), window(WEEKLY, 60, 3 * 86_400)],
        ),
        account(
            "acct_a",
            vec![window(FIVE_HOURS, 30, 600), window(WEEKLY, 60, 3 * 86_400)],
        ),
    ]));

    assert_eq!(assessment.weighted_candidates()[0].0.as_str(), "acct_a");
    assert_eq!(assessment.weighted_candidates()[1].0.as_str(), "acct_b");
    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a")
    );
}

#[test]
fn legacy_active_load_pressure_does_not_change_projected_burn() {
    let assessment = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 70, 4 * 3_600),
                window(WEEKLY, 70, 5 * 86_400),
            ],
        )
        .with_active_load_pressure(30),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 70, 4 * 3_600),
                window(WEEKLY, 70, 5 * 86_400),
            ],
        ),
    ]));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_a")
    );
    assert_eq!(
        account_assessment(&assessment, "acct_a").projected_burn_pressure(),
        account_assessment(&assessment, "acct_b").projected_burn_pressure(),
        "legacy active pressure must not be treated as projected quota burn"
    );
}

#[test]
fn legacy_active_load_pressure_does_not_change_selection_s2() {
    let without_legacy_pressure = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 70, 4 * 3_600),
                window(WEEKLY, 70, 5 * 86_400),
            ],
        ),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 70, 4 * 3_600),
                window(WEEKLY, 70, 5 * 86_400),
            ],
        ),
    ]));
    let with_legacy_pressure = assess_route_band(input(vec![
        account(
            "acct_a",
            vec![
                window(FIVE_HOURS, 70, 4 * 3_600),
                window(WEEKLY, 70, 5 * 86_400),
            ],
        )
        .with_active_load_pressure(100),
        account(
            "acct_b",
            vec![
                window(FIVE_HOURS, 70, 4 * 3_600),
                window(WEEKLY, 70, 5 * 86_400),
            ],
        ),
    ]));

    assert_eq!(
        with_legacy_pressure.preferred_next().map(AccountId::as_str),
        without_legacy_pressure
            .preferred_next()
            .map(AccountId::as_str),
        "S2: legacy active_pressure/headroom cost must not affect quota selection"
    );
}
