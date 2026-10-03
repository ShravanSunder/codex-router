use super::*;

#[test]
fn websocket_first_response_create_frame_selects_and_forwards_unchanged() {
    let router = WebSocketProtocolRouter::new();
    let frame = WebSocketFrame::Text(
        br#"{"type":"response.create","unknown_codex_field":{"kept":true}}"#.to_vec(),
    );
    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("X-Codex-Router-Token", "local-token"))
            .with_header(Header::new("Host", "127.0.0.1:8787"))
            .with_header(Header::new("Sec-WebSocket-Key", "client-key"))
            .with_header(Header::new("Authorization", "Bearer wrong"))
            .with_header(Header::new("ChatGPT-Account-Id", "hostile-account-id"))
            .with_header(Header::new("Connection", "upgrade"))
            .with_header(Header::new("Upgrade", "websocket"))
            .with_header(Header::new("OpenAI-Beta", "responses=v1")),
        frame.clone(),
        SecretString::new("selected-upstream-token"),
        Some("chatgpt-account-id-canary"),
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("valid first frame should route: {error:?}"),
    };

    let WebSocketFirstFrameDecision::OpenUpstream {
        headers,
        first_frame,
        ..
    } = decision;
    assert_eq!(first_frame, frame);
    assert_eq!(headers.value("openai-beta"), Some("responses=v1"));
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
    assert_eq!(
        headers.value("chatgpt-account-id"),
        Some("chatgpt-account-id-canary")
    );
    assert_eq!(headers.value("x-codex-router-token"), None);
    assert_eq!(headers.value("host"), None);
    assert_eq!(headers.value("sec-websocket-key"), None);
    assert_eq!(headers.value("connection"), None);
    assert_eq!(headers.value("upgrade"), None);
}

#[test]
fn websocket_first_future_json_payload_selects_and_forwards_unchanged() {
    let router = WebSocketProtocolRouter::new();
    let frame = WebSocketFrame::Text(
        br#"{"future_codex_shape":{"kept":true},"unknown_codex_field":{"kept":true}}"#.to_vec(),
    );

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("X-Codex-Router-Token", "local-token"))
            .with_header(Header::new("Authorization", "Bearer wrong")),
        frame.clone(),
        SecretString::new("selected-upstream-token"),
        Some("chatgpt-account-id-canary"),
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("future JSON first frame should route: {error:?}"),
    };

    let WebSocketFirstFrameDecision::OpenUpstream {
        headers,
        first_frame,
        ..
    } = decision;
    assert_eq!(first_frame, frame);
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
    assert_eq!(
        headers.value("chatgpt-account-id"),
        Some("chatgpt-account-id-canary")
    );
    assert_eq!(headers.value("x-codex-router-token"), None);
}

#[test]
fn authenticated_websocket_router_selects_after_local_auth_and_first_frame() {
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
    let frame = WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec());

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_header(Header::new("Authorization", "Bearer current-token"))
            .with_header(Header::new("session-id", "sync-session")),
        frame.clone(),
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("authenticated websocket should route: {error:?}"),
    };

    assert_eq!(
        selector.take_recorded(),
        vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
    );
    assert_eq!(
        selector.take_recorded_session_ids(),
        vec![Some("sync-session".to_owned())]
    );
    let WebSocketFirstFrameDecision::OpenUpstream {
        headers,
        first_frame,
        ..
    } = decision;
    assert_eq!(first_frame, frame);
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
    assert_eq!(headers.value("x-codex-router-token"), None);
}

#[test]
fn authenticated_websocket_router_routes_around_request_local_credential_failure() {
    let first_account_id = account_id("acct_first");
    let second_account_id = account_id("acct_second");
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = QuotaAwareAccountSelector::new(vec![
        QuotaAwareAccountState::new(
            first_account_id.clone(),
            90,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
        QuotaAwareAccountState::new(
            second_account_id,
            80,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
    ]);
    let resolver =
        FailFirstProviderCredentialResolver::new(first_account_id, "second-websocket-token");
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
    let frame = WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec());

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("X-Codex-Router-Token", "current-token")),
        frame,
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("websocket should route around failed credentials: {error:?}"),
    };

    assert_eq!(
        resolver.take_recorded(),
        vec!["acct_first".to_owned(), "acct_second".to_owned()]
    );
    let WebSocketFirstFrameDecision::OpenUpstream { headers, .. } = decision;
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer second-websocket-token"]
    );
}

#[test]
fn authenticated_websocket_router_reports_credential_failure_when_all_credentials_fail() {
    let first_account_id = account_id("acct_first");
    let second_account_id = account_id("acct_second");
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = QuotaAwareAccountSelector::new(vec![
        QuotaAwareAccountState::new(
            first_account_id,
            90,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
        QuotaAwareAccountState::new(
            second_account_id,
            80,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
    ]);
    let resolver =
        RejectingProviderCredentialResolver::new(CredentialResolverError::RefreshUnavailable);
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
    let frame = WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec());

    let error = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("X-Codex-Router-Token", "current-token")),
        frame,
    ) {
        Ok(decision) => panic!("all credential failures should fail closed: {decision:?}"),
        Err(error) => error,
    };

    assert_eq!(error, WebSocketCloseReason::ProviderCredential);
    assert_eq!(
        resolver.take_recorded(),
        vec!["acct_first".to_owned(), "acct_second".to_owned()]
    );
}

#[tokio::test]
async fn async_authenticated_websocket_router_selects_after_local_auth_and_first_frame() {
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingAsyncSelector::default();
    let resolver = RecordingAsyncProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let router =
        AsyncAuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
    let frame = WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec());

    let decision = match router
        .route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_header(Header::new("session-id", "async-session")),
            frame.clone(),
        )
        .await
    {
        Ok(decision) => decision,
        Err(error) => panic!("async authenticated websocket should route: {error:?}"),
    };

    assert_eq!(
        selector.take_recorded(),
        vec![("/v1/responses".to_owned(), TokenGeneration::new(1))],
    );
    assert_eq!(
        selector.take_recorded_session_ids(),
        vec![Some("async-session".to_owned())],
    );
    assert_eq!(resolver.take_recorded(), vec!["acct_selected".to_owned()],);
    let WebSocketFirstFrameDecision::OpenUpstream {
        token_generation,
        headers,
        first_frame,
        affinity_owner_context,
    } = decision;
    assert_eq!(token_generation, TokenGeneration::new(1));
    assert_eq!(first_frame, frame);
    assert!(affinity_owner_context.is_some());
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer selected-upstream-token"],
    );
    assert_eq!(headers.value("x-codex-router-token"), None);
}

#[tokio::test]
async fn async_websocket_router_reuses_and_refreshes_prompt_cache_account_affinity() {
    let temp_dir = ProxyTestTempDir::new("websocket_prompt_cache_account_affinity");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = SqliteStateStore::open(&database_path).expect("state store should open");
    let preferred = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_websocket_preferred"),
        "websocket-preferred",
        AccountStatus::Enabled,
    );
    let mapped = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_websocket_mapped"),
        "websocket-mapped",
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
            "websocket-session",
            mapped.account_id().clone(),
            1_000,
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
        Arc::new(|| 1_100),
    )
    .with_session_affinity_writer(writer.clone());
    let resolver = RecordingAsyncProviderCredentialResolver::new("selected-upstream-token");
    let protocol_router = WebSocketProtocolRouter::new();
    let auth_gate = local_auth_gate();
    let router =
        AsyncAuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    router
        .route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_header(Header::new("session-id", "websocket-session")),
            WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
        )
        .await
        .expect("websocket session affinity should route");

    assert_eq!(
        resolver.take_recorded(),
        vec![mapped.account_id().as_str().to_owned()]
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let affinity = async_state
                .load_session_account_affinity(Provider::Openai, "websocket-session")
                .await
                .expect("session affinity should load")
                .expect("session affinity should exist");
            if affinity.last_seen_unix_seconds() == 1_100 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("websocket session affinity should refresh");
    writer.shutdown().await;
}

#[tokio::test]
async fn async_authenticated_websocket_router_routes_around_request_local_credential_failure() {
    let first_account_id = account_id("acct_first");
    let second_account_id = account_id("acct_second");
    let protocol_router = WebSocketProtocolRouter::new();
    let selector =
        ExclusionAwareAsyncSelector::new(vec![first_account_id.clone(), second_account_id.clone()]);
    let resolver = FailFirstAsyncProviderCredentialResolver::new(
        first_account_id.clone(),
        "second-async-websocket-token",
    );
    let auth_gate = local_auth_gate();
    let router =
        AsyncAuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
    let frame = WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec());

    let decision = match router
        .route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token")),
            frame,
        )
        .await
    {
        Ok(decision) => decision,
        Err(error) => {
            panic!("async websocket should route around failed credentials: {error:?}")
        }
    };

    assert_eq!(
        selector.take_recorded(),
        vec![
            ("acct_first".to_owned(), TokenGeneration::new(1)),
            ("acct_second".to_owned(), TokenGeneration::new(1)),
        ]
    );
    assert_eq!(
        resolver.take_recorded(),
        vec!["acct_first".to_owned(), "acct_second".to_owned()]
    );
    let WebSocketFirstFrameDecision::OpenUpstream { headers, .. } = decision;
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer second-async-websocket-token"]
    );
}
