use crate::burn_down::AccountAvailability;
use crate::burn_down::BurnDownAccountInput;
use crate::burn_down::BurnDownRouteBandAssessmentInput;
use crate::burn_down::CreditBackedEligibility;
use crate::burn_down::QuotaEvidenceReason;
use crate::burn_down::QuotaWindowFact;
use crate::burn_down::QuotaWindowStatus;
use crate::burn_down::RoutingReason;
use crate::burn_down::SelectedPool;
use crate::burn_down::V1_SHORT_WINDOW_SECONDS;
use crate::burn_down::V1_WEEKLY_WINDOW_SECONDS;
use crate::burn_down::assess_route_band;
use crate::selection_outcome::SelectionOutcome;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::RESPONSES_HTTP;
use codex_router_core::routes::RouteBand;

const NOW_UNIX_SECONDS: u64 = 1_000_000;

fn account_id(value: &str) -> AccountId {
    AccountId::new(value.to_owned())
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"))
}

fn weekly_window(status: QuotaWindowStatus, remaining: u32, has_reset: bool) -> QuotaWindowFact {
    quota_window(V1_WEEKLY_WINDOW_SECONDS, status, remaining, has_reset)
}

fn quota_window(
    window_seconds: u64,
    status: QuotaWindowStatus,
    remaining: u32,
    has_reset: bool,
) -> QuotaWindowFact {
    let mut window = QuotaWindowFact::new(window_seconds, status)
        .with_remaining_headroom(remaining)
        .with_observed_unix_seconds(NOW_UNIX_SECONDS)
        .with_effective(true);
    if has_reset {
        window = window.with_reset_unix_seconds(NOW_UNIX_SECONDS + window_seconds);
    }
    window
}

fn account(label: &str, remaining: u32, eligible_for_credit_backing: bool) -> BurnDownAccountInput {
    let mut account = BurnDownAccountInput::new(
        account_id(label),
        label,
        Provider::Openai,
        vec![weekly_window(QuotaWindowStatus::Eligible, remaining, true)],
    );
    if eligible_for_credit_backing {
        account = account.with_credit_backed_eligibility(CreditBackedEligibility::Eligible);
    }
    account
}

fn assess(
    accounts: Vec<BurnDownAccountInput>,
) -> crate::burn_down::BurnDownRouteBandAssessmentResult {
    assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        NOW_UNIX_SECONDS,
        RESPONSES_HTTP.clone(),
        accounts,
    ))
}

#[test]
fn exhausted_opted_in_account_uses_credit_reserve_after_included_reserve() {
    let included_reserve_id = account_id("included_reserve");
    let credit_account_id = account_id("credit_backed");

    let result = assess(vec![
        account("included_reserve", 10, false).with_current_active_sessions(1),
        account("credit_backed", 0, true),
    ]);

    assert_eq!(result.selected_pool(), SelectedPool::Reserve);
    assert_eq!(result.preferred_next(), Some(&included_reserve_id));
    let credit_account = result
        .accounts()
        .iter()
        .find(|assessment| assessment.account_id() == &credit_account_id)
        .expect("credit-backed account should remain visible in assessments");
    assert_eq!(credit_account.availability(), AccountAvailability::Reserve);
    assert_eq!(
        credit_account.quota_evidence_reason(),
        QuotaEvidenceReason::CreditBacked
    );
    assert_eq!(credit_account.routing_reason(), RoutingReason::CreditBacked);
    assert!(
        credit_account.is_healthy_floor_switch_peer(),
        "fresh credit Reserve should remain an eligible existing peer"
    );
    assert_eq!(
        credit_account.projected_weekly_runway_seconds(),
        None,
        "peer admission must not invent quota runway for provider credits"
    );
}

#[test]
fn eligible_credit_reserve_is_selected_after_included_quota_is_exhausted() {
    let credit_account_id = account_id("credit_only");
    let result = assess(vec![account("credit_only", 0, true)]);

    assert_eq!(result.selected_pool(), SelectedPool::Reserve);
    assert_eq!(result.preferred_next(), Some(&credit_account_id));
    let outcome = SelectionOutcome::chosen_from_assessment(&result)
        .expect("fresh opted-in credits should supply the reserve selection");
    let SelectionOutcome::Chosen { reason, .. } = outcome else {
        panic!("credit-backed assessment should produce a choice");
    };
    assert_eq!(reason.routing_reason(), RoutingReason::CreditBacked);
}

#[test]
fn a_known_zero_in_either_canonical_window_allows_credit_reserve() {
    let scenarios = [
        (
            "five_hour_zero_weekly_positive",
            vec![
                quota_window(
                    V1_SHORT_WINDOW_SECONDS,
                    QuotaWindowStatus::Ineligible,
                    0,
                    true,
                ),
                weekly_window(QuotaWindowStatus::Eligible, 20, true),
            ],
        ),
        (
            "weekly_zero_five_hour_positive",
            vec![
                quota_window(
                    V1_SHORT_WINDOW_SECONDS,
                    QuotaWindowStatus::Eligible,
                    40,
                    true,
                ),
                weekly_window(QuotaWindowStatus::Ineligible, 0, true),
            ],
        ),
    ];

    for (label, windows) in scenarios {
        let result = assess(vec![
            BurnDownAccountInput::new(account_id(label), label, Provider::Openai, windows)
                .with_credit_backed_eligibility(CreditBackedEligibility::Eligible),
        ]);

        assert_eq!(result.selected_pool(), SelectedPool::Reserve, "{label}");
        assert_eq!(result.preferred_next(), Some(&account_id(label)), "{label}");
        let assessment = &result.accounts()[0];
        assert_eq!(
            assessment.availability(),
            AccountAvailability::Reserve,
            "{label}"
        );
        assert_eq!(
            assessment.quota_evidence_reason(),
            QuotaEvidenceReason::CreditBacked,
            "{label}"
        );
        assert_eq!(
            assessment.routing_reason(),
            RoutingReason::CreditBacked,
            "{label}"
        );
    }
}

#[test]
fn credit_backing_rejects_zero_or_unhealthy_noncanonical_windows() {
    let scenarios = [
        (
            "noncanonical_zero_only",
            vec![
                quota_window(
                    V1_SHORT_WINDOW_SECONDS,
                    QuotaWindowStatus::Eligible,
                    40,
                    true,
                ),
                weekly_window(QuotaWindowStatus::Eligible, 20, true),
                quota_window(3_600, QuotaWindowStatus::Ineligible, 0, true),
            ],
        ),
        (
            "unrelated_ineligible",
            vec![
                quota_window(
                    V1_SHORT_WINDOW_SECONDS,
                    QuotaWindowStatus::Ineligible,
                    0,
                    true,
                ),
                weekly_window(QuotaWindowStatus::Eligible, 20, true),
                quota_window(3_600, QuotaWindowStatus::Ineligible, 0, true),
            ],
        ),
        (
            "unrelated_unknown",
            vec![
                quota_window(
                    V1_SHORT_WINDOW_SECONDS,
                    QuotaWindowStatus::Ineligible,
                    0,
                    true,
                ),
                weekly_window(QuotaWindowStatus::Eligible, 20, true),
                quota_window(3_600, QuotaWindowStatus::Unknown, 0, true),
            ],
        ),
        (
            "unrelated_stale",
            vec![
                quota_window(
                    V1_SHORT_WINDOW_SECONDS,
                    QuotaWindowStatus::Ineligible,
                    0,
                    true,
                ),
                weekly_window(QuotaWindowStatus::Eligible, 20, true),
                quota_window(3_600, QuotaWindowStatus::Stale, 0, true),
            ],
        ),
    ];

    for (label, windows) in scenarios {
        let result = assess(vec![
            BurnDownAccountInput::new(account_id(label), label, Provider::Openai, windows)
                .with_credit_backed_eligibility(CreditBackedEligibility::Eligible),
        ]);

        assert_ne!(
            result.accounts()[0].routing_reason(),
            RoutingReason::CreditBacked,
            "{label}"
        );
    }
}

#[test]
fn credit_backing_never_overrides_unknown_stale_missing_reset_or_weekly_floor() {
    let stale = BurnDownAccountInput::new(
        account_id("stale_credit"),
        "stale_credit",
        Provider::Openai,
        vec![weekly_window(QuotaWindowStatus::Stale, 0, true)],
    )
    .with_credit_backed_eligibility(CreditBackedEligibility::Eligible);
    let missing_reset = BurnDownAccountInput::new(
        account_id("missing_reset_credit"),
        "missing_reset_credit",
        Provider::Openai,
        vec![weekly_window(QuotaWindowStatus::Eligible, 0, false)],
    )
    .with_credit_backed_eligibility(CreditBackedEligibility::Eligible);
    let unknown = BurnDownAccountInput::new(
        account_id("unknown_credit"),
        "unknown_credit",
        Provider::Openai,
        vec![weekly_window(QuotaWindowStatus::Unknown, 0, true)],
    )
    .with_credit_backed_eligibility(CreditBackedEligibility::Eligible);
    let floored = account("floor_credit", 0, true).with_weekly_quota_floor_basis_points(1_000);
    let disabled = account("disabled_credit", 0, true).with_account_enabled(false);
    let missing_credential =
        account("credentialless_credit", 0, true).with_active_credential(false);

    let result = assess(vec![
        stale,
        missing_reset,
        unknown,
        floored,
        disabled,
        missing_credential,
    ]);

    for label in [
        "stale_credit",
        "missing_reset_credit",
        "unknown_credit",
        "floor_credit",
        "disabled_credit",
        "credentialless_credit",
    ] {
        let assessment = result
            .accounts()
            .iter()
            .find(|assessment| assessment.account_id() == &account_id(label))
            .expect("guarded account should remain visible");
        assert_ne!(
            assessment.routing_reason(),
            RoutingReason::CreditBacked,
            "{label}"
        );
        assert!(
            !assessment.is_healthy_floor_switch_peer(),
            "credit peer guard must preserve quota, floor, account, and credential guards for {label}"
        );
    }
}

#[test]
fn opted_out_exhausted_account_keeps_normal_blocked_quota_result() {
    let result = assess(vec![account("disallowed", 0, false)]);
    let assessment = &result.accounts()[0];

    assert_eq!(result.selected_pool(), SelectedPool::None);
    assert_eq!(result.preferred_next(), None);
    assert_eq!(assessment.availability(), AccountAvailability::Blocked);
    assert_eq!(
        assessment.quota_evidence_reason(),
        QuotaEvidenceReason::WindowExhausted
    );
    assert_eq!(
        assessment.routing_reason(),
        RoutingReason::BlockedWindowExhausted
    );
}
