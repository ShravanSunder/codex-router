use super::*;

pub(super) fn claude_quota_account(
    account_label: &str,
    five_hour_remaining: u32,
    weekly_remaining: u32,
) -> codex_router_selection::burn_down::BurnDownAccountInput {
    use codex_router_selection::burn_down::BurnDownAccountInput;
    use codex_router_selection::burn_down::QuotaWindowFact;
    use codex_router_selection::burn_down::QuotaWindowStatus;
    use codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS;
    use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;

    let now_unix_seconds = 1_000_000;
    let account_id = AccountId::new(format!("acct_{account_label}"))
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"));
    BurnDownAccountInput::new(
        account_id,
        account_label,
        Provider::Claude,
        vec![
            QuotaWindowFact::new(V1_SHORT_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
                .with_remaining_headroom(five_hour_remaining)
                .with_reset_unix_seconds(now_unix_seconds + V1_SHORT_WINDOW_SECONDS)
                .with_observed_unix_seconds(now_unix_seconds),
            QuotaWindowFact::new(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Eligible)
                .with_remaining_headroom(weekly_remaining)
                .with_reset_unix_seconds(now_unix_seconds + V1_WEEKLY_WINDOW_SECONDS)
                .with_observed_unix_seconds(now_unix_seconds),
        ],
    )
}

pub(super) fn short_only_exhausted_account_input(
    account_id_value: &str,
    short_reset_unix_seconds: u64,
) -> codex_router_selection::burn_down::BurnDownAccountInput {
    codex_router_selection::burn_down::BurnDownAccountInput::new(
        account_id(account_id_value),
        account_id_value,
        Provider::Openai,
        vec![
            codex_router_selection::burn_down::QuotaWindowFact::new(
                codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                codex_router_selection::burn_down::QuotaWindowStatus::Ineligible,
            )
            .with_remaining_headroom(0)
            .with_reset_unix_seconds(short_reset_unix_seconds),
            codex_router_selection::burn_down::QuotaWindowFact::new(
                codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(80)
            .with_reset_unix_seconds(100_000),
        ],
    )
}

pub(super) fn fake_pin_is_active_at(
    last_seen_unix_seconds: u64,
    publication_unix_seconds: u64,
    pin_ttl_seconds: u64,
) -> bool {
    publication_unix_seconds < pin_ttl_seconds
        || last_seen_unix_seconds > publication_unix_seconds - pin_ttl_seconds
}

pub(super) fn account_input_for_runtime_exhaustion_test(
    account_id: AccountId,
) -> codex_router_selection::burn_down::BurnDownAccountInput {
    codex_router_selection::burn_down::BurnDownAccountInput::new(
        account_id,
        "runtime-test",
        Provider::Openai,
        vec![
            codex_router_selection::burn_down::QuotaWindowFact::new(
                codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(90)
            .with_reset_unix_seconds(18_000),
            codex_router_selection::burn_down::QuotaWindowFact::new(
                codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(90)
            .with_reset_unix_seconds(604_800),
        ],
    )
}

pub(super) fn selector_input_for_runtime_exhaustion_test(
    account_id: AccountId,
) -> codex_router_state::quota_snapshot::SelectorQuotaInput {
    selector_input_for_post_exhaustion_test(
        account_id,
        "runtime-test",
        codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Eligible,
    )
}

pub(super) fn selector_input_for_post_exhaustion_test(
    account_id: AccountId,
    account_label: &str,
    status: codex_router_state::quota_snapshot::SelectorQuotaWindowStatus,
) -> codex_router_state::quota_snapshot::SelectorQuotaInput {
    codex_router_state::quota_snapshot::SelectorQuotaInput::new(
        account_id.clone(),
        account_label,
        Provider::Openai,
        codex_router_state::account::AccountStatus::Enabled,
        Some(1),
        "responses",
        vec![
            codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow::new(
                account_id.clone(),
                "responses",
                codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                status,
            )
            .with_remaining_headroom(90)
            .with_reset_unix_seconds(18_000)
            .with_effective(true)
            .with_observed_unix_seconds(900),
            codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow::new(
                account_id,
                "responses",
                codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                status,
            )
            .with_remaining_headroom(90)
            .with_reset_unix_seconds(604_800)
            .with_effective(true)
            .with_observed_unix_seconds(900),
        ],
    )
}

pub(super) fn selector_input_for_short_only_exhaustion_test(
    account_id: AccountId,
) -> codex_router_state::quota_snapshot::SelectorQuotaInput {
    codex_router_state::quota_snapshot::SelectorQuotaInput::new(
        account_id.clone(),
        "short-only-exhausted",
        Provider::Openai,
        codex_router_state::account::AccountStatus::Enabled,
        Some(1),
        "responses",
        vec![
            codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow::new(
                account_id.clone(),
                "responses",
                codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Ineligible,
            )
            .with_remaining_headroom(0)
            .with_reset_unix_seconds(1_005)
            .with_effective(true)
            .with_observed_unix_seconds(900),
            codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow::new(
                account_id,
                "responses",
                codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(80)
            .with_reset_unix_seconds(100_000)
            .with_effective(true)
            .with_observed_unix_seconds(900),
        ],
    )
}

pub(super) fn selector_input_for_claude_short_only_exhaustion_test(
    account_id: AccountId,
) -> codex_router_state::quota_snapshot::SelectorQuotaInput {
    use codex_router_core::route_profile::WindowKind;
    use codex_router_state::window_observation::WindowObservation;
    use codex_router_state::window_observation::WindowObservationProps;

    let five_hour = WindowObservation::new(
        WindowObservationProps::new(account_id.clone(), WindowKind::FiveHour, 0, 900)
            .with_reset_unix_seconds(1_005),
    )
    .unwrap_or_else(|error| panic!("Claude five-hour observation should validate: {error}"));
    let weekly = WindowObservation::new(
        WindowObservationProps::new(account_id.clone(), WindowKind::Weekly, 8_000, 900)
            .with_reset_unix_seconds(100_000),
    )
    .unwrap_or_else(|error| panic!("Claude weekly observation should validate: {error}"));

    codex_router_state::quota_snapshot::SelectorQuotaInput::new(
        account_id,
        "claude-short-only",
        Provider::Claude,
        codex_router_state::account::AccountStatus::Enabled,
        Some(1),
        "responses",
        Vec::new(),
    )
    .with_window_state(vec![five_hour, weekly], Vec::new())
}
