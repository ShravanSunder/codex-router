use super::*;

#[test]
fn last_resort_short_window_guard_is_selected_when_no_better_candidate_exists() {
    let (assessment, guarded_account_id) = last_resort_short_window_guard_assessment();
    let mut holds = HashMap::new();
    let mut weighted_selector =
        codex_router_selection::weighted_deficit::WeightedDeficitSelector::default();

    let selected = super::select_from_burn_down_assessment(
        "responses",
        Provider::Openai,
        &assessment,
        &mut weighted_selector,
        &mut holds,
        120,
        10_000,
    )
    .unwrap_or_else(|error| panic!("last-resort guarded selection should succeed: {error}"));

    assert_eq!(selected.account_id(), &guarded_account_id);
    assert_eq!(
        selected.selection_reason(),
        "preferred_last_resort_short_window_guard"
    );
}

#[test]
fn post_exhaustion_alternative_allows_last_resort_short_window_guard() {
    let (assessment, _guarded_account_id) = last_resort_short_window_guard_assessment();

    assert_eq!(
        super::post_exhaustion_assessment_has_safe_known_fresh_alternative(&assessment),
        Ok(true)
    );
}

fn last_resort_short_window_guard_assessment() -> (
    codex_router_selection::burn_down::BurnDownRouteBandAssessmentResult,
    AccountId,
) {
    let guarded_account_id = account_id("acct_guarded");
    let empty_account_id = account_id("acct_empty");
    let ineligible_account_id = account_id("acct_ineligible");
    let account_inputs = vec![
        codex_router_selection::burn_down::BurnDownAccountInput::new(
            guarded_account_id.clone(),
            "guarded",
            Provider::Openai,
            vec![
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(2)
                .with_reset_unix_seconds(18_000)
                .with_per_connection_burn_basis_points_per_hour(100),
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(80)
                .with_reset_unix_seconds(4 * 86_400)
                .with_per_connection_burn_basis_points_per_hour(20),
            ],
        ),
        codex_router_selection::burn_down::BurnDownAccountInput::new(
            empty_account_id,
            "empty",
            Provider::Openai,
            vec![
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(0)
                .with_reset_unix_seconds(18_000),
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(0)
                .with_reset_unix_seconds(4 * 86_400),
            ],
        ),
        codex_router_selection::burn_down::BurnDownAccountInput::new(
            ineligible_account_id,
            "ineligible",
            Provider::Openai,
            vec![
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Ineligible,
                ),
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Ineligible,
                ),
            ],
        ),
    ];
    let assessment = codex_router_selection::burn_down::assess_route_band(
        codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput::new(
            codex_router_core::routes::RouteBand::Responses,
            10_000,
            RESPONSES_HTTP.clone(),
            account_inputs,
        ),
    );

    (assessment, guarded_account_id)
}

#[test]
fn selected_source_credit_provenance_comes_from_the_typed_assessment() {
    use codex_router_selection::burn_down::BurnDownAccountInput;
    use codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput;
    use codex_router_selection::burn_down::CreditBackedEligibility;
    use codex_router_selection::burn_down::QuotaWindowFact;
    use codex_router_selection::burn_down::QuotaWindowStatus;
    use codex_router_selection::burn_down::SelectedPool;
    use codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS;
    use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;
    use codex_router_selection::burn_down::assess_route_band;

    let now_unix_seconds = 1_000;
    let credit_backed_account = BurnDownAccountInput::new(
        account_id("acct_selected_credit_backed"),
        "selected-credit-backed",
        Provider::Openai,
        vec![
            QuotaWindowFact::new(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
                .with_remaining_headroom(0)
                .with_reset_unix_seconds(now_unix_seconds + V1_SHORT_WINDOW_SECONDS)
                .with_observed_unix_seconds(now_unix_seconds)
                .with_effective(true),
            QuotaWindowFact::new(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
                .with_remaining_headroom(20)
                .with_reset_unix_seconds(now_unix_seconds + V1_WEEKLY_WINDOW_SECONDS)
                .with_observed_unix_seconds(now_unix_seconds),
        ],
    )
    .with_credit_backed_eligibility(CreditBackedEligibility::Eligible);
    let credit_backed_assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        RESPONSES_HTTP.clone(),
        vec![credit_backed_account],
    ));
    assert_eq!(
        credit_backed_assessment.selected_pool(),
        SelectedPool::Reserve
    );
    let credit_backed_decision = super::select_from_burn_down_assessment(
        RouteBand::Responses.as_str(),
        Provider::Openai,
        &credit_backed_assessment,
        &mut super::WeightedDeficitSelector::default(),
        &mut HashMap::new(),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        now_unix_seconds,
    )
    .expect("credit-backed source should select");
    assert!(credit_backed_decision.credit_backed_at_selection());
    assert_eq!(credit_backed_decision.selection_reason(), "credit_backed");

    let mut credit_backed_affinity_holds = HashMap::new();
    let credit_backed_affinity_decision = super::select_affinity_owner(
        RouteBand::Responses,
        Provider::Openai,
        credit_backed_assessment
            .preferred_next()
            .expect("credit-backed candidate should be preferred"),
        &credit_backed_assessment,
        &mut credit_backed_affinity_holds,
        now_unix_seconds,
        "previous_response_affinity",
    )
    .expect("available credit-backed affinity owner should remain selectable");
    assert!(credit_backed_affinity_decision.credit_backed_at_selection());
    assert_eq!(
        credit_backed_affinity_decision.selection_reason(),
        "previous_response_affinity"
    );

    let last_resort_account = BurnDownAccountInput::new(
        account_id("acct_selected_last_resort"),
        "selected-last-resort",
        Provider::Openai,
        vec![
            QuotaWindowFact::new(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
                .with_remaining_headroom(2)
                .with_reset_unix_seconds(now_unix_seconds + 4 * 3_600)
                .with_observed_unix_seconds(now_unix_seconds)
                .with_per_connection_burn_basis_points_per_hour(100),
            QuotaWindowFact::new(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
                .with_remaining_headroom(80)
                .with_reset_unix_seconds(now_unix_seconds + 4 * 86_400)
                .with_observed_unix_seconds(now_unix_seconds)
                .with_per_connection_burn_basis_points_per_hour(20),
        ],
    );
    let last_resort_assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        RESPONSES_HTTP.clone(),
        vec![last_resort_account],
    ));
    assert_eq!(
        last_resort_assessment.selected_pool(),
        SelectedPool::LastResort
    );
    let last_resort_decision = super::select_from_burn_down_assessment(
        RouteBand::Responses.as_str(),
        Provider::Openai,
        &last_resort_assessment,
        &mut super::WeightedDeficitSelector::default(),
        &mut HashMap::new(),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        now_unix_seconds,
    )
    .expect("LastResort source should select");
    assert_eq!(
        last_resort_decision.selection_reason(),
        "preferred_last_resort_short_window_guard"
    );
    assert!(!last_resort_decision.credit_backed_at_selection());

    let mut unknown_holds = HashMap::new();
    let unknown_assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        RESPONSES_HTTP.clone(),
        vec![BurnDownAccountInput::new(
            account_id("acct_selected_unknown"),
            "selected-unknown",
            Provider::Openai,
            Vec::new(),
        )],
    ));
    assert_eq!(unknown_assessment.selected_pool(), SelectedPool::Unknown);
    let unknown_decision = super::select_from_burn_down_assessment(
        RouteBand::Responses.as_str(),
        Provider::Openai,
        &unknown_assessment,
        &mut super::WeightedDeficitSelector::default(),
        &mut unknown_holds,
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        now_unix_seconds,
    )
    .expect("unknown fallback source should select");
    assert_eq!(
        unknown_decision.selection_reason(),
        "unknown_fallback_preferred"
    );
    assert!(!unknown_decision.credit_backed_at_selection());
}
