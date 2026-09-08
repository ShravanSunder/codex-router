//! Cancellation while a real upstream HTTP upgrade is pending.
use super::*;

#[tokio::test]
async fn weekly_floor_notification_during_upstream_handshake_prevents_first_frame_forwarding() {
    let account_id = AccountId::new("acct_floor_during_connect")
        .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
    let selector = FixedAsyncSelector {
        account_id: account_id.clone(),
    };
    let credential_resolver = FixedAsyncCredentialResolver {
        account_id: account_id.clone(),
    };
    let auth_gate = ProxyLocalAuthGate::disabled();
    let affinity_secret_provider = FixedAffinitySecretProvider::new();
    let protocol_router = WebSocketProtocolRouter::new();
    let registry = WebSocketRevocationRegistry::new();
    let notifier = WebSocketQuotaFloorNotifier::new(registry.clone());
    let tunnel = AsyncWebSocketTunnel::new(
        &auth_gate,
        &selector,
        &credential_resolver,
        &protocol_router,
    )
    .with_revocation_registry(registry.clone())
    .with_affinity_secret_provider(&affinity_secret_provider);
    let upstream_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("test upstream should bind: {error}"));
    let upstream_address = upstream_listener
        .local_addr()
        .unwrap_or_else(|error| panic!("test upstream address should resolve: {error}"));
    let (router_local_stream, client_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;

    let (handshake_sender, handshake_receiver) = tokio::sync::oneshot::channel();
    let upstream_accepted = std::cell::Cell::new(false);
    let router_completed = std::cell::Cell::new(false);
    let upstream_url = format!("ws://{upstream_address}/v1/responses");
    let router_future = async {
        let result = tunnel
            .handle_upgraded_connection(
                router_local_websocket,
                WebSocketHandshakeRequest::new(),
                &upstream_url,
            )
            .await;
        router_completed.set(true);
        result
    };
    let upstream_future = async {
        let (mut stream, _peer_address) = upstream_listener
            .accept()
            .await
            .unwrap_or_else(|error| panic!("test upstream should accept: {error}"));
        upstream_accepted.set(true);
        let mut received = Vec::new();
        // Observe the real upgrade request before triggering cancellation. Registry
        // admission can precede TCP acceptance, so it is not a handshake-ready signal.
        while !received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let mut chunk = [0_u8; 1024];
            let count = stream
                .read(&mut chunk)
                .await
                .unwrap_or_else(|error| panic!("read upgrade request: {error}"));
            assert!(count > 0, "upstream closed before its upgrade request");
            received.extend_from_slice(&chunk[..count]);
            assert!(
                received.len() <= 16 * 1024,
                "upgrade request exceeded fixture bound"
            );
        }
        assert!(received.starts_with(b"GET /v1/responses HTTP/1.1\r\n"));
        handshake_sender
            .send(())
            .unwrap_or_else(|()| panic!("handshake waiter should still be present"));
        // Deliberately withhold the HTTP upgrade response: cancellation must finish
        // while the real client is waiting for this handshake, not after it succeeds.
        stream
            .read_to_end(&mut received)
            .await
            .unwrap_or_else(|error| panic!("test upstream should read to close: {error}"));
        received
    };
    let peer_future = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
        handshake_receiver
            .await
            .unwrap_or_else(|error| panic!("upstream should observe handshake: {error}"));
        assert_eq!(registry.snapshot().active_sessions, 1);
        notifier.signal_weekly_quota_floor_reached(&account_id);
        client_websocket
            .next()
            .await
            .unwrap_or_else(|| panic!("client should receive reconnect signal"))
            .unwrap_or_else(|error| panic!("reconnect signal should be readable: {error}"))
            .to_string()
    };

    let (router_result, upstream_received, client_message) =
        tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(router_future, upstream_future, peer_future)
        })
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("floor cutoff should not wait for upstream handshake; router_completed={}, upstream_accepted={}", router_completed.get(), upstream_accepted.get())
        });

    assert!(
        router_result.is_ok(),
        "router should signal then close: {router_result:?}"
    );
    assert_eq!(client_message, CODEX_WEBSOCKET_RECONNECT_SIGNAL);
    assert!(!String::from_utf8_lossy(&upstream_received).contains("response.create"));
    assert_eq!(registry.snapshot().active_sessions, 0);
}
