use super::*;

#[tokio::test]
async fn runtime_shutdown_cancels_active_duplex_pumps() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let router_upstream_websocket =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let mut upstream_websocket =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let registry = WebSocketRevocationRegistry::new();
    let session = registry.register_cancellation(TokenGeneration::new(1));
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();
    let session_shutdown_for_task = session_shutdown.clone();

    let router_task = tokio::spawn(async move {
        forward_duplex_until_complete(
            router_local_websocket,
            router_upstream_websocket,
            WebSocketForwardingContext {
                session_registration: session,
                affinity_owner_recorder: None,
                async_affinity_owner_recorder: None,
                affinity_record_tasks: TaskTracker::new(),
                affinity_owner_context: None,
                provider_error_observer: None,
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown_for_task,
            },
        )
        .await
    });
    client_websocket
        .send(Message::text(r#"{"type":"response.create"}"#))
        .await
        .unwrap_or_else(|error| panic!("local frame should send: {error}"));
    match upstream_websocket.next().await {
        Some(Ok(_message)) => {}
        Some(Err(error)) => panic!("upstream should receive frame: {error}"),
        None => panic!("upstream should receive frame"),
    }
    assert_eq!(registry.snapshot().active_sessions, 1);

    session_shutdown.cancel();
    let router_result = tokio::time::timeout(Duration::from_secs(1), router_task)
        .await
        .unwrap_or_else(|_elapsed| panic!("router pump should exit promptly on shutdown"))
        .unwrap_or_else(|error| panic!("router pump task should join: {error}"));
    assert!(
        router_result.is_ok(),
        "shutdown should close active pump cleanly, got {router_result:?}"
    );
    assert_eq!(registry.snapshot().active_sessions, 0);
}

#[tokio::test]
async fn weekly_floor_notification_sends_existing_reconnect_signal_and_closes_socket() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let router_upstream_websocket =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let mut upstream_websocket =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let registry = WebSocketRevocationRegistry::new();
    let selected_account = AccountId::new("acct_floor_reached")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let session = registry.register_cancellation_with_peer_addr(
        TokenGeneration::new(1),
        selected_account.clone(),
        None,
    );
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: selected_account.clone(),
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let notifier = WebSocketQuotaFloorNotifier::new(registry.clone());

    let router_task = forward_duplex_until_complete(
        router_local_websocket,
        router_upstream_websocket,
        WebSocketForwardingContext {
            session_registration: session,
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            affinity_owner_context: Some(&affinity_owner_context),
            provider_error_observer: None,
            account_admission_assessor: None,
            initial_turn_active: false,
            revocation: &revocation,
            session_shutdown: &session_shutdown,
        },
    );
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
            None => panic!("upstream should receive first frame"),
        }

        notifier.signal_weekly_quota_floor_reached(&selected_account);

        let client_message = tokio::time::timeout(Duration::from_secs(1), client_websocket.next())
            .await
            .unwrap_or_else(|_elapsed| panic!("client should promptly receive reconnect signal"))
            .unwrap_or_else(|| panic!("client should receive reconnect signal"))
            .unwrap_or_else(|error| panic!("client reconnect signal should be readable: {error}"));
        assert_eq!(client_message.to_string(), CODEX_WEBSOCKET_RECONNECT_SIGNAL);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should signal then close: {router_result:?}"
    );
    assert_eq!(registry.snapshot().quota_reconnect_signal_count, 1);
    assert_eq!(registry.snapshot().active_sessions, 0);
}
