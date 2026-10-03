use super::*;

#[tokio::test]
async fn websocket_credential_resolution_uses_websocket_profile_provider() {
    let account_id = AccountId::new("acct_ws_provider_test")
        .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
    let requested_providers = Arc::new(Mutex::new(Vec::new()));
    let selector = FixedAsyncSelector {
        account_id: account_id.clone(),
        credit_backed_at_selection: false,
    };
    let resolver = RecordingAsyncCredentialResolver {
        account_id,
        requested_providers: Arc::clone(&requested_providers),
    };
    let auth_gate = ProxyLocalAuthGate::disabled();
    let affinity_secret_provider = FixedAffinitySecretProvider::new();
    let protocol_router = WebSocketProtocolRouter::new();
    let tunnel = AsyncWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
        .with_affinity_secret_provider(&affinity_secret_provider);

    let decision = tunnel
        .router
        .route_first_frame(
            WebSocketHandshakeRequest::new(),
            WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
        )
        .await;

    assert!(
        decision.is_ok(),
        "Responses WebSocket should resolve credentials"
    );
    assert_eq!(
        *requested_providers
            .lock()
            .unwrap_or_else(|_error| panic!("provider request lock should be available")),
        vec![RESPONSES_WEBSOCKET.provider]
    );
}

#[tokio::test]
async fn runtime_shutdown_cancels_pending_first_frame_routing() {
    let account_id = AccountId::new("acct_ws_test")
        .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
    let selector_started = Arc::new(Notify::new());
    let selector = PendingAsyncSelector {
        started: Arc::clone(&selector_started),
    };
    let credential_resolver = FixedAsyncCredentialResolver { account_id };
    let auth_gate = ProxyLocalAuthGate::disabled();
    let affinity_secret_provider = FixedAffinitySecretProvider::new();
    let protocol_router = WebSocketProtocolRouter::new();
    let registry = WebSocketRevocationRegistry::new();
    let session_shutdown = CancellationToken::new();
    let tunnel = AsyncWebSocketTunnel::new(
        &auth_gate,
        &selector,
        &credential_resolver,
        &protocol_router,
    )
    .with_revocation_registry(registry.clone())
    .with_session_shutdown(session_shutdown.clone())
    .with_affinity_secret_provider(&affinity_secret_provider);
    let (router_local_stream, client_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_future = tunnel.handle_upgraded_connection(
        router_local_websocket,
        WebSocketHandshakeRequest::new(),
        "ws://127.0.0.1:1/v1/responses",
    );
    let peer_future = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
        tokio::time::timeout(Duration::from_secs(1), selector_started.notified())
            .await
            .unwrap_or_else(|_elapsed| panic!("selector should start before shutdown"));
        session_shutdown.cancel();
    };
    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("router should exit while routing is pending"));

    assert!(
        router_result.is_ok(),
        "shutdown should close pending first-frame routing cleanly, got {router_result:?}"
    );
    assert_eq!(registry.snapshot().active_sessions, 0);
}

#[tokio::test]
async fn all_accounts_exhausted_sends_scrubbed_router_error_before_upstream_connect() {
    let account_id = AccountId::new("acct_ws_test")
        .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
    let selector = RejectingAsyncSelector {
        reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
    };
    let credential_resolver = FixedAsyncCredentialResolver { account_id };
    let auth_gate = ProxyLocalAuthGate::disabled();
    let affinity_secret_provider = FixedAffinitySecretProvider::new();
    let protocol_router = WebSocketProtocolRouter::new();
    let tunnel = AsyncWebSocketTunnel::new(
        &auth_gate,
        &selector,
        &credential_resolver,
        &protocol_router,
    )
    .with_affinity_secret_provider(&affinity_secret_provider);
    let (router_local_stream, client_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_future = tunnel.handle_upgraded_connection(
        router_local_websocket,
        WebSocketHandshakeRequest::new(),
        "ws://127.0.0.1:1/v1/responses",
    );
    let peer_future = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
        let message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive router exhausted error: {error}"),
            None => panic!("client should receive router exhausted error"),
        };
        let rendered = message.to_string();
        assert_eq!(rendered, super::ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL);
        assert!(!rendered.contains("acct_"));
        assert!(rendered.contains("usage_limit_reached"));
        assert!(rendered.contains("codex_router_all_accounts_exhausted"));
    };

    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("router should return exhausted error promptly"));

    assert!(
        router_result.is_ok(),
        "all-accounts exhausted should be a clean router-level WebSocket error, got {router_result:?}"
    );
}

#[tokio::test]
async fn short_quota_exhaustion_sends_retryable_response_failed_before_upstream_connect() {
    let account_id = AccountId::new("acct_ws_test")
        .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
    let selector = RejectingAsyncSelector {
        reason: QuotaAwareAccountSelectorError::ShortQuotaExhausted {
            retry_after_seconds: 2,
        },
    };
    let credential_resolver = FixedAsyncCredentialResolver { account_id };
    let auth_gate = ProxyLocalAuthGate::disabled();
    let affinity_secret_provider = FixedAffinitySecretProvider::new();
    let protocol_router = WebSocketProtocolRouter::new();
    let tunnel = AsyncWebSocketTunnel::new(
        &auth_gate,
        &selector,
        &credential_resolver,
        &protocol_router,
    )
    .with_affinity_secret_provider(&affinity_secret_provider);
    let (router_local_stream, client_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_future = tunnel.handle_upgraded_connection(
        router_local_websocket,
        WebSocketHandshakeRequest::new(),
        "ws://127.0.0.1:1/v1/responses",
    );
    let peer_future = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
        let message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive short quota wait event: {error}"),
            None => panic!("client should receive short quota wait event"),
        };
        assert_eq!(
            message.to_string(),
            r#"{"type":"response.failed","response":{"id":"resp_router_short_quota_wait","status":"failed","error":{"code":"rate_limit_exceeded","message":"Rate limit exceeded. Try again in 2 seconds."}}}"#,
        );
    };

    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("router should deliver a short quota wait event promptly"));

    assert!(
        router_result.is_ok(),
        "short-only quota exhaustion should be a clean retryable router signal, got {router_result:?}"
    );
}

#[tokio::test]
async fn state_unavailable_sends_scrubbed_router_error_before_upstream_connect() {
    let account_id = AccountId::new("acct_ws_test")
        .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
    let selector = RejectingAsyncSelector {
        reason: QuotaAwareAccountSelectorError::StateUnavailable,
    };
    let credential_resolver = FixedAsyncCredentialResolver { account_id };
    let auth_gate = ProxyLocalAuthGate::disabled();
    let affinity_secret_provider = FixedAffinitySecretProvider::new();
    let protocol_router = WebSocketProtocolRouter::new();
    let tunnel = AsyncWebSocketTunnel::new(
        &auth_gate,
        &selector,
        &credential_resolver,
        &protocol_router,
    )
    .with_affinity_secret_provider(&affinity_secret_provider);
    let (router_local_stream, client_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_future = tunnel.handle_upgraded_connection(
        router_local_websocket,
        WebSocketHandshakeRequest::new(),
        "ws://127.0.0.1:1/v1/responses",
    );
    let peer_future = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
        let message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => {
                panic!("client should receive router state-unavailable error: {error}")
            }
            None => panic!("client should receive router state-unavailable error"),
        };
        let rendered = message.to_string();
        assert_eq!(rendered, super::ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL);
        assert!(!rendered.contains("acct_"));
        assert!(!rendered.contains("usage_limit_reached"));
        assert!(!rendered.contains("sk-live"));
        assert!(!rendered.contains("Authorization"));
    };

    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("router should return state-unavailable error promptly"));

    assert!(
        router_result.is_ok(),
        "state-unavailable should be a clean router-level WebSocket error, got {router_result:?}"
    );
}

#[tokio::test]
async fn runtime_shutdown_cancels_pending_upstream_connect() {
    let account_id = AccountId::new("acct_ws_test")
        .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
    let selector = FixedAsyncSelector {
        account_id: account_id.clone(),
        credit_backed_at_selection: false,
    };
    let credential_resolver = FixedAsyncCredentialResolver { account_id };
    let auth_gate = ProxyLocalAuthGate::disabled();
    let affinity_secret_provider = FixedAffinitySecretProvider::new();
    let protocol_router = WebSocketProtocolRouter::new();
    let registry = WebSocketRevocationRegistry::new();
    let session_shutdown = CancellationToken::new();
    let tunnel = AsyncWebSocketTunnel::new(
        &auth_gate,
        &selector,
        &credential_resolver,
        &protocol_router,
    )
    .with_revocation_registry(registry.clone())
    .with_session_shutdown(session_shutdown.clone())
    .with_affinity_secret_provider(&affinity_secret_provider);
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap_or_else(|error| panic!("blackhole listener should bind: {error}"));
    let upstream_url = format!(
        "ws://{}/v1/responses",
        listener
            .local_addr()
            .unwrap_or_else(|error| panic!("local addr should read: {error}"))
    );
    let accepted = Arc::new(Notify::new());
    let accepted_for_task = Arc::clone(&accepted);
    let accept_task = tokio::spawn(async move {
        let (_stream, _addr) = listener
            .accept()
            .await
            .unwrap_or_else(|error| panic!("blackhole should accept: {error}"));
        accepted_for_task.notify_one();
        std::future::pending::<()>().await;
    });
    let (router_local_stream, client_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_future = tunnel.handle_upgraded_connection(
        router_local_websocket,
        WebSocketHandshakeRequest::new(),
        &upstream_url,
    );
    let peer_future = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
        tokio::time::timeout(Duration::from_secs(1), accepted.notified())
            .await
            .unwrap_or_else(|_elapsed| panic!("upstream connection should be accepted"));
        assert_eq!(registry.snapshot().active_sessions, 1);
        session_shutdown.cancel();
    };
    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("router should exit while connect is pending"));
    accept_task.abort();

    assert!(
        router_result.is_ok(),
        "shutdown should close pending upstream connect cleanly, got {router_result:?}"
    );
    assert_eq!(registry.snapshot().active_sessions, 0);
}
