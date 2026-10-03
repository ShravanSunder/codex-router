use super::*;

#[tokio::test]
async fn fresh_selectors_share_atomic_selection_lock_before_projection() {
    let repository = SlowSelectionProjectionRepository::new(account_id("acct_shared_lock"));
    let weighted_selectors = super::RouteBandWeightedSelectors::default();
    let account_holds = super::RouteBandAccountHolds::default();
    let active_reservations = super::RouteBandReservationBooks::default();
    let runtime_exhaustions = super::RouteBandRuntimeExhaustions::default();
    let route_band_queue_health = super::RouteBandQueueHealth::default();
    let selection_reservation_lock = Arc::new(tokio::sync::Mutex::new(()));
    let request =
        crate::http_sse::HttpProxyRequest::new(crate::routes::Method::Post, "/v1/responses");
    let first_selector = super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        &repository,
        super::AsyncAccountSelectorRuntimeState::new_with_selection_lock(
            Arc::clone(&weighted_selectors),
            Arc::clone(&account_holds),
            Arc::clone(&active_reservations),
            Arc::clone(&runtime_exhaustions),
            Arc::clone(&route_band_queue_health),
            Arc::clone(&selection_reservation_lock),
        ),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        Arc::new(|| 1_000),
    );
    let second_selector =
        super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
            &repository,
            super::AsyncAccountSelectorRuntimeState::new_with_selection_lock(
                Arc::clone(&weighted_selectors),
                Arc::clone(&account_holds),
                Arc::clone(&active_reservations),
                Arc::clone(&runtime_exhaustions),
                Arc::clone(&route_band_queue_health),
                Arc::clone(&selection_reservation_lock),
            ),
            super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            Arc::new(|| 1_000),
        );

    let (first_result, second_result) = tokio::join!(
        first_selector.select_upstream_account(&request, TokenGeneration::new(1), None),
        second_selector.select_upstream_account(&request, TokenGeneration::new(1), None),
    );

    first_result.unwrap_or_else(|error| panic!("first selection should succeed: {error}"));
    second_result.unwrap_or_else(|error| panic!("second selection should succeed: {error}"));
    assert_eq!(
        repository.max_concurrent_selector_reads(),
        1,
        "fresh selectors must share the same process-wide assess/snapshot/reserve lock"
    );
}

#[tokio::test]
async fn concurrent_first_session_selections_share_live_owner_before_durable_write() {
    let repository = SlowSelectionProjectionRepository::new_with_accounts(vec![
        account_id("acct_a"),
        account_id("acct_b"),
    ]);
    let weighted_selectors = super::RouteBandWeightedSelectors::default();
    let account_holds = super::RouteBandAccountHolds::default();
    let active_reservations = super::RouteBandReservationBooks::default();
    let runtime_exhaustions = super::RouteBandRuntimeExhaustions::default();
    let route_band_queue_health = super::RouteBandQueueHealth::default();
    let selection_reservation_lock = Arc::new(tokio::sync::Mutex::new(()));
    let session_affinity_cache =
        crate::session_account_affinity_cache::SessionAccountAffinityCache::shared(
            DEFAULT_SESSION_PIN_IDLE_TTL,
        );
    let write_repository = Arc::new(ControlledFailingAffinityWriteRepository::default());
    let writer = DbWriteActor::start(write_repository.clone(), 4);
    let request =
        crate::http_sse::HttpProxyRequest::new(crate::routes::Method::Post, "/v1/responses")
            .with_header(crate::headers::Header::new("session-id", "shared-session"));
    let build_selector = || {
        super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
            &repository,
            super::AsyncAccountSelectorRuntimeState::new_with_selection_lock_and_affinity_cache(
                Arc::clone(&weighted_selectors),
                Arc::clone(&account_holds),
                Arc::clone(&active_reservations),
                Arc::clone(&runtime_exhaustions),
                Arc::clone(&route_band_queue_health),
                Arc::clone(&selection_reservation_lock),
                Arc::clone(&session_affinity_cache),
            ),
            super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            Arc::new(|| 1_000),
        )
        .with_session_affinity_writer(writer.clone())
    };
    let first_selector = build_selector();
    let second_selector = build_selector();

    let (first_result, second_result) = tokio::join!(
        first_selector.select_upstream_account(&request, TokenGeneration::new(1), None),
        second_selector.select_upstream_account(&request, TokenGeneration::new(1), None),
    );
    let first =
        first_result.unwrap_or_else(|error| panic!("first selection should succeed: {error}"));
    let second =
        second_result.unwrap_or_else(|error| panic!("second selection should succeed: {error}"));

    assert_eq!(first.account_id(), second.account_id());
    assert_eq!(first.account_id().as_str(), "acct_a");
    assert_eq!(second.selection_reason(), "prompt_cache_account_affinity");
    assert!(first.session_affinity_activity_handle().is_some());
    assert!(second.session_affinity_activity_handle().is_some());
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        write_repository.entered.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("durable affinity write should reach controlled seam"));
    assert_eq!(
        write_repository.calls.load(Ordering::Acquire),
        1,
        "both real selectors must converge while the immediate durable write is blocked"
    );
    write_repository.release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        write_repository.completed.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("controlled failing write should complete"));
    assert!(write_repository.failed.load(Ordering::Acquire));

    let third = build_selector()
        .select_upstream_account(&request, TokenGeneration::new(1), None)
        .await
        .unwrap_or_else(|error| panic!("live owner should survive failed durability: {error}"));
    assert_eq!(third.account_id(), first.account_id());
    assert_eq!(third.selection_reason(), "prompt_cache_account_affinity");

    writer.shutdown().await;
}

#[tokio::test]
async fn failed_late_reservation_does_not_publish_selected_session_affinity() {
    let persisted_owner = account_id("acct_a");
    let repository = SlowSelectionProjectionRepository::new_with_blocking_affinity(
        vec![persisted_owner.clone(), account_id("acct_b")],
        codex_router_state::session_account_affinity::SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-failed-reservation",
            persisted_owner.clone(),
            900,
        ),
    );
    let active_reservations = super::RouteBandReservationBooks::default();
    let session_affinity_cache =
        crate::session_account_affinity_cache::SessionAccountAffinityCache::shared(
            DEFAULT_SESSION_PIN_IDLE_TTL,
        );
    let write_repository = Arc::new(ControlledFailingAffinityWriteRepository::default());
    let writer = DbWriteActor::start(write_repository.clone(), 4);
    writer.shutdown().await;
    let selector = super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        &repository,
        super::AsyncAccountSelectorRuntimeState::new_with_selection_lock_and_affinity_cache(
            super::RouteBandWeightedSelectors::default(),
            super::RouteBandAccountHolds::default(),
            Arc::clone(&active_reservations),
            super::RouteBandRuntimeExhaustions::default(),
            super::RouteBandQueueHealth::default(),
            Arc::new(tokio::sync::Mutex::new(())),
            Arc::clone(&session_affinity_cache),
        ),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        Arc::new(|| 1_000),
    )
    .with_session_affinity_writer(writer.clone());
    let request =
        crate::http_sse::HttpProxyRequest::new(crate::routes::Method::Post, "/v1/responses")
            .with_header(crate::headers::Header::new(
                "session-id",
                "session-failed-reservation",
            ));

    let select = selector.select_upstream_account(&request, TokenGeneration::new(1), None);
    let poison_reservations = async {
        repository.wait_for_affinity_read().await;
        let reservations_to_poison = Arc::clone(&active_reservations);
        let poison_result = std::thread::spawn(move || {
            let _guard = reservations_to_poison
                .lock()
                .unwrap_or_else(|_| panic!("reservation book should initially lock"));
            panic!("poison reservation book after the initial snapshot");
        })
        .join();
        assert!(poison_result.is_err());
        repository.release_affinity_read();
    };
    let (selection_result, ()) = tokio::join!(select, poison_reservations);

    assert!(matches!(
        selection_result,
        Err(crate::http_sse::HttpProxyError::Selection {
            reason: super::QuotaAwareAccountSelectorError::SelectorStateUnavailable,
        })
    ));
    let seeded = crate::session_account_affinity_cache::lookup_session_account_affinity(
        &session_affinity_cache,
        Provider::Openai,
        "session-failed-reservation",
        RouteBand::Responses,
        Some(&writer),
        1_000,
    )
    .unwrap_or_else(|_| panic!("lookup-only persisted seed should remain readable"))
    .unwrap_or_else(|| panic!("persisted affinity should remain seeded"));
    assert_eq!(seeded.account_id(), &persisted_owner);
    assert_eq!(write_repository.calls.load(Ordering::Acquire), 0);
    assert_eq!(
        writer.last_degraded_event(),
        None,
        "failed reservation must not attempt selected-request durability"
    );
}

#[tokio::test]
async fn different_unowned_sessions_observe_the_first_initial_admission_reservation() {
    let repository = SlowSelectionProjectionRepository::new_with_accounts(vec![
        account_id("acct_a"),
        account_id("acct_b"),
    ]);
    let weighted_selectors = super::RouteBandWeightedSelectors::default();
    let account_holds = super::RouteBandAccountHolds::default();
    let active_reservations = super::RouteBandReservationBooks::default();
    let runtime_exhaustions = super::RouteBandRuntimeExhaustions::default();
    let route_band_queue_health = super::RouteBandQueueHealth::default();
    let selection_reservation_lock = Arc::new(tokio::sync::Mutex::new(()));
    let session_affinity_cache =
        crate::session_account_affinity_cache::SessionAccountAffinityCache::shared(
            DEFAULT_SESSION_PIN_IDLE_TTL,
        );
    let build_selector = || {
        super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
            &repository,
            super::AsyncAccountSelectorRuntimeState::new_with_selection_lock_and_affinity_cache(
                Arc::clone(&weighted_selectors),
                Arc::clone(&account_holds),
                Arc::clone(&active_reservations),
                Arc::clone(&runtime_exhaustions),
                Arc::clone(&route_band_queue_health),
                Arc::clone(&selection_reservation_lock),
                Arc::clone(&session_affinity_cache),
            ),
            super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            Arc::new(|| 1_000),
        )
    };
    let first_request =
        crate::http_sse::HttpProxyRequest::new(crate::routes::Method::Post, "/v1/responses")
            .with_header(crate::headers::Header::new("session-id", "session-one"));
    let second_request =
        crate::http_sse::HttpProxyRequest::new(crate::routes::Method::Post, "/v1/responses")
            .with_header(crate::headers::Header::new("session-id", "session-two"));

    let first = build_selector()
        .select_upstream_account(&first_request, TokenGeneration::new(1), None)
        .await
        .unwrap_or_else(|error| panic!("first initial admission should select: {error}"));
    let second = build_selector()
        .select_upstream_account(&second_request, TokenGeneration::new(1), None)
        .await
        .unwrap_or_else(|error| panic!("second initial admission should select: {error}"));

    assert_eq!(first.account_id().as_str(), "acct_a");
    assert_eq!(
        first.selection_reason(),
        "preferred_near_reset_initial_admission"
    );
    assert_eq!(second.account_id().as_str(), "acct_b");
    assert_eq!(
        second.selection_reason(),
        "preferred_near_reset_initial_admission"
    );
    assert_ne!(second.selection_reason(), "prompt_cache_account_affinity");
}
