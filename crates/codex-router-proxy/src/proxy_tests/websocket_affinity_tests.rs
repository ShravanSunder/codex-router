use super::*;

#[test]
fn authenticated_websocket_router_rejects_mismatched_local_auth_carriers() {
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    assert_eq!(
        router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_header(Header::new("Authorization", "Bearer wrong")),
            WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
        ),
        Err(WebSocketCloseReason::LocalAuth {
            reason: LocalAuthError::Wrong
        })
    );
    assert!(selector.take_recorded().is_empty());
}

#[test]
fn authenticated_websocket_router_rejects_first_frame_auth_smuggling_before_selection() {
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    assert_eq!(
        router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token")),
            WebSocketFrame::Text(
                br#"{"type":"response.create","authorization":"Bearer current-token"}"#.to_vec()
            ),
        ),
        Err(WebSocketCloseReason::UnexpectedFirstFrame)
    );
    assert!(selector.take_recorded().is_empty());
}

#[test]
fn authenticated_websocket_router_routes_malformed_first_frame_to_upstream_owner() {
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("X-Codex-Router-Token", "current-token")),
        WebSocketFrame::Text(br#"{"type":"#.to_vec()),
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("router must not reject malformed Codex payload: {error:?}"),
    };

    assert_eq!(
        selector.take_recorded(),
        vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
    );
    assert_eq!(resolver.take_recorded(), vec!["acct_selected".to_owned()]);
    let WebSocketFirstFrameDecision::OpenUpstream {
        headers,
        first_frame,
        ..
    } = decision;
    assert_eq!(first_frame, WebSocketFrame::Text(br#"{"type":"#.to_vec()));
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
}

#[test]
fn websocket_first_frame_rejects_hostile_preselection_cases() {
    let router = WebSocketProtocolRouter::new();

    let binary_decision = router.route_first_frame(
        WebSocketHandshakeRequest::new(),
        WebSocketFrame::Binary(vec![1, 2, 3]),
        SecretString::new("selected-upstream-token"),
        None,
    );
    assert!(
        binary_decision.is_ok(),
        "router must not reject binary frames before upstream"
    );
    let large_decision = router.route_first_frame(
        WebSocketHandshakeRequest::new(),
        WebSocketFrame::Text(br#"{"type":"response.create","padding":"too-large"}"#.to_vec()),
        SecretString::new("selected-upstream-token"),
        None,
    );
    assert!(
        large_decision.is_ok(),
        "router must not own a first-frame size cap"
    );
    assert_eq!(
        router
            .route_first_frame(
                WebSocketHandshakeRequest::new(),
                WebSocketFrame::Text(br#"{"x-codex-router-token":"smuggled"}"#.to_vec()),
                SecretString::new("selected-upstream-token"),
                None,
            )
            .err(),
        Some(WebSocketCloseReason::UnexpectedFirstFrame)
    );
    let malformed_decision = router.route_first_frame(
        WebSocketHandshakeRequest::new(),
        WebSocketFrame::Text(br#"{"type":"#.to_vec()),
        SecretString::new("selected-upstream-token"),
        None,
    );
    assert!(
        malformed_decision.is_ok(),
        "router must not reject malformed Codex-owned payloads"
    );
}

#[test]
fn authenticated_websocket_router_requires_affinity_secret_before_selection() {
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router);

    assert_eq!(
        router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token")),
            WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
        ),
        Err(WebSocketCloseReason::Selection {
            reason: QuotaAwareAccountSelectorError::SecretUnavailable
        })
    );
    assert!(selector.take_recorded().is_empty());
    assert!(resolver.take_recorded().is_empty());
}

#[test]
fn authenticated_websocket_router_routes_previous_response_affinity_owner() {
    let temp_dir = ProxyTestTempDir::new("websocket-router-affinity");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(SqliteStateStore::open(&database_path));
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

    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RepositoryBackedAccountSelector::new(&state);
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("X-Codex-Router-Token", "current-token")),
        WebSocketFrame::Text(
            br#"{"type":"response.create","previous_response_id":"resp_beta"}"#.to_vec(),
        ),
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("websocket affinity owner should route: {error:?}"),
    };

    assert_eq!(resolver.take_recorded(), vec!["acct_beta".to_owned()]);
    let WebSocketFirstFrameDecision::OpenUpstream { headers, .. } = decision;
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
}

#[test]
fn authenticated_websocket_router_replaced_affinity_secret_fails_continuation_closed() {
    let temp_dir = ProxyTestTempDir::new("websocket-router-replaced-affinity-secret");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(SqliteStateStore::open(&database_path));
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

    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RepositoryBackedAccountSelector::new(&state);
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let replacement_secret_provider =
        FixedAffinitySecretProvider::new(replacement_affinity_secret());
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&replacement_secret_provider);

    assert_eq!(
        router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token")),
            WebSocketFrame::Text(
                br#"{"type":"response.create","previous_response_id":"resp_old_secret"}"#.to_vec(),
            ),
        ),
        Err(WebSocketCloseReason::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerMissing
        })
    );
    assert!(resolver.take_recorded().is_empty());
}

#[test]
fn authenticated_websocket_router_refreshes_expired_access_token_before_upstream_open() {
    let temp_dir = ProxyTestTempDir::new("websocket-router-refresh");
    let state = must_ok(SqliteStateStore::open(
        &temp_dir.path().join("state.sqlite"),
    ));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("acct_selected");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "selected",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let expired_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &expired_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "stale-websocket-access-token",
                    Some("websocket-refresh-token".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let refresh_client = RecordingRefreshClient::new(
        "acct_selected",
        "websocket-refresh-token",
        AccountCredentialBundle::imported_codex_auth(
            "refreshed-websocket-access-token",
            Some("refreshed-websocket-refresh-token".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    );
    let resolver = RouterCredentialResolver::new(&state, &secrets, refresh_client.clone(), 1_000);
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingSelector::new();
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
    let frame = WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec());

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_header(Header::new("Authorization", "Bearer current-token")),
        frame,
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("expired websocket credential should refresh: {error:?}"),
    };

    let WebSocketFirstFrameDecision::OpenUpstream { headers, .. } = decision;
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer refreshed-websocket-access-token"]
    );
    assert_eq!(refresh_client.calls(), 1);
    let loaded_account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .unwrap_or_else(|| panic!("account should remain registered"));
    assert_eq!(loaded_account.active_credential_generation(), Some(2));
}

#[test]
fn proxy_credential_resolver_refreshes_expired_bundle_through_runtime_wrapper() {
    let temp_dir = ProxyTestTempDir::new("proxy-runtime-resolver-refresh");
    let state_database_path = temp_dir.path().join("state.sqlite");
    let secret_store_root = temp_dir.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_database_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            &secret_store_root,
        ),
    );
    let account_id = account_id("acct_proxy_runtime_refresh");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "runtime",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let expired_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &expired_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-proxy-runtime-access-token",
                    Some("proxy-runtime-refresh-token".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let refresh_client = RecordingRefreshClient::new(
        "acct_proxy_runtime_refresh",
        "proxy-runtime-refresh-token",
        AccountCredentialBundle::imported_codex_auth(
            "refreshed-proxy-runtime-access-token",
            Some("refreshed-proxy-runtime-refresh-token".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    );
    let resolver = must_ok(ProxyCredentialResolver::open_with_refresh_client(
        &state_database_path,
        &secret_store_root,
        1_000,
        refresh_client.clone(),
    ));

    let resolved =
        must_ok(resolver.resolve_provider_credentials(
            &account_id,
            codex_router_core::provider::Provider::Openai,
        ));

    assert_eq!(
        resolved.access_token().expose_secret(),
        "refreshed-proxy-runtime-access-token"
    );
    assert_eq!(refresh_client.calls(), 1);
    let loaded_account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .unwrap_or_else(|| panic!("account should remain registered"));
    assert_eq!(loaded_account.active_credential_generation(), Some(2));
}

#[test]
fn authenticated_websocket_router_missing_refresh_token_fails_closed_before_upstream_open() {
    let temp_dir = ProxyTestTempDir::new("websocket-router-missing-refresh");
    let state = must_ok(SqliteStateStore::open(
        &temp_dir.path().join("state.sqlite"),
    ));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("acct_selected");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "selected",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let expired_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &expired_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth("stale-websocket-access-token", None)
                    .with_expires_unix_seconds(900)
                    .to_secret_string(),
            ),
        ),
    );
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingSelector::new();
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    assert_eq!(
        router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token")),
            WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
        ),
        Err(WebSocketCloseReason::ProviderCredential)
    );
    assert_eq!(
        selector.take_recorded(),
        vec![
            ("/v1/responses".to_owned(), TokenGeneration::new(1)),
            ("/v1/responses".to_owned(), TokenGeneration::new(1)),
        ]
    );
}

#[test]
fn authenticated_websocket_router_rejects_missing_local_token_before_selection() {
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    assert_eq!(
        router.route_first_frame(
            WebSocketHandshakeRequest::new(),
            WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
        ),
        Err(WebSocketCloseReason::LocalAuth {
            reason: LocalAuthError::Missing
        })
    );
    assert!(selector.take_recorded().is_empty());
}

#[test]
fn authenticated_websocket_router_accepts_codex_env_key_authorization_bearer() {
    let protocol_router = WebSocketProtocolRouter::new();
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let router =
        AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("Authorization", "Bearer current-token")),
        WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("authorization bearer should satisfy local auth: {error:?}"),
    };

    assert_eq!(
        selector.take_recorded(),
        vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
    );
    let WebSocketFirstFrameDecision::OpenUpstream {
        token_generation,
        headers,
        ..
    } = decision;
    assert_eq!(token_generation, TokenGeneration::new(1));
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
}
