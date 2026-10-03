use super::*;

#[test]
fn repository_backed_selector_skips_ineligible_account_for_next_normal_request() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_next_normal");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let exhausted = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_exhausted"),
        "exhausted",
        AccountStatus::Enabled,
    );
    let eligible = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_eligible"),
        "eligible",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &exhausted,
        "responses",
        &[(18_000, 0, true), (604_800, 0, false)],
    );
    persist_fresh_account_with_selector_window_specs(
        &state,
        &eligible,
        "responses",
        &[(18_000, 42, true), (604_800, 42, false)],
    );

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("next normal request should select eligible account: {error}"),
    };

    assert_eq!(selected.account_id(), eligible.account_id());
    assert_eq!(selected.selection_reason(), "preferred_idle_far_reset");
}

#[test]
fn repository_backed_selector_uses_unknown_fallback_when_all_accounts_need_probe() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_all_unknown_fallback");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let unknown = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_unknown"),
        "unknown",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_status_specs(
        &state,
        &unknown,
        "responses",
        &[
            (18_000, 100, true, SelectorQuotaWindowStatus::Unknown),
            (604_800, 100, false, SelectorQuotaWindowStatus::Unknown),
        ],
    );

    let selector = RepositoryBackedAccountSelector::new(&state);

    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses"),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("unknown fallback account should be selected: {error}"),
    };

    assert_eq!(selected.account_id(), unknown.account_id());
    assert_eq!(selected.selection_reason(), "unknown_fallback_preferred");
}

#[test]
fn repository_backed_selector_affinity_owner_bypasses_hold_and_weighted_choice() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_owner_hit");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let alpha = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_alpha"),
        "alpha",
        AccountStatus::Enabled,
    );
    let beta = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_beta"),
        "beta",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &beta,
        "responses",
        &[(18_000, 50, true), (604_800, 50, false)],
    );
    let affinity_secret = test_affinity_secret();
    if let Err(error) =
        persist_previous_response_owner(&state, "resp_beta", &affinity_secret, beta.account_id())
    {
        panic!("affinity owner should persist: {error}");
    }

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_body(br#"{"previous_response_id":"resp_beta"}"#.to_vec()),
        TokenGeneration::new(1),
        Some(&affinity_secret),
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("affinity owner should select: {error}"),
    };

    assert_eq!(selected.account_id(), beta.account_id());
    assert_eq!(selected.selection_reason(), "previous_response_affinity");
}

#[tokio::test]
async fn async_repository_backed_selector_preserves_healthy_previous_response_affinity() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_healthy_affinity_owner");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let alpha = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_alpha"),
        "alpha",
        AccountStatus::Enabled,
    );
    let beta = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_beta"),
        "beta",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &beta,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    let affinity_secret = test_affinity_secret();
    if let Err(error) =
        persist_previous_response_owner(&state, "resp_beta", &affinity_secret, beta.account_id())
    {
        panic!("affinity owner should persist: {error}");
    }
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };

    let selector = AsyncRepositoryBackedAccountSelector::new(&async_state);
    let selected = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_body(br#"{"previous_response_id":"resp_beta"}"#.to_vec()),
            TokenGeneration::new(1),
            Some(&affinity_secret),
        )
        .await
        .unwrap_or_else(|error| panic!("healthy previous-response owner should select: {error}"));

    assert_eq!(selected.account_id(), beta.account_id());
    assert_eq!(selected.selection_reason(), "previous_response_affinity");
    assert!(
        !selected.credit_backed_at_selection(),
        "healthy included affinity should remain ordinary quota admission"
    );
}

#[tokio::test]
async fn async_repository_backed_selector_preserves_credit_backed_previous_response_affinity() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_affinity_owner_hit");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let alpha = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_alpha"),
        "alpha",
        AccountStatus::Enabled,
    );
    let beta = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_beta"),
        "beta",
        AccountStatus::Enabled,
    );
    let secrets = codex_router_secret_store::test_support::open_encrypted_credential_store(
        temp_dir.path().join("secrets"),
    )
    .expect("affinity credential store should open");
    let now_unix_seconds = test_unix_seconds();
    persist_credit_backed_account_with_token_async(
        &database_path,
        &secrets,
        &alpha,
        "alpha-credit-token",
        now_unix_seconds,
        true,
    )
    .await;
    persist_credit_backed_account_with_token_async(
        &database_path,
        &secrets,
        &beta,
        "beta-credit-token",
        now_unix_seconds,
        true,
    )
    .await;
    let affinity_secret = test_affinity_secret();
    if let Err(error) =
        persist_previous_response_owner(&state, "resp_beta", &affinity_secret, beta.account_id())
    {
        panic!("affinity owner should persist: {error}");
    }
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    let projection =
        codex_router_state::selection_projection::project_route_band_selection_inputs_read_only(
            &async_state,
            "responses",
            now_unix_seconds,
            300,
        )
        .await
        .expect("credit-backed affinity projection should load");
    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        RESPONSES_HTTP.clone(),
        projection.accounts().to_vec(),
    ));
    assert_eq!(
        assessment.selected_pool(),
        codex_router_selection::burn_down::SelectedPool::Reserve
    );
    for account in assessment.accounts() {
        assert_eq!(
            account.routing_reason(),
            codex_router_selection::burn_down::RoutingReason::CreditBacked
        );
    }

    let selector = AsyncRepositoryBackedAccountSelector::new(&async_state);
    let selected = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_body(br#"{"previous_response_id":"resp_beta"}"#.to_vec()),
            TokenGeneration::new(1),
            Some(&affinity_secret),
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("affinity owner should select: {error}"),
    };

    assert_eq!(selected.account_id(), beta.account_id());
    assert_eq!(selected.selection_reason(), "previous_response_affinity");
    assert!(selected.credit_backed_at_selection());
}

#[test]
fn repository_backed_selector_allows_reserve_affinity_owner_outside_selected_pool() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_reserve_affinity_owner");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let alpha = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_alpha"),
        "alpha",
        AccountStatus::Enabled,
    );
    let beta = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_beta"),
        "beta",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 5, true), (604_800, 80, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &beta,
        "responses",
        &[(18_000, 90, true), (604_800, 20, false)],
    );
    let affinity_secret = test_affinity_secret();
    if let Err(error) =
        persist_previous_response_owner(&state, "resp_beta", &affinity_secret, beta.account_id())
    {
        panic!("affinity owner should persist: {error}");
    }

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_body(br#"{"previous_response_id":"resp_beta"}"#.to_vec()),
        TokenGeneration::new(1),
        Some(&affinity_secret),
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("reserve affinity owner should remain usable: {error}"),
    };

    assert_eq!(selected.account_id(), beta.account_id());
    assert_eq!(selected.selection_reason(), "previous_response_affinity");
}

#[test]
fn repository_backed_selector_keeps_low_balance_affinity_owner() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_low_balance_affinity_owner");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let preferred = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_preferred"),
        "preferred",
        AccountStatus::Enabled,
    );
    let retiring = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_retiring"),
        "retiring",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &preferred,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &retiring,
        "responses",
        &[(18_000, 4, true), (604_800, 80, false)],
    );
    let affinity_secret = test_affinity_secret();
    if let Err(error) = persist_previous_response_owner(
        &state,
        "resp_retiring",
        &affinity_secret,
        retiring.account_id(),
    ) {
        panic!("affinity owner should persist: {error}");
    }

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_body(br#"{"previous_response_id":"resp_retiring"}"#.to_vec()),
        TokenGeneration::new(1),
        Some(&affinity_secret),
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("retiring affinity owner should remain usable: {error}"),
    };

    assert_eq!(selected.account_id(), retiring.account_id());
    assert_eq!(selected.selection_reason(), "previous_response_affinity");
}

#[test]
fn repository_backed_selector_ignores_previous_response_id_for_non_capable_routes() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_non_capable_previous_response");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let preferred = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_compact_preferred"),
        "compact-preferred",
        AccountStatus::Enabled,
    );
    let affinity_owner = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_compact_affinity_owner"),
        "compact-affinity-owner",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &preferred,
        "responses_compact",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &affinity_owner,
        "responses_compact",
        &[(18_000, 40, true), (604_800, 40, false)],
    );
    let affinity_secret = test_affinity_secret();
    if let Err(error) = persist_previous_response_owner_for_route(
        &state,
        "resp_passthrough",
        &affinity_secret,
        affinity_owner.account_id(),
        RouteBand::ResponsesCompact,
    ) {
        panic!("compact affinity owner should persist: {error}");
    }

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses/compact")
            .with_body(br#"{"previous_response_id":"resp_passthrough"}"#.to_vec()),
        TokenGeneration::new(1),
        Some(&affinity_secret),
    ) {
        Ok(selected) => selected,
        Err(error) => panic!("non-capable route should ignore affinity field: {error}"),
    };

    assert_eq!(selected.account_id(), preferred.account_id());
    assert_ne!(selected.selection_reason(), "previous_response_affinity");
}

#[test]
fn repository_backed_selector_affinity_missing_owner_fails_closed() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_missing_owner");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let alpha = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_alpha"),
        "alpha",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );

    let selector = RepositoryBackedAccountSelector::new(&state);

    assert_eq!(
        selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_body(br#"{"previous_response_id":"resp_missing"}"#.to_vec()),
            TokenGeneration::new(1),
            Some(&test_affinity_secret()),
        ),
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerMissing
        })
    );
}

#[test]
fn repository_backed_selector_affinity_replaced_secret_ignores_stale_owner() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_replaced_secret");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let alpha = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_alpha"),
        "alpha",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &alpha,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    let original_secret = test_affinity_secret();
    if let Err(error) = persist_previous_response_owner(
        &state,
        "resp_old_secret",
        &original_secret,
        alpha.account_id(),
    ) {
        panic!("affinity owner should persist with original secret: {error}");
    }

    let selector = RepositoryBackedAccountSelector::new(&state);

    assert_eq!(
        selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_body(br#"{"previous_response_id":"resp_old_secret"}"#.to_vec()),
            TokenGeneration::new(1),
            Some(&replacement_affinity_secret()),
        ),
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerMissing
        })
    );
}

#[test]
fn repository_backed_selector_affinity_ineligible_owner_fails_closed() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_owner_ineligible");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let exhausted = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_exhausted"),
        "exhausted",
        AccountStatus::Enabled,
    );
    let eligible = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_eligible"),
        "eligible",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &exhausted,
        "responses",
        &[(18_000, 0, true), (604_800, 0, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &eligible,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    let affinity_secret = test_affinity_secret();
    if let Err(error) = persist_previous_response_owner(
        &state,
        "resp_exhausted",
        &affinity_secret,
        exhausted.account_id(),
    ) {
        panic!("affinity owner should persist: {error}");
    }

    let selector = RepositoryBackedAccountSelector::new(&state);

    assert_eq!(
        selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_body(br#"{"previous_response_id":"resp_exhausted"}"#.to_vec()),
            TokenGeneration::new(1),
            Some(&affinity_secret),
        ),
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable
        })
    );
}

#[tokio::test]
async fn async_repository_backed_selector_affinity_ineligible_owner_fails_closed() {
    let temp_dir = ProxyTestTempDir::new("async_repository_selector_affinity_ineligible");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let exhausted = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_exhausted"),
        "exhausted",
        AccountStatus::Enabled,
    );
    let eligible = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_eligible"),
        "eligible",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &exhausted,
        "responses",
        &[(18_000, 0, true), (604_800, 0, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &eligible,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    let affinity_secret = test_affinity_secret();
    if let Err(error) = persist_previous_response_owner(
        &state,
        "resp_exhausted",
        &affinity_secret,
        exhausted.account_id(),
    ) {
        panic!("affinity owner should persist: {error}");
    }
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };

    let selector = AsyncRepositoryBackedAccountSelector::new(&async_state);

    assert_eq!(
        selector
            .select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses")
                    .with_body(br#"{"previous_response_id":"resp_exhausted"}"#.to_vec()),
                TokenGeneration::new(1),
                Some(&affinity_secret),
            )
            .await,
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable
        })
    );
}
