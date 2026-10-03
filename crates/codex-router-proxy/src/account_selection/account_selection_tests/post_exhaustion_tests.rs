use super::*;

fn post_exhaustion_input<'a, TRepository>(
    state_repository: &'a TRepository,
    route_band: RouteBand,
    route_profile: codex_router_core::route_profile::RouteProfile,
    excluded_account_id: &'a AccountId,
    now_unix_seconds: u64,
) -> super::RouteBandPostExhaustionOutcomeInput<'a, TRepository> {
    super::RouteBandPostExhaustionOutcomeInput {
        state_repository,
        active_reservations: None,
        runtime_exhaustions: None,
        route_band_queue_health: None,
        route_band,
        route_profile,
        excluded_account_id,
        now_unix_seconds,
    }
}

#[tokio::test]
async fn post_exhaustion_alternative_selection_fails_closed_when_queue_health_degraded() {
    let route_band_health = super::RouteBandQueueHealth::default();
    super::mark_route_band_queue_degraded(
        &route_band_health,
        codex_router_core::routes::RouteBand::Responses,
        super::RouteBandQueueDegradedReason::DbWriteQueueFull,
        1_000,
    )
    .unwrap_or_else(|error| panic!("queue degraded state should record: {error}"));

    let exhausted_account_id = account_id("acct_exhausted");
    let mut input = post_exhaustion_input(
        &PanicSelectionProjectionRepository,
        RouteBand::Responses,
        RESPONSES_HTTP.clone(),
        &exhausted_account_id,
        1_001,
    );
    input.route_band_queue_health = Some(&route_band_health);
    let result = super::route_band_post_exhaustion_outcome(input).await;

    assert!(
        result.is_err(),
        "post-exhaustion alternative selection must fail closed before projection reads when route-band queue health is degraded"
    );
}

#[tokio::test]
async fn post_exhaustion_alternative_selection_fails_closed_for_unknown_only_alternative() {
    let exhausted_account_id = account_id("acct_exhausted_unknown");
    let unknown_account_id = account_id("acct_unknown_alternative");
    let repository = StaticSelectionProjectionRepository::new(vec![
        selector_input_for_runtime_exhaustion_test(exhausted_account_id.clone()),
        selector_input_for_post_exhaustion_test(
            unknown_account_id,
            "unknown-alternative",
            codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Unknown,
        ),
    ]);

    let result = super::route_band_post_exhaustion_outcome(post_exhaustion_input(
        &repository,
        RouteBand::Responses,
        RESPONSES_HTTP.clone(),
        &exhausted_account_id,
        1_001,
    ))
    .await;

    assert!(
        result.is_err(),
        "post-exhaustion reconnect must fail closed when the only alternative has unknown quota evidence"
    );
}

#[tokio::test]
async fn post_exhaustion_alternative_selection_fails_closed_for_stale_only_alternative() {
    let exhausted_account_id = account_id("acct_exhausted_stale");
    let stale_account_id = account_id("acct_stale_alternative");
    let repository = StaticSelectionProjectionRepository::new(vec![
        selector_input_for_runtime_exhaustion_test(exhausted_account_id.clone()),
        selector_input_for_post_exhaustion_test(
            stale_account_id,
            "stale-alternative",
            codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Stale,
        ),
    ]);

    let result = super::route_band_post_exhaustion_outcome(post_exhaustion_input(
        &repository,
        RouteBand::Responses,
        RESPONSES_HTTP.clone(),
        &exhausted_account_id,
        1_001,
    ))
    .await;

    assert!(
        result.is_err(),
        "post-exhaustion reconnect must fail closed when the only alternative has stale quota evidence"
    );
}

#[tokio::test]
async fn post_exhaustion_outcome_waits_only_for_fresh_short_quota_exhaustion() {
    let exhausted_account_id = account_id("acct_exhausted_short_wait");
    let short_exhausted_account_id = account_id("acct_short_wait");
    let repository = StaticSelectionProjectionRepository::new(vec![
        selector_input_for_runtime_exhaustion_test(exhausted_account_id.clone()),
        selector_input_for_short_only_exhaustion_test(short_exhausted_account_id),
    ]);

    let result = super::route_band_post_exhaustion_outcome(post_exhaustion_input(
        &repository,
        RouteBand::Responses,
        RESPONSES_HTTP.clone(),
        &exhausted_account_id,
        1_000,
    ))
    .await;

    assert!(
        matches!(
            result,
            Ok(super::PostExhaustionRouteBandOutcome::ShortQuotaWait {
                retry_after_seconds: 65..=125,
            })
        ),
        "5h retry should target reset plus one-to-two-minute jitter: {result:?}"
    );
}

#[tokio::test]
async fn post_exhaustion_outcome_waits_for_selected_single_account_with_healthy_weekly_window() {
    let exhausted_account_id = account_id("acct_single_short_wait");
    let repository =
        StaticSelectionProjectionRepository::new(vec![selector_input_for_runtime_exhaustion_test(
            exhausted_account_id.clone(),
        )]);

    let result = super::route_band_post_exhaustion_outcome(post_exhaustion_input(
        &repository,
        RouteBand::Responses,
        RESPONSES_HTTP.clone(),
        &exhausted_account_id,
        1_000,
    ))
    .await;

    assert!(matches!(
        result,
        Ok(super::PostExhaustionRouteBandOutcome::ShortQuotaWait {
            retry_after_seconds: 17_060..=17_120,
        })
    ));
}

#[tokio::test]
async fn post_exhaustion_short_wait_ignores_other_provider_windows() {
    let exhausted_account_id = account_id("acct_openai_exhausted_without_wait");
    let claude_account_id = account_id("acct_claude_short_wait_must_not_leak");
    let repository = StaticSelectionProjectionRepository::new(vec![
        selector_input_for_post_exhaustion_test(
            exhausted_account_id.clone(),
            "openai-exhausted",
            codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Ineligible,
        ),
        selector_input_for_claude_short_only_exhaustion_test(claude_account_id),
    ]);

    let result = super::route_band_post_exhaustion_outcome(post_exhaustion_input(
        &repository,
        RouteBand::Responses,
        RESPONSES_HTTP.clone(),
        &exhausted_account_id,
        1_000,
    ))
    .await;

    assert_eq!(
        result,
        Ok(super::PostExhaustionRouteBandOutcome::NoSelectableAlternative)
    );
}

#[tokio::test]
async fn post_exhaustion_outcome_stops_for_selected_account_with_exhausted_weekly_window() {
    let exhausted_account_id = account_id("acct_single_weekly_exhausted");
    let repository =
        StaticSelectionProjectionRepository::new(vec![selector_input_for_post_exhaustion_test(
            exhausted_account_id.clone(),
            "weekly-exhausted",
            codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Ineligible,
        )]);

    let result = super::route_band_post_exhaustion_outcome(post_exhaustion_input(
        &repository,
        RouteBand::Responses,
        RESPONSES_HTTP.clone(),
        &exhausted_account_id,
        1_000,
    ))
    .await;

    assert_eq!(
        result,
        Ok(super::PostExhaustionRouteBandOutcome::NoSelectableAlternative)
    );
}
