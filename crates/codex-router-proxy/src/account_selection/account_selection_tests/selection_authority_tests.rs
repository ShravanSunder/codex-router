use super::*;

#[test]
fn strict_quota_selection_does_not_rotate_to_weak_account_by_deficit() {
    let weak_account_id = account_id("acct_weekly_low");
    let strong_account_id = account_id("acct_weekly_healthy");
    let selector = QuotaAwareAccountSelector::new(vec![
        QuotaAwareAccountState::new(
            weak_account_id.clone(),
            23,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
        QuotaAwareAccountState::new(
            strong_account_id.clone(),
            76,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
    ]);

    for _attempt in 0..200 {
        let selected = selector
            .select_upstream_account(
                &crate::http_sse::HttpProxyRequest::new(
                    crate::routes::Method::Post,
                    "/v1/responses",
                ),
                TokenGeneration::new(1),
                None,
            )
            .unwrap_or_else(|error| panic!("selection should succeed: {error}"));

        assert_eq!(selected.account_id(), &strong_account_id);
        assert_ne!(selected.account_id(), &weak_account_id);
    }
}

#[test]
fn strict_quota_selection_keeps_strong_account_across_second_boundary() {
    let weak_account_id = account_id("acct_weekly_low");
    let strong_account_id = account_id("acct_weekly_healthy");
    let accounts = vec![
        QuotaAwareAccountState::new(
            weak_account_id.clone(),
            23,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
        QuotaAwareAccountState::new(
            strong_account_id.clone(),
            76,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
    ];
    let boundary_instants = [1_800_000_000, 1_800_000_001];
    let mut clock_reads = 0;
    let mut weighted_selector = super::WeightedDeficitSelector::default();

    let selected =
        super::select_from_account_states_with_selector(&accounts, &mut weighted_selector, || {
            // Production consumes the first instant; a hypothetical second read crosses the
            // boundary, giving the first account a weekly reset R21 prefers before quota weight.
            let instant = boundary_instants[clock_reads.min(boundary_instants.len() - 1)];
            clock_reads += 1;
            instant
        })
        .unwrap_or_else(|error| panic!("selection should succeed: {error}"));

    assert_eq!(clock_reads, 1, "one selection must read its clock once");
    assert_eq!(selected.account_id(), &strong_account_id);
    assert_ne!(selected.account_id(), &weak_account_id);
}

#[test]
fn stale_quota_authority_is_unavailable_not_all_accounts_exhausted() {
    let account_inputs = vec![
        codex_router_selection::burn_down::BurnDownAccountInput::new(
            account_id("acct_stale_authority"),
            "stale-authority",
            Provider::Openai,
            vec![
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Stale,
                )
                .with_remaining_headroom(90)
                .with_reset_unix_seconds(18_000),
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Stale,
                )
                .with_remaining_headroom(90)
                .with_reset_unix_seconds(604_800),
            ],
        )
        .with_weekly_quota_floor_basis_points(500),
        codex_router_selection::burn_down::BurnDownAccountInput::new(
            account_id("acct_fresh_exhausted"),
            "fresh-exhausted",
            Provider::Openai,
            vec![
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Ineligible,
                )
                .with_remaining_headroom(0)
                .with_reset_unix_seconds(18_000),
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Ineligible,
                )
                .with_remaining_headroom(0)
                .with_reset_unix_seconds(604_800),
            ],
        ),
    ];
    let assessment = codex_router_selection::burn_down::assess_route_band(
        codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput::new(
            codex_router_core::routes::RouteBand::Responses,
            1_001,
            RESPONSES_HTTP.clone(),
            account_inputs,
        ),
    );
    assert_eq!(
        assessment.selected_pool(),
        codex_router_selection::burn_down::SelectedPool::None
    );

    let error = super::select_from_burn_down_assessment_without_hold(
        &assessment,
        &mut super::WeightedDeficitSelector::default(),
    )
    .expect_err("stale authority must not select an account");

    assert!(matches!(
        error,
        crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::StateUnavailable,
        }
    ));
    assert!(
        super::post_exhaustion_assessment_has_safe_known_fresh_alternative(&assessment).is_err(),
        "post-exhaustion classification must not report stale authority as proven exhaustion"
    );
}

#[test]
fn request_local_attempt_ledger_excludes_previously_attempted_account() {
    let first_account_id = account_id("acct_first");
    let second_account_id = account_id("acct_second");
    let selector = QuotaAwareAccountSelector::new(vec![
        QuotaAwareAccountState::new(
            first_account_id.clone(),
            90,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
        QuotaAwareAccountState::new(
            second_account_id.clone(),
            80,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
    ]);
    let request =
        crate::http_sse::HttpProxyRequest::new(crate::routes::Method::Post, "/v1/responses")
            .with_excluded_account(first_account_id.clone());

    let selected = selector
        .select_upstream_account(&request, TokenGeneration::new(1), None)
        .unwrap_or_else(|error| panic!("selection should choose unattempted account: {error}"));

    assert_eq!(selected.account_id(), &second_account_id);
    assert_ne!(selected.account_id(), &first_account_id);
}

#[test]
fn runtime_quota_exhaustion_excludes_account_before_sqlite_catches_up() {
    let exhausted_account_id = account_id("acct_runtime_exhausted");
    let fallback_account_id = account_id("acct_runtime_fallback");
    let runtime_exhaustions = super::RouteBandRuntimeExhaustions::default();
    super::mark_runtime_quota_exhausted(
        &runtime_exhaustions,
        codex_router_core::routes::RouteBand::Responses,
        exhausted_account_id.clone(),
        1_000,
    )
    .unwrap_or_else(|error| panic!("runtime exhaustion should record: {error}"));
    let accounts = vec![
        account_input_for_runtime_exhaustion_test(exhausted_account_id.clone()),
        account_input_for_runtime_exhaustion_test(fallback_account_id.clone()),
    ];

    let filtered = super::projected_accounts_excluding_runtime_exhaustions(
        accounts,
        &runtime_exhaustions,
        "responses",
        1_001,
    )
    .unwrap_or_else(|error| panic!("runtime exhaustion filtering should succeed: {error}"));

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].account_id(), &fallback_account_id);
    assert_ne!(filtered[0].account_id(), &exhausted_account_id);
}

#[test]
fn route_band_queue_degraded_blocks_selection_until_recovered() {
    let route_band_health = super::RouteBandQueueHealth::default();

    super::mark_route_band_queue_degraded(
        &route_band_health,
        codex_router_core::routes::RouteBand::Responses,
        super::RouteBandQueueDegradedReason::DbWriteQueueFull,
        1_000,
    )
    .unwrap_or_else(|error| panic!("queue degraded state should record: {error}"));

    assert!(
        super::route_band_queue_health_allows_selection(
            &route_band_health,
            codex_router_core::routes::RouteBand::Responses,
        )
        .is_err(),
        "new route-band selections must fail closed while DB write queue health is degraded"
    );

    super::clear_route_band_queue_degraded(
        &route_band_health,
        codex_router_core::routes::RouteBand::Responses,
    )
    .unwrap_or_else(|error| panic!("queue degraded state should clear: {error}"));

    super::route_band_queue_health_allows_selection(
        &route_band_health,
        codex_router_core::routes::RouteBand::Responses,
    )
    .unwrap_or_else(|error| panic!("selection should recover after queue health clears: {error}"));
}

#[test]
fn route_band_queue_degraded_selection_emits_scrubbed_freshness_log() {
    let route_band_health = super::RouteBandQueueHealth::default();
    super::mark_route_band_queue_degraded(
        &route_band_health,
        codex_router_core::routes::RouteBand::Responses,
        super::RouteBandQueueDegradedReason::DbWriteQueueFull,
        1_000,
    )
    .unwrap_or_else(|error| panic!("queue degraded state should record: {error}"));

    let rendered_log = capture_log_output(|| {
        assert!(
            super::route_band_queue_health_allows_selection(
                &route_band_health,
                codex_router_core::routes::RouteBand::Responses,
            )
            .is_err(),
            "selection should fail closed while route-band queue health is degraded"
        );
    });

    assert!(rendered_log.contains("codex_router.snapshot_freshness_observed"));
    assert!(rendered_log.contains("queue_health"));
    assert!(rendered_log.contains("responses"));
    assert!(rendered_log.contains("unavailable"));
    assert!(rendered_log.contains("fail_closed"));
    assert!(!rendered_log.contains("raw-provider-body-canary"));
    assert!(!rendered_log.contains("sk-live-token-canary"));
    assert!(!rendered_log.contains("Authorization"));
    assert!(!rendered_log.contains("acct_raw_canary"));
    assert!(!rendered_log.contains("friendly account label"));
    assert!(!rendered_log.contains("reservation_raw_canary"));
    assert!(!rendered_log.contains("/Users/shravansunder"));
}
