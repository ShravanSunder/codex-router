use super::account_id;
use super::read_response;
use crate::server::claude_selection_unavailable_response;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::WindowKind;
use codex_router_selection::selection_outcome::CredentialStoreAvailability;
use codex_router_selection::selection_outcome::HeadroomTimestamp;
use codex_router_selection::selection_outcome::SelectionAccountRestriction;
use codex_router_selection::selection_outcome::SelectionAccountState;
use codex_router_selection::selection_outcome::SelectionHoldReason;
use codex_router_selection::selection_outcome::SelectionWindowRejection;
use codex_router_selection::selection_outcome::UnavailableReason;
use codex_router_selection::selection_outcome::classify_unavailable_reason;
use http::StatusCode;

#[test]
fn claude_unavailable_reason_precedence_matches_r12() {
    let disabled = [SelectionAccountState::disabled(
        account_id("disabled"),
        Provider::Claude,
    )];
    let no_enabled_account = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::KeyUnreadable,
        &disabled,
    );
    assert_eq!(no_enabled_account.r12_order(), 1);
    assert_eq!(
        no_enabled_account,
        UnavailableReason::NoneConfiguredOrEnabled
    );

    let needs_login = [SelectionAccountState::enabled(
        account_id("login"),
        Provider::Claude,
        SelectionAccountRestriction::NeedsLogin,
        Vec::new(),
        Vec::new(),
    )];
    let key_unreadable = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::KeyUnreadable,
        &needs_login,
    );
    assert_eq!(key_unreadable.r12_order(), 2);
    assert_eq!(key_unreadable, UnavailableReason::KeyUnreadable);

    let all_need_login = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &needs_login,
    );
    assert_eq!(all_need_login.r12_order(), 3);
    assert!(matches!(
        all_need_login,
        UnavailableReason::AllNeedLogin { .. }
    ));

    let login_and_exhausted = [
        needs_login[0].clone(),
        SelectionAccountState::enabled(
            account_id("exhausted"),
            Provider::Claude,
            SelectionAccountRestriction::Exhausted,
            Vec::new(),
            Vec::new(),
        ),
    ];
    let all_usable_accounts_exhausted = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &login_and_exhausted,
    );
    assert_eq!(all_usable_accounts_exhausted.r12_order(), 4);
    assert!(matches!(
        all_usable_accounts_exhausted,
        UnavailableReason::AllExhausted { .. }
    ));

    let held_by_floor = [SelectionAccountState::enabled(
        account_id("held"),
        Provider::Claude,
        SelectionAccountRestriction::HeldByFloor {
            reason: SelectionHoldReason::WaitingForFreshWeeklyObservation,
        },
        Vec::new(),
        Vec::new(),
    )];
    let held_accounts = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &held_by_floor,
    );
    assert_eq!(held_accounts.r12_order(), 5);
    assert!(matches!(
        held_accounts,
        UnavailableReason::HeldByFloors { .. }
    ));
}

#[tokio::test]
async fn claude_unavailable_responses_explain_account_and_reason() {
    let login_account = account_id("login-one");
    let hard_floor_account = account_id("hard-floor");
    let weekly_wait_account = account_id("weekly-wait");
    let account_labels = vec![
        (login_account.clone(), "Claude One".to_owned()),
        (hard_floor_account.clone(), "Floor Account".to_owned()),
        (weekly_wait_account.clone(), "Weekly Account".to_owned()),
    ];

    let (status, _headers, body) = read_response(claude_selection_unavailable_response(
        UnavailableReason::NoneConfiguredOrEnabled,
        &account_labels,
        1_000,
        None,
    ))
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["type"], "api_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("No Claude account is configured or enabled"))
    );

    for (reason, expected_detail) in [
        (
            UnavailableReason::KeyUnreadable,
            "Keychain key is unreadable",
        ),
        (
            UnavailableReason::MigrationIncomplete,
            "pooled-credential migration is incomplete",
        ),
    ] {
        let (status, _headers, body) = read_response(claude_selection_unavailable_response(
            reason,
            &account_labels,
            1_000,
            None,
        ))
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let message = body["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("Claude error message should be present"));
        assert!(message.contains("Router cannot unlock its Claude credentials"));
        assert!(message.contains(expected_detail));
    }

    let (status, _headers, body) = read_response(claude_selection_unavailable_response(
        UnavailableReason::AllNeedLogin {
            accounts: vec![login_account],
        },
        &account_labels,
        1_000,
        None,
    ))
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let login_message = body["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("Claude error message should be present"));
    assert!(login_message.contains("Claude One"));
    assert!(login_message.contains("account login"));

    let (status, headers, body) = read_response(claude_selection_unavailable_response(
        UnavailableReason::AllExhausted {
            earliest_headroom: Some(HeadroomTimestamp::from_unix_seconds(1_045)),
        },
        &account_labels,
        1_000,
        None,
    ))
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["type"], "rate_limit_error");
    assert_eq!(body["error"]["code"], "usage_limit_reached");
    assert!(body["error"]["message"].as_str().is_some_and(|message| {
        message.contains("usage limit") && message.contains("45 seconds")
    }));
    assert_eq!(
        headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("45")
    );

    let (status, headers, body) = read_response(claude_selection_unavailable_response(
        UnavailableReason::AllExhausted {
            earliest_headroom: None,
        },
        &account_labels,
        1_000,
        None,
    ))
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.get("retry-after").is_none());
    assert_eq!(body["error"]["code"], "usage_limit_reached");
    assert!(
        !body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("seconds"))
    );

    let waiting_reason = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &[
            SelectionAccountState::enabled(
                hard_floor_account,
                Provider::Claude,
                SelectionAccountRestriction::HeldByFloor {
                    reason: SelectionHoldReason::HardFloor,
                },
                Vec::new(),
                Vec::new(),
            ),
            SelectionAccountState::enabled(
                weekly_wait_account,
                Provider::Claude,
                SelectionAccountRestriction::HeldByFloor {
                    reason: SelectionHoldReason::WaitingForFreshWeeklyObservation,
                },
                Vec::new(),
                Vec::new(),
            ),
        ],
    );
    let (status, _headers, body) = read_response(claude_selection_unavailable_response(
        waiting_reason,
        &account_labels,
        1_000,
        None,
    ))
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let held_message = body["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("Claude error message should be present"));
    assert!(held_message.contains("Floor Account"));
    assert!(held_message.contains("hard quota floor"));
    assert!(held_message.contains("Weekly Account"));
    assert!(held_message.contains("fresh weekly quota observation"));
}

#[tokio::test]
async fn exhaustion_reset_hint_waits_for_every_rejected_window_per_account() {
    let account_a = SelectionAccountState::enabled(
        account_id("two-windows"),
        Provider::Claude,
        SelectionAccountRestriction::Exhausted,
        Vec::new(),
        vec![
            SelectionWindowRejection::new(
                WindowKind::FiveHour,
                1_000,
                Some(HeadroomTimestamp::from_unix_seconds(1_300)),
            ),
            SelectionWindowRejection::new(
                WindowKind::Weekly,
                1_000,
                Some(HeadroomTimestamp::from_unix_seconds(2_000)),
            ),
        ],
    );
    let account_b = SelectionAccountState::enabled(
        account_id("one-window"),
        Provider::Claude,
        SelectionAccountRestriction::Exhausted,
        Vec::new(),
        vec![SelectionWindowRejection::new(
            WindowKind::FiveHour,
            1_000,
            Some(HeadroomTimestamp::from_unix_seconds(1_500)),
        )],
    );
    let unavailable = classify_unavailable_reason(
        Provider::Claude,
        CredentialStoreAvailability::Available,
        &[account_a, account_b],
    );
    let UnavailableReason::AllExhausted { earliest_headroom } = unavailable else {
        panic!("all usable accounts are exhausted")
    };
    assert_eq!(
        earliest_headroom,
        Some(HeadroomTimestamp::from_unix_seconds(1_500))
    );

    let response = claude_selection_unavailable_response(
        UnavailableReason::AllExhausted { earliest_headroom },
        &[],
        1_000,
        None,
    );
    let (status, headers, body) = read_response(response).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("500")
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("500 seconds"))
    );
}
