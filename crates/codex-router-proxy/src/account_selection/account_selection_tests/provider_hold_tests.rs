use super::*;

#[test]
fn previous_response_owner_skips_lower_priority_session_affinity_lookup() {
    assert_eq!(
        super::session_affinity_lookup_session_id(Some("session-id"), true),
        None
    );
}

#[test]
fn claude_messages_route_uses_the_claude_selection_profile() {
    assert_eq!(
        super::route_profile_for_kind(RouteKind::ClaudeMessages),
        CLAUDE_MESSAGES
    );
}

#[test]
fn route_profile_provider_filter_removes_other_accounts_before_derived_delays() {
    let openai_account = codex_router_selection::burn_down::BurnDownAccountInput::new(
        account_id("acct_openai_short_reset"),
        "openai-short-reset",
        Provider::Openai,
        Vec::new(),
    );
    let claude_account = claude_quota_account("claude_short_reset", 80, 80);

    let claude_accounts = super::filter_selector_accounts_for_provider(
        vec![openai_account, claude_account],
        Provider::Claude,
    );

    assert_eq!(claude_accounts.len(), 1);
    assert_eq!(claude_accounts[0].provider(), Provider::Claude);
    assert_eq!(
        claude_accounts[0].account_id().as_str(),
        "acct_claude_short_reset"
    );
}

#[test]
fn claude_reserve_pin_yields_only_when_a_preferred_account_exists() {
    use codex_router_core::route_profile::CLAUDE_MESSAGES;
    use codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput;
    use codex_router_selection::burn_down::SelectedPool;
    use codex_router_selection::burn_down::assess_route_band;

    let reserve = claude_quota_account("reserve", 5, 80);
    let reserve_id = reserve.account_id().clone();
    let preferred = claude_quota_account("preferred", 6, 80);
    let preferred_assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        1_000_000,
        CLAUDE_MESSAGES.clone(),
        vec![reserve, preferred],
    ));

    assert_eq!(preferred_assessment.selected_pool(), SelectedPool::Usable);
    assert!(super::assessment_account_must_yield(
        &preferred_assessment,
        &reserve_id,
        &CLAUDE_MESSAGES,
    ));

    let lone_reserve = claude_quota_account("lone_reserve", 5, 80);
    let lone_reserve_id = lone_reserve.account_id().clone();
    let reserve_assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        1_000_000,
        CLAUDE_MESSAGES.clone(),
        vec![lone_reserve],
    ));

    assert_eq!(reserve_assessment.selected_pool(), SelectedPool::Reserve);
    assert!(!super::assessment_account_must_yield(
        &reserve_assessment,
        &lone_reserve_id,
        &CLAUDE_MESSAGES,
    ));
}

#[test]
fn claude_cooldown_does_not_replace_an_openai_hold_on_the_same_route_band() {
    use codex_router_core::route_profile::CLAUDE_MESSAGES;
    use codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput;
    use codex_router_selection::burn_down::assess_route_band;

    let openai_account_id = account_id("acct_openai_cooldown");
    let claude_account = claude_quota_account("claude_cooldown", 80, 80);
    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        1_000_000,
        CLAUDE_MESSAGES.clone(),
        vec![claude_account],
    ));
    let mut holds = HashMap::from([(
        super::ProviderRouteBand::new(Provider::Openai, RouteBand::Responses),
        super::AccountHold::new(openai_account_id.clone(), 999_990),
    )]);
    let mut weighted_selector =
        codex_router_selection::weighted_deficit::WeightedDeficitSelector::default();

    let selected = super::select_from_burn_down_assessment(
        "responses",
        Provider::Claude,
        &assessment,
        &mut weighted_selector,
        &mut holds,
        120,
        1_000_000,
    )
    .unwrap_or_else(|error| panic!("Claude selection should succeed: {error}"));

    assert_eq!(selected.account_id().as_str(), "acct_claude_cooldown");
    assert_eq!(
        holds
            .get(&super::ProviderRouteBand::new(
                Provider::Openai,
                RouteBand::Responses,
            ))
            .map(|hold| hold.account_id.as_str()),
        Some(openai_account_id.as_str()),
        "selecting Claude must not erase the OpenAI cooldown entry"
    );
    assert_eq!(
        holds
            .get(&super::ProviderRouteBand::new(
                Provider::Claude,
                RouteBand::Responses,
            ))
            .map(|hold| hold.account_id.as_str()),
        Some("acct_claude_cooldown")
    );
}
