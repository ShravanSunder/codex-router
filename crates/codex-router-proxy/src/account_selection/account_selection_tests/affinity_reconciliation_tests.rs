use super::*;

#[tokio::test]
async fn live_activity_during_persisted_affinity_await_wins_reconciliation() {
    let persisted_owner = account_id("acct_b");
    let repository = SlowSelectionProjectionRepository::new_with_blocking_affinity(
        vec![account_id("acct_a"), persisted_owner.clone()],
        codex_router_state::session_account_affinity::SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-post-await",
            persisted_owner,
            7_900,
        ),
    );
    let session_affinity_cache =
        crate::session_account_affinity_cache::SessionAccountAffinityCache::shared(
            DEFAULT_SESSION_PIN_IDLE_TTL,
        );
    let original_live_owner =
        crate::session_account_affinity_cache::publish_session_account_affinity(
            &session_affinity_cache,
            Provider::Openai,
            "session-post-await",
            &account_id("acct_a"),
            RouteBand::Responses,
            None,
            0,
        )
        .unwrap_or_else(|_| panic!("expired live owner fixture should publish"));
    let selector = super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        &repository,
        super::AsyncAccountSelectorRuntimeState::new_with_selection_lock_and_affinity_cache(
            super::RouteBandWeightedSelectors::default(),
            super::RouteBandAccountHolds::default(),
            super::RouteBandReservationBooks::default(),
            super::RouteBandRuntimeExhaustions::default(),
            super::RouteBandQueueHealth::default(),
            Arc::new(tokio::sync::Mutex::new(())),
            Arc::clone(&session_affinity_cache),
        ),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        Arc::new(|| 8_000),
    );
    let request =
        crate::http_sse::HttpProxyRequest::new(crate::routes::Method::Post, "/v1/responses")
            .with_header(crate::headers::Header::new(
                "session-id",
                "session-post-await",
            ));

    let select = selector.select_upstream_account(&request, TokenGeneration::new(1), None);
    let renew_while_awaiting = async {
        repository.wait_for_affinity_read().await;
        assert!(
            original_live_owner
                .activity_handle()
                .touch_if_current(8_000)
                .unwrap_or_else(|_| panic!("live activity touch should succeed"))
        );
        repository.release_affinity_read();
    };
    let (selected, ()) = tokio::join!(select, renew_while_awaiting);
    let selected =
        selected.unwrap_or_else(|error| panic!("post-await reconciliation should select: {error}"));

    assert_eq!(selected.account_id().as_str(), "acct_a");
    assert_eq!(selected.selection_reason(), "prompt_cache_account_affinity");
}

#[test]
fn account_hold_cooldown_does_not_keep_materially_weaker_account() {
    let weak_account_id = account_id("acct_weekly_low");
    let strong_account_id = account_id("acct_weekly_healthy");
    let account_inputs = vec![
        codex_router_selection::burn_down::BurnDownAccountInput::new(
            weak_account_id.clone(),
            "weak",
            Provider::Openai,
            vec![
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(98)
                .with_reset_unix_seconds(18_000),
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(23)
                .with_reset_unix_seconds(604_800),
            ],
        )
        .with_current_active_sessions(1),
        codex_router_selection::burn_down::BurnDownAccountInput::new(
            strong_account_id.clone(),
            "strong",
            Provider::Openai,
            vec![
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(76)
                .with_reset_unix_seconds(18_000),
                codex_router_selection::burn_down::QuotaWindowFact::new(
                    codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS,
                    codex_router_selection::burn_down::QuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(76)
                .with_reset_unix_seconds(604_800),
            ],
        )
        .with_current_active_sessions(1),
    ];
    let assessment = codex_router_selection::burn_down::assess_route_band(
        codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput::new(
            codex_router_core::routes::RouteBand::Responses,
            10_000,
            RESPONSES_HTTP.clone(),
            account_inputs,
        ),
    );
    let mut holds = HashMap::from([(
        super::ProviderRouteBand::new(Provider::Openai, RouteBand::Responses),
        super::AccountHold::new(weak_account_id.clone(), 9_990),
    )]);
    let mut weighted_selector =
        codex_router_selection::weighted_deficit::WeightedDeficitSelector::default();

    let selected = super::select_from_burn_down_assessment(
        "responses",
        Provider::Openai,
        &assessment,
        &mut weighted_selector,
        &mut holds,
        120,
        10_000,
    )
    .unwrap_or_else(|error| panic!("selection should succeed: {error}"));

    assert_eq!(selected.account_id(), &strong_account_id);
    assert_ne!(selected.account_id(), &weak_account_id);
    assert_eq!(
        holds
            .get(&super::ProviderRouteBand::new(
                Provider::Openai,
                RouteBand::Responses,
            ))
            .map(|hold| hold.account_id.as_str()),
        Some(strong_account_id.as_str())
    );
}
