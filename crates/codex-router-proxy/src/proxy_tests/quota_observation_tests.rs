use super::*;

#[tokio::test]
async fn usage_limit_observation_quarantines_account_before_next_selection() {
    let temp_dir = ProxyTestTempDir::new("usage_limit_observation_quarantine");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let limited = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_limited"),
        "limited",
        AccountStatus::Enabled,
    );
    let fallback = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_fallback"),
        "fallback",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &limited,
        "responses",
        &[(18_000, 90, true), (604_800, 90, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &fallback,
        "responses",
        &[(18_000, 80, true), (604_800, 80, false)],
    );
    let async_state = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(state) => state,
        Err(error) => panic!("async state store should open: {error}"),
    };
    let usage_limit_body = br#"{
            "type": "error",
            "status": 429,
            "error": {
                "type": "usage_limit_reached",
                "code": "usage_limit_reached"
            }
        }"#;

    if let Err(error) = record_provider_error_observation(
        &async_state,
        limited.account_id(),
        "responses",
        classify_provider_error_envelope(usage_limit_body),
        2_000,
    )
    .await
    {
        panic!("usage limit observation should persist: {error}");
    }

    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
        &async_state,
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(HashMap::new())),
        0,
        Arc::new(|| 2_000),
    );
    let selected = match selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        )
        .await
    {
        Ok(selected) => selected,
        Err(error) => panic!("fallback account should be selected: {error}"),
    };

    assert_eq!(selected.account_id(), fallback.account_id());
}

#[tokio::test]
async fn websocket_connection_limit_observation_keeps_account_selectable() {
    let connection_limit_body = br#"{
            "type": "error",
            "status": 400,
            "error": {
                "type": "invalid_request_error",
                "code": "websocket_connection_limit_reached"
            }
        }"#;

    let selected = selected_account_after_provider_error_observation(
        "connection_limit_observation",
        connection_limit_body,
    )
    .await;

    assert_eq!(selected, account_id("acct_observed"));
}

#[tokio::test]
async fn ambiguous_quota_text_observation_keeps_account_selectable() {
    let ambiguous_body = br#"{
            "type": "response.output_text.delta",
            "delta": "This turn explains usage_limit_reached but is not a provider error."
        }"#;

    let selected = selected_account_after_provider_error_observation(
        "ambiguous_quota_text_observation",
        ambiguous_body,
    )
    .await;

    assert_eq!(selected, account_id("acct_observed"));
}
