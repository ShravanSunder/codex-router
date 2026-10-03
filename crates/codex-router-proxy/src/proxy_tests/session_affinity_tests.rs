use super::*;

#[tokio::test]
async fn prompt_cache_account_affinity_reuses_mapped_account_inside_two_hours() {
    let temp_dir = ProxyTestTempDir::new("prompt_cache_account_affinity_reuses_inside_ttl");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path).expect("state store should open");
    let preferred = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_preferred"),
        "preferred",
        AccountStatus::Enabled,
    );
    let mapped = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_mapped"),
        "mapped",
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
        &mapped,
        "responses",
        &[(18_000, 50, true), (604_800, 50, false)],
    );
    drop(state);
    let async_state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("async state should open");
    async_state
        .upsert_session_account_affinity(&SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            " session-inside-ttl ",
            mapped.account_id().clone(),
            10_001,
        ))
        .await
        .expect("session affinity should persist");
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        0,
        Arc::new(|| 10_000 + PROMPT_CACHE_ACCOUNT_AFFINITY_IDLE_TTL_SECONDS),
    );

    let selected = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("session-id", " session-inside-ttl ")),
            TokenGeneration::new(1),
            None,
        )
        .await
        .expect("mapped session account should select");

    assert_eq!(selected.account_id(), mapped.account_id());
    assert_eq!(selected.selection_reason(), "prompt_cache_account_affinity");
}

#[tokio::test]
async fn prompt_cache_account_affinity_expires_at_exactly_two_hours() {
    let temp_dir = ProxyTestTempDir::new("prompt_cache_account_affinity_expires_at_ttl");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path).expect("state store should open");
    let preferred = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_preferred"),
        "preferred",
        AccountStatus::Enabled,
    );
    let mapped = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_mapped"),
        "mapped",
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
        &mapped,
        "responses",
        &[(18_000, 50, true), (604_800, 50, false)],
    );
    drop(state);
    let async_state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("async state should open");
    async_state
        .upsert_session_account_affinity(&SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-at-ttl",
            mapped.account_id().clone(),
            10_000,
        ))
        .await
        .expect("session affinity should persist");
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        0,
        Arc::new(|| 10_000 + PROMPT_CACHE_ACCOUNT_AFFINITY_IDLE_TTL_SECONDS),
    );

    let selected = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("session-id", "session-at-ttl")),
            TokenGeneration::new(1),
            None,
        )
        .await
        .expect("normal selection should fall back");

    assert_eq!(selected.account_id(), preferred.account_id());
    assert_ne!(selected.selection_reason(), "prompt_cache_account_affinity");
}

#[tokio::test]
async fn unavailable_prompt_cache_account_affinity_falls_back_to_normal_selection() {
    let temp_dir = ProxyTestTempDir::new("prompt_cache_account_affinity_unavailable_fallback");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path).expect("state store should open");
    let available = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_available"),
        "available",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &available,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    drop(state);
    let async_state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("async state should open");
    async_state
        .upsert_session_account_affinity(&SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-unavailable",
            account_id("acct_missing"),
            10_000,
        ))
        .await
        .expect("session affinity should persist");
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        0,
        Arc::new(|| 10_100),
    );

    let selected = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("session-id", "session-unavailable")),
            TokenGeneration::new(1),
            None,
        )
        .await
        .expect("normal selection should fall back");

    assert_eq!(selected.account_id(), available.account_id());
    assert_ne!(selected.selection_reason(), "prompt_cache_account_affinity");
}

#[tokio::test]
async fn first_normal_selection_persists_prompt_cache_account_affinity() {
    let temp_dir = ProxyTestTempDir::new("first_selection_persists_session_affinity");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path).expect("state store should open");
    let selected_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_first_selection"),
        "first-selection",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &selected_account,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    drop(state);
    let async_state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("async state should open");
    let writer = DbWriteActor::start(
        Arc::new(SqliteDbWriteRepository::new(async_state.clone())),
        4,
    );
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        0,
        Arc::new(|| 10_100),
    )
    .with_session_affinity_writer(writer.clone());

    let selected = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("session-id", "session-first-selection")),
            TokenGeneration::new(1),
            None,
        )
        .await
        .expect("normal account should select");
    assert_eq!(selected.account_id(), selected_account.account_id());

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if async_state
                .load_session_account_affinity(Provider::Openai, "session-first-selection")
                .await
                .expect("session affinity should load")
                == Some(SessionAccountAffinity::new(
                    codex_router_core::provider::Provider::Openai,
                    "session-first-selection",
                    selected_account.account_id().clone(),
                    10_100,
                ))
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("normal selection should persist session affinity");
    writer.shutdown().await;
}

#[tokio::test]
async fn previous_response_affinity_wins_and_refreshes_prompt_cache_account_affinity() {
    let temp_dir = ProxyTestTempDir::new("previous_response_refreshes_session_affinity");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path).expect("state store should open");
    let session_owner = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_session_owner"),
        "session-owner",
        AccountStatus::Enabled,
    );
    let response_owner = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_response_owner"),
        "response-owner",
        AccountStatus::Enabled,
    );
    persist_account_with_selector_window_specs(
        &state,
        &session_owner,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    persist_account_with_selector_window_specs(
        &state,
        &response_owner,
        "responses",
        &[(18_000, 100, true), (604_800, 100, false)],
    );
    let affinity_secret = test_affinity_secret();
    persist_previous_response_owner(
        &state,
        "resp-owner",
        &affinity_secret,
        response_owner.account_id(),
    )
    .expect("previous response owner should persist");
    drop(state);
    let async_state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("async state should open");
    async_state
        .upsert_session_account_affinity(&SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "session-rebound",
            session_owner.account_id().clone(),
            10_000,
        ))
        .await
        .expect("session affinity should persist");
    let writer = DbWriteActor::start(
        Arc::new(SqliteDbWriteRepository::new(async_state.clone())),
        4,
    );
    let selector = AsyncRepositoryBackedAccountSelector::new_with_runtime(
        &async_state,
        RouteBandWeightedSelectors::default(),
        RouteBandAccountHolds::default(),
        0,
        Arc::new(|| 10_100),
    )
    .with_session_affinity_writer(writer.clone());

    let selected = selector
        .select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("session-id", "session-rebound"))
                .with_body(br#"{"previous_response_id":"resp-owner"}"#.to_vec()),
            TokenGeneration::new(1),
            Some(&affinity_secret),
        )
        .await
        .expect("previous response owner should select");
    assert_eq!(selected.account_id(), response_owner.account_id());
    assert_eq!(selected.selection_reason(), "previous_response_affinity");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let affinity = async_state
                .load_session_account_affinity(Provider::Openai, "session-rebound")
                .await
                .expect("session affinity should load")
                .expect("session affinity should exist");
            if affinity.account_id() == Some(response_owner.account_id())
                && affinity.last_seen_unix_seconds() == 10_100
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("previous response selection should refresh session affinity");
    writer.shutdown().await;
}
