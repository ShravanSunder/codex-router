use super::*;

#[test]
fn short_quota_wait_uses_earliest_fresh_short_reset_when_weekly_is_usable() {
    let accounts = vec![
        short_only_exhausted_account_input("acct_short_a", 1_008),
        short_only_exhausted_account_input("acct_short_b", 1_005),
    ];

    assert_eq!(
        super::short_quota_wait_delay_seconds_with_jitter(&accounts, 1_000, 90),
        Some(95),
        "the first usable 5h reset plus the injected jitter should drive the retry delay"
    );

    let production_delay = super::short_quota_wait_delay_seconds(&accounts, 1_000);
    assert!(
        matches!(production_delay, Some(65..=125)),
        "production jitter must remain between one and two minutes: {production_delay:?}"
    );
    assert_eq!(
        super::short_quota_wait_delay_seconds_with_jitter(&accounts, 1_000, 60),
        Some(65),
        "minimum jitter should be one minute"
    );
    assert_eq!(
        super::short_quota_wait_delay_seconds_with_jitter(&accounts, 1_000, 120),
        Some(125),
        "maximum jitter should be two minutes"
    );
}

#[test]
fn short_quota_wait_rejects_weekly_exhaustion() {
    let account = codex_router_selection::burn_down::BurnDownAccountInput::new(
        account_id("acct_weekly_exhausted"),
        "weekly-exhausted",
        Provider::Openai,
        vec![
            codex_router_selection::burn_down::QuotaWindowFact::new(
                codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                codex_router_selection::burn_down::QuotaWindowStatus::Ineligible,
            )
            .with_remaining_headroom(0)
            .with_reset_unix_seconds(1_005),
            codex_router_selection::burn_down::QuotaWindowFact::new(
                codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                codex_router_selection::burn_down::QuotaWindowStatus::Ineligible,
            )
            .with_remaining_headroom(0)
            .with_reset_unix_seconds(100_000),
        ],
    );

    assert_eq!(
        super::short_quota_wait_delay_seconds(&[account], 1_000),
        None
    );
}

#[test]
fn short_quota_wait_rejects_stale_short_quota_evidence() {
    let account = codex_router_selection::burn_down::BurnDownAccountInput::new(
        account_id("acct_short_stale"),
        "short-stale",
        Provider::Openai,
        vec![
            codex_router_selection::burn_down::QuotaWindowFact::new(
                codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                codex_router_selection::burn_down::QuotaWindowStatus::Stale,
            )
            .with_remaining_headroom(0)
            .with_reset_unix_seconds(1_005),
            codex_router_selection::burn_down::QuotaWindowFact::new(
                codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(80)
            .with_reset_unix_seconds(100_000),
        ],
    );

    assert_eq!(
        super::short_quota_wait_delay_seconds(&[account], 1_000),
        None
    );
}

#[test]
fn test_short_quota_jitter_override_is_positive_bounded_and_fails_closed() {
    assert_eq!(super::bounded_positive_test_jitter(Some("2")), Some(2));
    for invalid in ["", "0", "121", "not-a-number"] {
        assert_eq!(super::bounded_positive_test_jitter(Some(invalid)), None);
    }
    assert_eq!(super::bounded_positive_test_jitter(None), None);
}
