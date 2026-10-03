use super::*;

#[tokio::test]
async fn claude_admission_reads_its_session_header_and_keeps_the_active_pin() {
    let preferred_account = account_id("acct_claude_preferred");
    let pinned_account = account_id("acct_claude_pinned");
    let mut repository = SlowSelectionProjectionRepository::new_with_accounts(vec![
        preferred_account,
        pinned_account.clone(),
    ]);
    repository.provider = Provider::Claude;
    repository.short_headroom.insert(pinned_account.clone(), 80);
    repository
        .persisted_affinities
        .lock()
        .expect("test pin lock")
        .push(
            codex_router_state::session_account_affinity::SessionAccountAffinity::with_pin_state(
                Provider::Claude,
                "claude-session",
                Some(pinned_account.clone()),
                7,
                1_000,
            ),
        );
    let selector = super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        &repository,
        super::AsyncAccountSelectorRuntimeState::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        ),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        Arc::new(|| 1_001),
    );
    let request = crate::http_sse::HttpProxyRequest::new(
        crate::routes::Method::Post,
        "/anthropic/v1/messages",
    )
    .with_header(crate::headers::Header::new("session-id", "codex-session"))
    .with_header(crate::headers::Header::new(
        "x-claude-code-session-id",
        "claude-session",
    ));

    let selected = selector
        .select_upstream_account(&request, TokenGeneration::new(1), None)
        .await
        .expect("Claude selection should succeed");

    assert_eq!(selected.account_id(), &pinned_account);
    assert_eq!(selected.selection_reason(), "prompt_cache_account_affinity");
    assert_eq!(
        selected.pin_observation(),
        Some(&super::PinObservation::new(Some(pinned_account), 7))
    );
    assert!(selected.session_affinity_activity_handle().is_none());
}

#[tokio::test]
async fn claude_admission_releases_reserve_pin_before_selecting_a_preferred_account() {
    let preferred_account = account_id("acct_claude_preferred");
    let pinned_account = account_id("acct_claude_reserve");
    let mut repository = SlowSelectionProjectionRepository::new_with_accounts(vec![
        preferred_account.clone(),
        pinned_account.clone(),
    ]);
    repository.provider = Provider::Claude;
    repository.short_headroom.insert(pinned_account.clone(), 5);
    repository
        .persisted_affinities
        .lock()
        .expect("test pin lock")
        .push(
            codex_router_state::session_account_affinity::SessionAccountAffinity::with_pin_state(
                Provider::Claude,
                "claude-session",
                Some(pinned_account),
                7,
                1_000,
            ),
        );
    let selector = super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        &repository,
        super::AsyncAccountSelectorRuntimeState::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        ),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        Arc::new(|| 1_001),
    );
    let request = crate::http_sse::HttpProxyRequest::new(
        crate::routes::Method::Post,
        "/anthropic/v1/messages",
    )
    .with_header(crate::headers::Header::new(
        "x-claude-code-session-id",
        "claude-session",
    ));

    let selected = selector
        .select_upstream_account(&request, TokenGeneration::new(1), None)
        .await
        .expect("Claude selection should succeed");

    assert_eq!(selected.account_id(), &preferred_account);
    assert_eq!(
        selected.pin_observation(),
        Some(&super::PinObservation::new(None, 8))
    );
    assert!(selected.session_affinity_activity_handle().is_none());
    let persisted = repository
        .persisted_affinities
        .lock()
        .expect("test pin lock");
    assert_eq!(persisted[0].account_id(), None);
    assert_eq!(persisted[0].pin_version(), 8);
}

#[test]
fn session_header_extraction_is_profile_specific_and_codex_is_unchanged() {
    let request =
        crate::http_sse::HttpProxyRequest::new(crate::routes::Method::Post, "/v1/responses")
            .with_header(crate::headers::Header::new("session-id", "codex-session"))
            .with_header(crate::headers::Header::new(
                "x-claude-code-session-id",
                "claude-session",
            ));
    for route_kind in [RouteKind::Responses, RouteKind::ResponsesWebSocket] {
        assert_eq!(
            super::session_id_for_route(&request, route_kind),
            Some("codex-session")
        );
    }
    assert_eq!(
        super::session_id_for_route(&request, RouteKind::ClaudeMessages),
        Some("claude-session")
    );
    assert_eq!(
        super::session_id_for_route(&request, RouteKind::Models),
        None
    );
    let empty_claude_session = crate::http_sse::HttpProxyRequest::new(
        crate::routes::Method::Post,
        "/anthropic/v1/messages",
    )
    .with_header(crate::headers::Header::new("session-id", "codex-session"))
    .with_header(crate::headers::Header::new("x-claude-code-session-id", ""));
    assert_eq!(
        super::session_id_for_route(&empty_claude_session, RouteKind::ClaudeMessages),
        None
    );
}

#[tokio::test]
async fn claude_admission_preserves_inactive_versions_and_keeps_a_single_reserve_pin() {
    let account = account_id("acct_claude_only");
    for (stored_pin, expected) in [
        (None, super::PinObservation::new(None, 0)),
        (
            Some((Some(account.clone()), 7, 990)),
            super::PinObservation::new(None, 7),
        ),
        (Some((None, 8, 1_000)), super::PinObservation::new(None, 8)),
        (
            Some((Some(account.clone()), 9, 1_000)),
            super::PinObservation::new(Some(account.clone()), 9),
        ),
    ] {
        let mut repository = SlowSelectionProjectionRepository::new(account.clone());
        repository.provider = Provider::Claude;
        repository.short_headroom.insert(account.clone(), 5);
        if let Some((owner, version, last_seen)) = stored_pin {
            repository.persisted_affinities.lock().unwrap_or_else(|_| panic!("test pin lock")).push(
                    codex_router_state::session_account_affinity::SessionAccountAffinity::with_pin_state(
                        Provider::Claude, "claude-session", owner, version, last_seen,
                    ),
                );
        }
        let runtime_state =
            super::AsyncAccountSelectorRuntimeState::new_with_selection_lock_and_affinity_cache(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                super::SessionAccountAffinityCache::shared(std::time::Duration::from_secs(10)),
            );
        let selector = super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
            &repository,
            runtime_state,
            super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            Arc::new(|| 1_001),
        );
        let request = crate::http_sse::HttpProxyRequest::new(
            crate::routes::Method::Post,
            "/anthropic/v1/messages",
        )
        .with_header(crate::headers::Header::new(
            "x-claude-code-session-id",
            "claude-session",
        ));
        let selected = selector
            .select_upstream_account(&request, TokenGeneration::new(1), None)
            .await
            .unwrap_or_else(|error| panic!("single Reserve account should select: {error}"));
        assert_eq!(selected.account_id(), &account);
        assert_eq!(selected.pin_observation(), Some(&expected));
        assert!(selected.session_affinity_activity_handle().is_none());
    }
}

#[tokio::test]
async fn claude_admission_releases_an_ineligible_pin_even_without_a_selectable_pool() {
    let mut repository = SlowSelectionProjectionRepository::new_with_accounts(Vec::new());
    repository.provider = Provider::Claude;
    repository
        .persisted_affinities
        .lock()
        .unwrap_or_else(|_| panic!("test pin lock"))
        .push(
            codex_router_state::session_account_affinity::SessionAccountAffinity::with_pin_state(
                Provider::Claude,
                "claude-session",
                Some(account_id("acct_ineligible")),
                7,
                1_000,
            ),
        );
    let selector = super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        &repository,
        super::AsyncAccountSelectorRuntimeState::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        ),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        Arc::new(|| 1_001),
    );
    let request = crate::http_sse::HttpProxyRequest::new(
        crate::routes::Method::Post,
        "/anthropic/v1/messages",
    )
    .with_header(crate::headers::Header::new(
        "x-claude-code-session-id",
        "claude-session",
    ));
    assert!(
        selector
            .select_upstream_account(&request, TokenGeneration::new(1), None)
            .await
            .is_err()
    );
    let persisted = repository
        .persisted_affinities
        .lock()
        .unwrap_or_else(|_| panic!("test pin lock"));
    assert_eq!(persisted.first().and_then(|pin| pin.account_id()), None);
    assert_eq!(persisted.first().map(|pin| pin.pin_version()), Some(8));
}

#[tokio::test]
async fn claude_admission_single_contention_releases_the_fresh_reserve_pin() {
    assert_claude_admission_contention(false, 80).await;
}

#[tokio::test]
async fn claude_admission_double_contention_selects_afresh_with_the_latest_observation() {
    for latest_short_headroom in [5, 80] {
        assert_claude_admission_contention(true, latest_short_headroom).await;
    }
}

async fn assert_claude_admission_contention(double_contention: bool, latest_short_headroom: u32) {
    use codex_router_state::session_account_affinity::SessionAccountAffinity;
    let first_reserve = account_id("acct_first_reserve");
    let second_reserve = account_id("acct_second_reserve");
    let latest_preferred = account_id("acct_latest_preferred");
    let best_preferred = account_id("acct_best_preferred");
    let mut repository = SlowSelectionProjectionRepository::new_with_accounts(vec![
        first_reserve.clone(),
        second_reserve.clone(),
        latest_preferred.clone(),
        best_preferred.clone(),
    ]);
    repository.provider = Provider::Claude;
    repository.short_headroom.insert(first_reserve.clone(), 5);
    repository.short_headroom.insert(second_reserve.clone(), 5);
    repository
        .short_headroom
        .insert(latest_preferred.clone(), latest_short_headroom);
    repository
        .persisted_affinities
        .lock()
        .unwrap_or_else(|_| panic!("test pin lock"))
        .push(SessionAccountAffinity::with_pin_state(
            Provider::Claude,
            "session",
            Some(first_reserve),
            7,
            1_000,
        ));
    {
        let mut contention = repository
            .release_contention_pins
            .lock()
            .unwrap_or_else(|_| panic!("test contention lock"));
        contention.push_back(SessionAccountAffinity::with_pin_state(
            Provider::Claude,
            "session",
            Some(second_reserve),
            9,
            1_000,
        ));
        if double_contention {
            contention.push_back(SessionAccountAffinity::with_pin_state(
                Provider::Claude,
                "session",
                Some(latest_preferred.clone()),
                11,
                1_000,
            ));
        }
    }
    let selector = super::AsyncRepositoryBackedAccountSelector::new_with_runtime_dependencies(
        &repository,
        super::AsyncAccountSelectorRuntimeState::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        ),
        super::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
        Arc::new(|| 1_001),
    );
    let request = crate::http_sse::HttpProxyRequest::new(
        crate::routes::Method::Post,
        "/anthropic/v1/messages",
    )
    .with_header(crate::headers::Header::new(
        "x-claude-code-session-id",
        "session",
    ));
    let selected = selector
        .select_upstream_account(&request, TokenGeneration::new(1), None)
        .await
        .unwrap_or_else(|error| panic!("contention selection: {error}"));
    assert_eq!(
        repository
            .release_compare_and_set_count
            .load(Ordering::Relaxed),
        2
    );
    assert_eq!(selected.account_id(), &best_preferred);
    let expected = if double_contention {
        super::PinObservation::new(Some(latest_preferred), 11)
    } else {
        super::PinObservation::new(None, 10)
    };
    assert_eq!(selected.pin_observation(), Some(&expected));
    assert_ne!(selected.selection_reason(), "prompt_cache_account_affinity");
}
