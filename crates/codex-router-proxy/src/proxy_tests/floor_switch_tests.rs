use super::*;

#[tokio::test]
async fn weekly_floor_switch_preserves_hard_affinity_until_configured_stop() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_weekly_floor");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path).expect("state store should open");
    let eligible = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_floor_eligible"),
        "eligible",
        AccountStatus::Enabled,
    );
    let protected = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_floor_protected"),
        "protected",
        AccountStatus::Enabled,
    );
    persist_fresh_account_with_selector_window_specs(
        &state,
        &eligible,
        "responses",
        &[(18_000, 80, true), (604_800, 8, false)],
    );
    persist_fresh_account_with_selector_window_specs(
        &state,
        &protected,
        "responses",
        &[(18_000, 100, true), (604_800, 8, false)],
    );
    let affinity_secret = test_affinity_secret();
    persist_previous_response_owner(
        &state,
        "resp_protected",
        &affinity_secret,
        protected.account_id(),
    )
    .expect("protected affinity owner should persist");
    drop(state);

    let async_state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("async state should open");
    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect("mutation store should open");
    let floor = WeeklyQuotaFloorBasisPoints::new(500).expect("valid floor");
    mutation
        .set_weekly_quota_floor_by_label(protected.label(), Some(floor))
        .await
        .expect("protected floor should commit");

    let selector = AsyncRepositoryBackedAccountSelector::new(&async_state);
    let ordinary_request = HttpProxyRequest::new(Method::Post, "/v1/responses");
    let affinity_request = HttpProxyRequest::new(Method::Post, "/v1/responses")
        .with_body(br#"{"previous_response_id":"resp_protected"}"#.to_vec());
    let selected = selector
        .select_upstream_account(
            &ordinary_request,
            TokenGeneration::new(1),
            Some(&affinity_secret),
        )
        .await
        .expect("eligible peer should be selected");
    assert_eq!(selected.account_id(), eligible.account_id());
    assert_ne!(selected.selection_reason(), "previous_response_affinity");
    let continued = selector
        .select_upstream_account(
            &affinity_request,
            TokenGeneration::new(1),
            Some(&affinity_secret),
        )
        .await
        .expect("hard continuation should retain its owner above the configured floor");
    assert_eq!(continued.account_id(), protected.account_id());
    assert_eq!(continued.selection_reason(), "previous_response_affinity");

    mutation
        .set_weekly_quota_floor_by_label(eligible.label(), Some(floor))
        .await
        .expect("eligible floor should commit");
    let fallback = selector
        .select_upstream_account(
            &ordinary_request,
            TokenGeneration::new(1),
            Some(&affinity_secret),
        )
        .await
        .expect("two switch-band accounts should retain eligible fallback");
    assert!(
        fallback.account_id() == eligible.account_id()
            || fallback.account_id() == protected.account_id()
    );
    let state = SqliteStateStore::open(&database_path).expect("fresh state should reopen");
    for account in [&eligible, &protected] {
        persist_fresh_account_with_selector_window_specs(
            &state,
            account,
            "responses",
            &[(18_000, 100, true), (604_800, 5, false)],
        );
    }
    drop(state);
    assert_eq!(
        selector
            .select_upstream_account(
                &affinity_request,
                TokenGeneration::new(1),
                Some(&affinity_secret),
            )
            .await,
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable
        })
    );
    assert_eq!(
        selector
            .select_upstream_account(
                &ordinary_request,
                TokenGeneration::new(1),
                Some(&affinity_secret),
            )
            .await,
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
        })
    );
    let state = SqliteStateStore::open(&database_path).expect("fresh state should reopen");
    persist_fresh_account_with_selector_window_specs(
        &state,
        &protected,
        "responses",
        &[(18_000, 100, true), (604_800, 9, false)],
    );
    drop(state);

    let continued = selector
        .select_upstream_account(
            &affinity_request,
            TokenGeneration::new(1),
            Some(&affinity_secret),
        )
        .await
        .expect("above the configured floor should allow affinity continuation");
    assert_eq!(continued.account_id(), protected.account_id());
    assert_eq!(continued.selection_reason(), "previous_response_affinity");
    let serialized_state = AsyncSqliteStateStore::open_read_only(&database_path)
        .await
        .expect("serialized selector state should reopen read-only");
    let serialized_selector = AsyncRepositoryBackedAccountSelector::new(&serialized_state);
    let ordinary_after_refresh = serialized_selector
        .select_upstream_account(
            &ordinary_request,
            TokenGeneration::new(1),
            Some(&affinity_secret),
        )
        .await
        .expect("above the switch point should allow new selection");
    assert_eq!(ordinary_after_refresh.account_id(), protected.account_id());
    serialized_state
        .close()
        .await
        .expect("serialized state should close");
    mutation.close().await;
}

#[tokio::test]
async fn live_floor_switch_peer_assessment_is_read_only_and_respects_runtime_quarantine() {
    let temp_dir = ProxyTestTempDir::new("live-floor-switch-peer");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path).expect("state store should open");
    let protected = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_floor_source"),
        "source",
        AccountStatus::Enabled,
    );
    let peer = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_floor_peer"),
        "peer",
        AccountStatus::Enabled,
    );
    for (account, remaining) in [(&protected, 8), (&peer, 80)] {
        persist_fresh_account_with_selector_window_specs(
            &state,
            account,
            "responses",
            &[(18_000, 100, true), (604_800, remaining, false)],
        );
    }
    drop(state);
    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect("floor mutation store should open");
    mutation
        .set_weekly_quota_floor_by_label(
            protected.label(),
            Some(WeeklyQuotaFloorBasisPoints::new(500).expect("valid floor")),
        )
        .await
        .expect("floor should commit");
    let state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("async state should open");
    let reservations = RouteBandReservationBooks::default();
    let weighted_selectors = RouteBandWeightedSelectors::default();
    let account_holds = RouteBandAccountHolds::default();
    let runtime_exhaustions = RouteBandRuntimeExhaustions::default();
    let queue_health = RouteBandQueueHealth::default();
    let runtime_state = AsyncAccountSelectorRuntimeState::new(
        Arc::clone(&weighted_selectors),
        Arc::clone(&account_holds),
        Arc::clone(&reservations),
        Arc::clone(&runtime_exhaustions),
        Arc::clone(&queue_health),
    );
    let assessor = RuntimeAccountAdmissionAssessor::new(
        state.clone(),
        &runtime_state,
        Arc::new(test_unix_seconds),
    );
    assert_eq!(
        assessor
            .assess_peer(protected.account_id(), RouteBand::Responses)
            .await,
        FloorSwitchPeerAssessment::SelectablePeer
    );
    assert!(
        reservations
            .lock()
            .expect("reservations should be readable")
            .is_empty()
    );
    assert!(
        weighted_selectors
            .lock()
            .expect("weighted state should be readable")
            .is_empty()
    );
    assert!(
        account_holds
            .lock()
            .expect("hold state should be readable")
            .is_empty()
    );
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        &state,
        runtime_state,
        120,
        Arc::new(test_unix_seconds),
    );
    let selected = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        )
        .await
        .expect("actual selection should use assessed peer");
    assert_eq!(selected.account_id(), peer.account_id());
    drop(selected);

    mark_runtime_quota_exhausted(
        &runtime_exhaustions,
        RouteBand::Responses,
        peer.account_id().clone(),
        test_unix_seconds(),
    )
    .expect("runtime quarantine should record");
    assert_eq!(
        assessor
            .assess_peer(protected.account_id(), RouteBand::Responses)
            .await,
        FloorSwitchPeerAssessment::NoPeer
    );
    let fallback = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        )
        .await
        .expect("runtime-quarantined peer should leave above-floor fallback");
    assert_eq!(fallback.account_id(), protected.account_id());
    drop(fallback);
    mark_route_band_queue_degraded(
        &queue_health,
        RouteBand::Responses,
        RouteBandQueueDegradedReason::DbWriteFailed,
        test_unix_seconds(),
    )
    .expect("queue failure should record");
    assert_eq!(
        assessor
            .assess_peer(protected.account_id(), RouteBand::Responses)
            .await,
        FloorSwitchPeerAssessment::AuthorityUnavailable
    );
    assert_eq!(
        selector
            .select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses"),
                TokenGeneration::new(1),
                None,
            )
            .await,
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::StateUnavailable
        })
    );
    mutation.close().await;
}
