use super::CredentialStoreAvailability;
use super::HeadroomTimestamp;
use super::SelectionAccountRestriction;
use super::SelectionAccountState;
use super::SelectionHoldReason;
use super::SelectionOutcome;
use super::SelectionWindowRejection;
use super::UnavailableReason;
use super::classify_unavailable_reason;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::WindowKind;

fn account_id(label: &str) -> AccountId {
    AccountId::new(format!("acct_{label}"))
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"))
}

fn exhausted_account(label: &str, resets: Vec<Option<HeadroomTimestamp>>) -> SelectionAccountState {
    let rejections = resets
        .into_iter()
        .enumerate()
        .map(|(index, reset)| {
            SelectionWindowRejection::new(
                if index == 0 {
                    WindowKind::FiveHour
                } else {
                    WindowKind::Weekly
                },
                100,
                reset,
            )
        })
        .collect();
    SelectionAccountState::enabled(
        account_id(label),
        Provider::Claude,
        SelectionAccountRestriction::Exhausted,
        Vec::new(),
        rejections,
    )
}

fn enabled_account_state(
    label: &str,
    provider: Provider,
    restriction: SelectionAccountRestriction,
) -> SelectionAccountState {
    SelectionAccountState::enabled(
        account_id(label),
        provider,
        restriction,
        Vec::new(),
        Vec::new(),
    )
}

#[test]
fn unavailable_reasons_follow_the_r12_order() {
    let no_accounts = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::KeyUnreadable,
        &[],
    );
    assert_eq!(no_accounts, UnavailableReason::NoneConfiguredOrEnabled);
    assert_eq!(no_accounts.r12_order(), 1);

    let accounts_needing_login = [
        enabled_account_state(
            "login_a",
            Provider::Claude,
            SelectionAccountRestriction::NeedsLogin,
        ),
        enabled_account_state(
            "login_b",
            Provider::Claude,
            SelectionAccountRestriction::NeedsLogin,
        ),
    ];
    let key_unreadable = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::KeyUnreadable,
        &accounts_needing_login,
    );
    assert_eq!(key_unreadable, UnavailableReason::KeyUnreadable);
    assert_eq!(key_unreadable.r12_order(), 2);

    let needs_login = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &accounts_needing_login,
    );
    assert_eq!(
        needs_login,
        UnavailableReason::AllNeedLogin {
            accounts: vec![account_id("login_a"), account_id("login_b")],
        }
    );
    assert_eq!(needs_login.r12_order(), 3);

    let exhausted = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &[
            enabled_account_state(
                "needs_login",
                Provider::Claude,
                SelectionAccountRestriction::NeedsLogin,
            ),
            exhausted_account(
                "first",
                vec![
                    Some(HeadroomTimestamp::from_unix_seconds(300)),
                    Some(HeadroomTimestamp::from_unix_seconds(200)),
                ],
            ),
            exhausted_account(
                "second",
                vec![Some(HeadroomTimestamp::from_unix_seconds(250))],
            ),
        ],
    );
    assert_eq!(
        exhausted,
        UnavailableReason::AllExhausted {
            earliest_headroom: Some(HeadroomTimestamp::from_unix_seconds(250)),
        }
    );
    assert_eq!(exhausted.r12_order(), 4);

    let held = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &[
            enabled_account_state(
                "floor",
                Provider::Claude,
                SelectionAccountRestriction::HeldByFloor {
                    reason: SelectionHoldReason::HardFloor,
                },
            ),
            enabled_account_state(
                "stale_week",
                Provider::Claude,
                SelectionAccountRestriction::HeldByFloor {
                    reason: SelectionHoldReason::WaitingForFreshWeeklyObservation,
                },
            ),
        ],
    );
    assert!(matches!(held, UnavailableReason::HeldByFloors { .. }));
    assert_eq!(held.r12_order(), 5);
}

#[test]
fn all_exhausted_reset_hint_is_omitted_when_any_rejected_window_has_no_reset() {
    let reason = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &[
            exhausted_account(
                "known",
                vec![Some(HeadroomTimestamp::from_unix_seconds(300))],
            ),
            exhausted_account("unknown", vec![None]),
        ],
    );

    assert_eq!(
        reason,
        UnavailableReason::AllExhausted {
            earliest_headroom: None,
        }
    );
}

#[test]
fn unavailable_classification_filters_other_providers_and_disabled_accounts() {
    let reason = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &[
            enabled_account_state(
                "openai",
                Provider::Openai,
                SelectionAccountRestriction::Available,
            ),
            SelectionAccountState::disabled(account_id("disabled"), Provider::Claude),
        ],
    );

    assert_eq!(reason, UnavailableReason::NoneConfiguredOrEnabled);
}

#[test]
fn reason_five_lists_only_floor_and_missing_weekly_evidence_causes() {
    let reason = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &[
            enabled_account_state(
                "available_unknown_window",
                Provider::Claude,
                SelectionAccountRestriction::Available,
            ),
            enabled_account_state(
                "hard_floor",
                Provider::Claude,
                SelectionAccountRestriction::HeldByFloor {
                    reason: SelectionHoldReason::HardFloor,
                },
            ),
            enabled_account_state(
                "waiting_weekly",
                Provider::Claude,
                SelectionAccountRestriction::HeldByFloor {
                    reason: SelectionHoldReason::WaitingForFreshWeeklyObservation,
                },
            ),
        ],
    );

    let UnavailableReason::HeldByFloors { accounts } = reason else {
        panic!("floor-held account reasons should take R12 slot five");
    };
    assert_eq!(accounts.len(), 2);
    assert!(
        accounts
            .iter()
            .all(|account| { account.account_id() != &account_id("available_unknown_window") })
    );
}

#[test]
fn migration_incomplete_uses_the_second_r12_reason_slot() {
    let reason = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::MigrationIncomplete,
        &[enabled_account_state(
            "ready",
            Provider::Claude,
            SelectionAccountRestriction::Available,
        )],
    );

    assert_eq!(reason, UnavailableReason::MigrationIncomplete);
    assert_eq!(reason.r12_order(), 2);
}

#[test]
fn unavailable_credential_store_overrides_an_assessable_candidate() {
    use crate::burn_down::BurnDownAccountInput;
    use crate::burn_down::BurnDownRouteBandAssessmentInput;
    use crate::burn_down::QuotaWindowFact;
    use crate::burn_down::QuotaWindowStatus;
    use crate::burn_down::V1_SHORT_WINDOW_SECONDS;
    use crate::burn_down::V1_WEEKLY_WINDOW_SECONDS;
    use crate::burn_down::assess_route_band;
    use codex_router_core::route_profile::CLAUDE_MESSAGES;
    use codex_router_core::routes::RouteBand;

    let account_id = account_id("credential_store_unavailable");
    let now_unix_seconds = 1_000_000;
    let windows = vec![
        QuotaWindowFact::new(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
            .with_remaining_headroom(80)
            .with_reset_unix_seconds(now_unix_seconds + V1_SHORT_WINDOW_SECONDS)
            .with_observed_unix_seconds(now_unix_seconds),
        QuotaWindowFact::new(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
            .with_remaining_headroom(80)
            .with_reset_unix_seconds(now_unix_seconds + V1_WEEKLY_WINDOW_SECONDS)
            .with_observed_unix_seconds(now_unix_seconds),
    ];
    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        CLAUDE_MESSAGES.clone(),
        vec![BurnDownAccountInput::new(
            account_id.clone(),
            "ready",
            Provider::Claude,
            windows,
        )],
    ));
    let account_states = [enabled_account_state(
        "credential_store_unavailable",
        Provider::Claude,
        SelectionAccountRestriction::Available,
    )];

    assert_eq!(
        SelectionOutcome::from_assessment(
            &assessment,
            Provider::Claude,
            CredentialStoreAvailability::KeyUnreadable,
            &account_states,
        ),
        SelectionOutcome::Unavailable(UnavailableReason::KeyUnreadable),
        "R12 reason 2 prevents a candidate from being chosen when pooled credentials cannot be read"
    );

    assert!(matches!(
        SelectionOutcome::from_assessment(
            &assessment,
            Provider::Claude,
            CredentialStoreAvailability::Available,
            &account_states,
        ),
        SelectionOutcome::Chosen { account, .. } if account == account_id
    ));
}
