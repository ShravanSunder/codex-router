use super::*;

#[path = "credit_compact_test_support.rs"]
mod support;

use support::CompactFixture;
use support::clear_selector_windows;
use support::compact_available_credits;
use support::mark_credit_refresh_pending;
use support::replace_selector_windows;
use support::set_provider_spend_control_reached;

#[test]
fn assembled_compact_uses_credit_after_canonical_responses_exhaustion() {
    let fixture = CompactFixture::new("compact_credit_fallback");
    fixture.add_credit_account(
        "acct_compact_exhausted_allow",
        "compact-exhausted-allow",
        "compact-credit-token",
        true,
        compact_available_credits("3.25"),
    );

    fixture.assert_route(
        "canonical_exhaustion_credit_fallback",
        Some("compact-credit-token"),
        Some("credit_backed"),
    );
}

#[test]
fn assembled_compact_selects_canonical_included_peer_before_credit_account() {
    let fixture = CompactFixture::new("compact_credit_included_peer");
    fixture.add_credit_account(
        "acct_compact_credit_exhausted",
        "compact-credit-exhausted",
        "must-not-select-credit-token",
        true,
        compact_available_credits("3.25"),
    );
    let included_account = fixture.add_credit_account(
        "acct_compact_included_peer",
        "compact-included-peer",
        "canonical-included-peer-token",
        true,
        compact_available_credits("4.50"),
    );
    replace_selector_windows(
        &fixture.database_path,
        included_account.account_id(),
        "responses",
        fixture.now_unix_seconds,
        SelectorQuotaWindowStatus::Eligible,
        72,
    );

    fixture.assert_route(
        "canonical_included_peer",
        Some("canonical-included-peer-token"),
        None,
    );
}

#[test]
fn assembled_disallow_compact_keeps_its_own_band_selection() {
    let fixture = CompactFixture::new("compact_disallow_own_band");
    let account = fixture.add_credit_account(
        "acct_compact_disallow",
        "compact-disallow",
        "disallow-compact-token",
        false,
        compact_available_credits("3.25"),
    );
    replace_selector_windows(
        &fixture.database_path,
        account.account_id(),
        "responses_compact",
        fixture.now_unix_seconds,
        SelectorQuotaWindowStatus::Eligible,
        72,
    );

    fixture.assert_route(
        "disallow_own_compact_band",
        Some("disallow-compact-token"),
        None,
    );
}

#[test]
fn assembled_compact_blocks_credit_without_entitlement_floor_or_clean_bands() {
    #[derive(Clone, Copy)]
    enum CompactCreditGuard {
        None,
        WeeklyFloor,
        SuspectRoute(&'static str),
        PendingRefresh,
        SpendControlReached,
    }

    let guard_cases = [
        (
            "zero_balance",
            codex_router_core::credit_usage::CreditAvailability::Available {
                balance: Some(
                    codex_router_core::credit_usage::CreditBalance::new("0")
                        .expect("zero balance should parse"),
                ),
            },
            CompactCreditGuard::None,
        ),
        (
            "depleted",
            codex_router_core::credit_usage::CreditAvailability::Depleted,
            CompactCreditGuard::None,
        ),
        (
            "unknown_entitlement",
            codex_router_core::credit_usage::CreditAvailability::Unknown,
            CompactCreditGuard::None,
        ),
        (
            "weekly_floor",
            compact_available_credits("3.25"),
            CompactCreditGuard::WeeklyFloor,
        ),
        (
            "compact_suspect_exhausted",
            compact_available_credits("3.25"),
            CompactCreditGuard::SuspectRoute("responses_compact"),
        ),
        (
            "responses_suspect_exhausted",
            compact_available_credits("3.25"),
            CompactCreditGuard::SuspectRoute("responses"),
        ),
        (
            "pending_credit_refresh",
            compact_available_credits("3.25"),
            CompactCreditGuard::PendingRefresh,
        ),
        (
            "provider_spend_control_reached",
            compact_available_credits("3.25"),
            CompactCreditGuard::SpendControlReached,
        ),
    ];

    for (case_name, availability, guard) in guard_cases {
        let fixture = CompactFixture::new(&format!("compact_credit_guard_{case_name}"));
        let account = fixture.add_credit_account(
            "acct_compact_credit_guard",
            case_name,
            "must-not-route-guarded-credit-token",
            true,
            availability,
        );

        match guard {
            CompactCreditGuard::None => {}
            CompactCreditGuard::WeeklyFloor => {
                set_weekly_floor_for_test(&fixture.database_path, account.label(), 500);
            }
            CompactCreditGuard::SuspectRoute(route_band) => {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("compact guard runtime should build");
                runtime.block_on(async {
                    let state = AsyncSqliteStateStore::open(&fixture.database_path)
                        .await
                        .expect("compact guard state should open");
                    state
                        .mark_route_band_quota_exhausted(
                            account.account_id(),
                            route_band,
                            fixture.now_unix_seconds,
                        )
                        .await
                        .expect("suspect exhaustion should persist");
                    state
                        .close()
                        .await
                        .expect("compact guard state should close");
                });
            }
            CompactCreditGuard::PendingRefresh => {
                mark_credit_refresh_pending(&fixture.database_path, account.account_id());
            }
            CompactCreditGuard::SpendControlReached => {
                set_provider_spend_control_reached(
                    &fixture.database_path,
                    account.account_id(),
                    fixture.now_unix_seconds,
                );
            }
        }

        fixture.assert_route(case_name, None, None);
    }
}

#[test]
fn assembled_compact_keeps_unknown_fallback_without_known_canonical_evidence() {
    for (case_name, status) in [
        ("canonical_stale", Some(SelectorQuotaWindowStatus::Stale)),
        (
            "canonical_unknown",
            Some(SelectorQuotaWindowStatus::Unknown),
        ),
        ("canonical_empty", None),
    ] {
        let fixture = CompactFixture::new(&format!("compact_credit_{case_name}"));
        let account = fixture.add_credit_account(
            "acct_compact_unknown_canonical",
            case_name,
            "unknown-canonical-token",
            true,
            compact_available_credits("3.25"),
        );
        if let Some(status) = status {
            replace_selector_windows(
                &fixture.database_path,
                account.account_id(),
                "responses",
                fixture.now_unix_seconds,
                status,
                0,
            );
        } else {
            clear_selector_windows(
                &fixture.database_path,
                account.account_id(),
                "responses",
                fixture.now_unix_seconds,
            );
        }

        fixture.assert_route(
            case_name,
            Some("unknown-canonical-token"),
            Some("unknown_fallback_preferred"),
        );
    }
}
