use crate::burn_down::AccountAvailability;
use crate::burn_down::BurnDownAccountInput;
use crate::burn_down::BurnDownRouteBandAssessmentInput;
use crate::burn_down::QuotaWindowFact;
use crate::burn_down::QuotaWindowStatus;
use crate::burn_down::SelectedPool;
use crate::burn_down::V1_SHORT_WINDOW_SECONDS;
use crate::burn_down::V1_WEEKLY_WINDOW_SECONDS;
use crate::burn_down::assess_route_band;
use crate::selection_outcome::SelectionOutcome;
use crate::selection_outcome::Tier;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::CLAUDE_MESSAGES;
use codex_router_core::route_profile::RESPONSES_HTTP;
use codex_router_core::route_profile::RESPONSES_WEBSOCKET;
use codex_router_core::route_profile::WindowKind;
use codex_router_core::route_profile::WindowPolicy;
use codex_router_core::route_profile::WindowRule;
use codex_router_core::routes::RouteBand;

const NOW_UNIX_SECONDS: u64 = 1_000_000;

fn account_id(value: &str) -> AccountId {
    AccountId::new(value.to_owned())
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"))
}

fn quota_window(
    window_seconds: u64,
    status: QuotaWindowStatus,
    remaining_headroom: u32,
) -> QuotaWindowFact {
    QuotaWindowFact::new(window_seconds, status)
        .with_remaining_headroom(remaining_headroom)
        .with_reset_unix_seconds(NOW_UNIX_SECONDS + window_seconds)
        .with_observed_unix_seconds(NOW_UNIX_SECONDS)
        .with_effective(true)
}

fn claude_account(
    account_label: &str,
    five_hour_status: QuotaWindowStatus,
    five_hour_remaining: u32,
    weekly_status: QuotaWindowStatus,
    weekly_remaining: u32,
) -> BurnDownAccountInput {
    BurnDownAccountInput::new(
        account_id(&format!("acct_{account_label}")),
        account_label,
        vec![
            quota_window(
                V1_SHORT_WINDOW_SECONDS,
                five_hour_status,
                five_hour_remaining,
            ),
            quota_window(V1_WEEKLY_WINDOW_SECONDS, weekly_status, weekly_remaining),
        ],
    )
    .with_provider(Provider::Claude)
}

fn claude_assessment_input(
    accounts: Vec<BurnDownAccountInput>,
) -> BurnDownRouteBandAssessmentInput {
    BurnDownRouteBandAssessmentInput::new(RouteBand::Responses, NOW_UNIX_SECONDS, accounts)
        .with_route_profile(CLAUDE_MESSAGES)
}

#[test]
fn claude_five_hour_threshold_releases_reserve_only_when_preferred_peer_exists() {
    let reserve_account = claude_account(
        "reserve",
        QuotaWindowStatus::Eligible,
        5,
        QuotaWindowStatus::Eligible,
        80,
    );
    let preferred_account = claude_account(
        "preferred",
        QuotaWindowStatus::Eligible,
        6,
        QuotaWindowStatus::Eligible,
        80,
    );
    let reserve_id = reserve_account.account_id().clone();
    let preferred_id = preferred_account.account_id().clone();

    let assessment = assess_route_band(claude_assessment_input(vec![
        reserve_account,
        preferred_account,
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Usable);
    assert_eq!(assessment.preferred_next(), Some(&preferred_id));
    assert_eq!(
        assessment
            .accounts()
            .iter()
            .find(|account| account.account_id() == &reserve_id)
            .map(|account| account.availability()),
        Some(AccountAvailability::Reserve)
    );
}

#[test]
fn a_lone_claude_reserve_account_stays_selectable() {
    let reserve_account = claude_account(
        "only",
        QuotaWindowStatus::Eligible,
        5,
        QuotaWindowStatus::Eligible,
        80,
    );
    let reserve_id = reserve_account.account_id().clone();

    let assessment = assess_route_band(claude_assessment_input(vec![reserve_account]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Reserve);
    assert_eq!(assessment.preferred_next(), Some(&reserve_id));
    let outcome = SelectionOutcome::chosen_from_assessment(&assessment);
    assert!(matches!(
        &outcome,
        Some(SelectionOutcome::Chosen {
            tier: Tier::Reserve,
            ..
        })
    ));
    if let Some(SelectionOutcome::Chosen { account, tier, .. }) = outcome {
        assert_eq!(account, reserve_id);
        assert_eq!(tier, Tier::Reserve);
    }
}

#[test]
fn stale_or_unknown_claude_windows_select_only_from_the_unknown_tier() {
    let stale_account = claude_account(
        "stale",
        QuotaWindowStatus::Stale,
        99,
        QuotaWindowStatus::Eligible,
        99,
    );
    let unknown_account = claude_account(
        "unknown",
        QuotaWindowStatus::Unknown,
        99,
        QuotaWindowStatus::Eligible,
        99,
    );
    let stale_id = stale_account.account_id().clone();
    let unknown_id = unknown_account.account_id().clone();

    let assessment = assess_route_band(claude_assessment_input(vec![
        stale_account,
        unknown_account,
    ]));

    assert_eq!(assessment.selected_pool(), SelectedPool::Unknown);
    assert_eq!(assessment.preferred_next(), Some(&stale_id));
    assert_eq!(
        assessment
            .accounts()
            .iter()
            .find(|account| account.account_id() == &unknown_id)
            .map(|account| account.availability()),
        Some(AccountAvailability::Unknown)
    );
}

#[test]
fn configured_near_full_threshold_and_weekly_floor_switch_are_applied() {
    let near_full_account = claude_account(
        "near-full",
        QuotaWindowStatus::Eligible,
        10,
        QuotaWindowStatus::Eligible,
        80,
    );
    let healthy_account = claude_account(
        "healthy",
        QuotaWindowStatus::Eligible,
        11,
        QuotaWindowStatus::Eligible,
        80,
    );
    let near_full_id = near_full_account.account_id().clone();
    let healthy_id = healthy_account.account_id().clone();
    let configured_policies = vec![
        WindowPolicy {
            kind: WindowKind::FiveHour,
            rule: WindowRule::NearFullReserve { percent: 90 },
        },
        WindowPolicy {
            kind: WindowKind::Weekly,
            rule: WindowRule::WeeklyFloor {
                early_switch_bps: 300,
            },
        },
    ];

    let threshold_assessment = assess_route_band(
        claude_assessment_input(vec![near_full_account, healthy_account])
            .with_window_policies(configured_policies),
    );

    assert_eq!(threshold_assessment.preferred_next(), Some(&healthy_id));
    assert_eq!(
        threshold_assessment
            .accounts()
            .iter()
            .find(|account| account.account_id() == &near_full_id)
            .map(|account| account.availability()),
        Some(AccountAvailability::Reserve)
    );

    let floor_switch_account = claude_account(
        "floor-switch",
        QuotaWindowStatus::Eligible,
        80,
        QuotaWindowStatus::Eligible,
        13,
    )
    .with_weekly_quota_floor_basis_points(1_000);
    let floor_switch_id = floor_switch_account.account_id().clone();
    let floor_assessment = assess_route_band(claude_assessment_input(vec![floor_switch_account]));
    assert_eq!(
        floor_assessment
            .accounts()
            .iter()
            .find(|account| account.account_id() == &floor_switch_id)
            .map(|account| account.availability()),
        Some(AccountAvailability::Reserve)
    );
}

#[test]
fn claude_does_not_use_the_openai_last_resort_pool() {
    let account = BurnDownAccountInput::new(
        account_id("acct_short_guard"),
        "short_guard",
        vec![
            QuotaWindowFact::new(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
                .with_remaining_headroom(60)
                .with_reset_unix_seconds(NOW_UNIX_SECONDS + 9_000)
                .with_observed_unix_seconds(NOW_UNIX_SECONDS)
                .with_projected_exhaustion_unix_seconds(NOW_UNIX_SECONDS + 5_000),
            quota_window(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Eligible, 80),
        ],
    )
    .with_provider(Provider::Claude);

    let assessment = assess_route_band(claude_assessment_input(vec![account]));

    assert_ne!(assessment.selected_pool(), SelectedPool::LastResort);
    assert!(assessment.preferred_next().is_some());
}

#[test]
fn profile_provider_filters_candidates_before_assessment() {
    let claude = claude_account(
        "claude",
        QuotaWindowStatus::Eligible,
        80,
        QuotaWindowStatus::Eligible,
        80,
    );
    let openai = BurnDownAccountInput::new(
        account_id("acct_openai"),
        "openai",
        vec![quota_window(
            V1_WEEKLY_WINDOW_SECONDS,
            QuotaWindowStatus::Eligible,
            90,
        )],
    )
    .with_provider(Provider::Openai);

    let assessment = assess_route_band(claude_assessment_input(vec![claude, openai]));

    assert_eq!(assessment.accounts().len(), 1);
    assert_eq!(
        assessment.accounts()[0].account_id().as_str(),
        "acct_claude"
    );
}

#[test]
fn explicit_openai_profile_matches_the_legacy_selector_branch_space() {
    let regular = BurnDownAccountInput::new(
        account_id("acct_regular"),
        "regular",
        vec![quota_window(
            V1_WEEKLY_WINDOW_SECONDS,
            QuotaWindowStatus::Eligible,
            70,
        )],
    );
    let near_floor = BurnDownAccountInput::new(
        account_id("acct_floor"),
        "floor",
        vec![quota_window(
            V1_WEEKLY_WINDOW_SECONDS,
            QuotaWindowStatus::Eligible,
            13,
        )],
    )
    .with_weekly_quota_floor_basis_points(1_000);
    let long_window_reserve = BurnDownAccountInput::new(
        account_id("acct_long_reserve"),
        "long_reserve",
        vec![
            quota_window(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Eligible, 10)
                .with_reset_unix_seconds(NOW_UNIX_SECONDS + 500_000),
        ],
    );
    let long_window_at_reserve_threshold = BurnDownAccountInput::new(
        account_id("acct_long_threshold"),
        "long_threshold",
        vec![
            quota_window(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Eligible, 25)
                .with_reset_unix_seconds(NOW_UNIX_SECONDS + 500_000),
        ],
    );
    let long_window_below_reserve_threshold = BurnDownAccountInput::new(
        account_id("acct_long_below_threshold"),
        "long_below_threshold",
        vec![
            quota_window(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Eligible, 76)
                .with_reset_unix_seconds(NOW_UNIX_SECONDS + 500_000),
        ],
    );
    let short_guard = BurnDownAccountInput::new(
        account_id("acct_short"),
        "short",
        vec![
            QuotaWindowFact::new(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
                .with_remaining_headroom(60)
                .with_reset_unix_seconds(NOW_UNIX_SECONDS + 9_000)
                .with_observed_unix_seconds(NOW_UNIX_SECONDS)
                .with_projected_exhaustion_unix_seconds(NOW_UNIX_SECONDS + 5_000),
            quota_window(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Eligible, 70),
        ],
    );
    let stale = BurnDownAccountInput::new(
        account_id("acct_stale"),
        "stale",
        vec![quota_window(
            V1_WEEKLY_WINDOW_SECONDS,
            QuotaWindowStatus::Stale,
            80,
        )],
    );
    let unknown = BurnDownAccountInput::new(
        account_id("acct_unknown"),
        "unknown",
        vec![quota_window(
            V1_WEEKLY_WINDOW_SECONDS,
            QuotaWindowStatus::Unknown,
            80,
        )],
    );
    let configured_floor_hard_stop = BurnDownAccountInput::new(
        account_id("acct_floor_hard_stop"),
        "floor_hard_stop",
        vec![quota_window(
            V1_WEEKLY_WINDOW_SECONDS,
            QuotaWindowStatus::Eligible,
            10,
        )],
    )
    .with_weekly_quota_floor_basis_points(1_000);
    let examples = [
        vec![],
        vec![regular.clone()],
        vec![near_floor, regular.clone()],
        vec![long_window_reserve],
        vec![long_window_at_reserve_threshold],
        vec![long_window_below_reserve_threshold],
        vec![short_guard.clone()],
        vec![short_guard, regular.clone()],
        vec![stale.clone(), unknown.clone()],
        vec![stale],
        vec![unknown],
        vec![configured_floor_hard_stop, regular.clone()],
        vec![regular.clone().with_account_enabled(false)],
        vec![regular.with_active_credential(false)],
    ];

    for accounts in examples {
        let legacy_input =
            BurnDownRouteBandAssessmentInput::new(RouteBand::Responses, NOW_UNIX_SECONDS, accounts);
        let legacy_assessment = assess_route_band(legacy_input.clone());
        for route_profile in [RESPONSES_HTTP, RESPONSES_WEBSOCKET] {
            let profile_assessment =
                assess_route_band(legacy_input.clone().with_route_profile(route_profile));
            assert_eq!(
                legacy_assessment, profile_assessment,
                "explicit LegacyOpenAi profile must preserve all current OpenAI decisions"
            );
        }
    }
}
