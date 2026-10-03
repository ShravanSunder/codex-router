use super::*;

#[test]
fn repository_backed_selector_ignores_malformed_affinity_key_and_selects_normally() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_malformed");
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

    let decision = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_body(br#"{"previous_response_id":42}"#.to_vec()),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("malformed affinity metadata should be ignored: {error}"),
    };
    assert_eq!(decision.account_id(), alpha.account_id());
}

#[test]
fn repository_backed_selector_ignores_prompt_text_previous_response_id_fragment() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_prompt_fragment");
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

    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses").with_body(
            br#"{"input":"please print \"previous_response_id\":\"resp_nested\" literally"}"#
                .to_vec(),
        ),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => {
            panic!("prompt text must not trigger previous_response_id affinity: {error:?}")
        }
    };

    assert_eq!(selected.account_id().as_str(), "acct_alpha");
}

#[test]
fn repository_backed_selector_ignores_malformed_prompt_previous_response_id_fragment() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_malformed_prompt");
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

    let selected = match selector.select_upstream_account(
        &HttpProxyRequest::new(Method::Post, "/v1/responses").with_body(
            br#"{"input":"please print "previous_response_id":"resp_nested" literally"#.to_vec(),
        ),
        TokenGeneration::new(1),
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => {
            panic!("malformed prompt text must not trigger affinity parsing: {error:?}")
        }
    };

    assert_eq!(selected.account_id().as_str(), "acct_alpha");
}

#[test]
fn repository_backed_selector_skips_account_with_ineligible_secondary_window() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_secondary_ineligible");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let weekly_exhausted = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_weekly_exhausted"),
        "weekly-exhausted",
        AccountStatus::Enabled,
    );
    let eligible = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_weekly_eligible"),
        "weekly-eligible",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_status_specs(
        &state,
        &weekly_exhausted,
        "responses",
        &[
            (18_000, 80, true, SelectorQuotaWindowStatus::Eligible),
            (604_800, 0, false, SelectorQuotaWindowStatus::Ineligible),
        ],
    );
    persist_account_with_selector_window_status_specs(
        &state,
        &eligible,
        "responses",
        &[
            (18_000, 42, true, SelectorQuotaWindowStatus::Eligible),
            (604_800, 42, false, SelectorQuotaWindowStatus::Eligible),
        ],
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
    assert_eq!(selected.selection_reason(), "preferred_safest_quota");
}

#[test]
fn repository_backed_selector_routes_with_fresh_weekly_only_quota() {
    let temp_dir = ProxyTestTempDir::new("repository_selector_weekly_only");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path)
        .unwrap_or_else(|error| panic!("state store should open: {error}"));
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_weekly_only"),
        "weekly-only",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &account,
        "responses",
        &[(604_800, 90, true)],
    );

    let selector = RepositoryBackedAccountSelector::new(&state);
    let selected = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        )
        .unwrap_or_else(|error| panic!("weekly-only quota should remain routable: {error:?}"));

    assert_eq!(selected.account_id(), account.account_id());
    assert_ne!(selected.selection_reason(), "fallback_unknown_quota");
}
