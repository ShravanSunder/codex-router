use codex_router_core::ids::AccountId;
use codex_router_core::routes::RouteBand;
use codex_router_selection::burn_down::BurnDownAccountInput;
use codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput;
use codex_router_selection::burn_down::QuotaWindowFact;
use codex_router_selection::burn_down::QuotaWindowStatus;
use codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS;
use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;
use codex_router_selection::burn_down::assess_route_band;
use serde_json::Value;
use serde_json::json;

const NOW: u64 = 1_000_000;

fn account_id(value: &str) -> AccountId {
    AccountId::new(value.to_owned()).unwrap_or_else(|error| panic!("valid fixture id: {error}"))
}

fn window(
    window_seconds: u64,
    status: QuotaWindowStatus,
    remaining_headroom: u32,
    reset_after_seconds: u64,
) -> QuotaWindowFact {
    QuotaWindowFact::new(window_seconds, status)
        .with_remaining_headroom(remaining_headroom)
        .with_reset_unix_seconds(NOW + reset_after_seconds)
        .with_observed_unix_seconds(NOW)
        .with_effective(true)
}

fn account(id: &str, windows: Vec<QuotaWindowFact>) -> BurnDownAccountInput {
    BurnDownAccountInput::new(account_id(id), id, windows)
}

fn long_window(remaining: u32, reset_after_seconds: u64) -> QuotaWindowFact {
    window(
        V1_WEEKLY_WINDOW_SECONDS,
        QuotaWindowStatus::Eligible,
        remaining,
        reset_after_seconds,
    )
}

fn far_idle_account(id: &str, remaining: u32, reset_after_seconds: u64, burn: u32) -> BurnDownAccountInput {
    let projected_runout_seconds = u64::from(remaining)
        .saturating_mul(100)
        .saturating_mul(3_600)
        .checked_div(u64::from(burn))
        .unwrap_or(u64::MAX);
    account(
        id,
        vec![
            window(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible, 100, 4 * 3_600),
            long_window(remaining, reset_after_seconds)
                .with_projected_exhaustion_unix_seconds(NOW + projected_runout_seconds)
                .with_per_connection_burn_basis_points_per_hour(burn),
        ],
    )
}

fn cases() -> Vec<(&'static str, Vec<BurnDownAccountInput>)> {
    let short_guard = || {
        account(
            "acct_short_guard",
            vec![
                window(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible, 60, 9_000)
                    .with_projected_exhaustion_unix_seconds(NOW + 5_000),
                long_window(70, 500_000),
            ],
        )
    };

    let floor_switch_account = || {
        account(
            "acct_floor_switch",
            vec![
                window(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible, 100, 4 * 3_600),
                long_window(8, 20 * 3_600),
            ],
        )
        .with_weekly_quota_floor_basis_points(500)
    };

    let initial_admission_accounts = || {
        vec![
            account(
                "acct_initial_admission",
                vec![
                    window(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible, 100, 4 * 3_600),
                    long_window(20, 24 * 3_600),
                ],
            ),
            far_idle_account("acct_known_later", 34, 96 * 3_600, 20),
            far_idle_account("acct_healthy_later", 80, 7 * 86_400, 20),
        ]
    };

    let far_idle_accounts = |first_active_sessions| {
        vec![
            far_idle_account("acct_seven_53h", 7, 53 * 3_600, 82)
                .with_current_active_sessions(first_active_sessions),
            far_idle_account("acct_two_97h", 2, 97 * 3_600, 14),
            far_idle_account("acct_busy_healthy", 80, 5 * 86_400, 30)
                .with_current_active_sessions(1),
        ]
    };

    vec![
        (
            "long-headroom-threshold-at-ten",
            vec![account("acct_long_at_ten", vec![long_window(10, 200_000)])
                .with_current_active_sessions(1)],
        ),
        (
            "long-headroom-threshold-above-ten",
            vec![account("acct_long_above_ten", vec![long_window(11, 200_000)])
                .with_current_active_sessions(1)],
        ),
        (
            "long-pressure-threshold-at-twenty-five",
            vec![account("acct_pressure_at_25", vec![long_window(58, 500_000)])
                .with_current_active_sessions(1)],
        ),
        (
            "long-pressure-threshold-below-twenty-five",
            vec![account("acct_pressure_below_25", vec![long_window(59, 500_000)])
                .with_current_active_sessions(1)],
        ),
        ("short-guard-last-resort", vec![short_guard()]),
        (
            "short-guard-with-usable-peer",
            vec![
                short_guard(),
                account("acct_usable_peer", vec![long_window(70, 500_000)])
                    .with_current_active_sessions(1),
            ],
        ),
        (
            "held-floor-switch-early-peer",
            vec![
                floor_switch_account(),
                far_idle_account("acct_healthy_peer", 60, 97 * 3_600, 14),
            ],
        ),
        (
            "unknown-evidence-only",
            vec![account(
                "acct_unknown",
                vec![window(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Unknown, 50, 5 * 86_400)],
            )],
        ),
        (
            "stale-evidence-only",
            vec![account(
                "acct_stale",
                vec![window(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Stale, 50, 5 * 86_400)],
            )],
        ),
        (
            "configured-floor-hard-stop",
            vec![account("acct_hard_floor", vec![long_window(5, 5 * 86_400)])
                .with_weekly_quota_floor_basis_points(500)],
        ),
        ("initial-admission", initial_admission_accounts()),
        ("far-idle-priority-first", far_idle_accounts(0)),
        ("far-idle-priority-after-first-start", far_idle_accounts(1)),
    ]
}

fn snapshot(name: &str, accounts: Vec<BurnDownAccountInput>) -> Value {
    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        NOW,
        accounts,
    ));

    json!({
        "case": name,
        "selected_pool": format!("{:?}", assessment.selected_pool()),
        "preferred_next": assessment.preferred_next().map(AccountId::as_str),
        "weighted_candidates": assessment.weighted_candidates().iter().map(|(id, weight)| {
            json!({ "account_id": id.as_str(), "weight": weight })
        }).collect::<Vec<_>>(),
        "account_assessments": assessment.accounts().iter().map(|account| {
            json!({
                "account_id": account.account_id().as_str(),
                "availability": format!("{:?}", account.availability()),
                "freshness": format!("{:?}", account.freshness()),
                "routing_exclusion": format!("{:?}", account.routing_exclusion()),
                "quota_evidence_reason": format!("{:?}", account.quota_evidence_reason()),
                "routing_reason": account.routing_reason().as_str(),
                "routing_weight": account.routing_weight(),
                "preferred_next": account.preferred_next(),
                "initial_admission_priority": account.has_initial_admission_priority(),
                "floor_switch_band": account.in_weekly_floor_switch_band(),
                "short_pressure": account.short_pressure(),
                "long_pressure": account.long_pressure(),
            })
        }).collect::<Vec<_>>(),
    })
}

fn main() {
    let snapshots = cases()
        .into_iter()
        .map(|(name, accounts)| snapshot(name, accounts))
        .collect::<Vec<_>>();
    println!("{}", serde_json::to_string_pretty(&json!({
        "oracle_commit": "05b0be91c7fbdc42c0ed429d5518755b9112c674",
        "cases": snapshots,
    })).unwrap_or_else(|error| panic!("oracle JSON should serialize: {error}")));
}
