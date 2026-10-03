use super::*;

#[tokio::test]
async fn upstream_usage_limit_frame_emits_state_unavailable_when_queue_is_degraded() {
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
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let selected_account = AccountId::new("acct_selected")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: selected_account,
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let usage_limit_frame =
        r#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#;

    let router_task = async {
        forward_duplex_until_complete(
            router_local_websocket,
            router_upstream_websocket,
            WebSocketForwardingContext {
                session_registration: session,
                affinity_owner_recorder: None,
                async_affinity_owner_recorder: None,
                affinity_record_tasks: TaskTracker::new(),
                affinity_owner_context: Some(&affinity_owner_context),
                provider_error_observer: Some(Arc::new(FailingAsyncProviderErrorObserver)),
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown,
            },
        )
        .await
    };
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("local frame should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
            None => panic!("upstream should receive first frame"),
        }
        upstream_websocket
            .send(Message::text(usage_limit_frame))
            .await
            .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
        let client_message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => {
                panic!("client should receive state-unavailable signal: {error}")
            }
            None => panic!("client should receive state-unavailable signal"),
        };
        assert_eq!(
            client_message.to_string(),
            super::ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL
        );
        assert!(!client_message.to_string().contains("usage_limit_reached"));
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should fail closed when durable queue health is degraded, got {router_result:?}"
    );
}

#[tokio::test]
async fn upstream_usage_limit_frame_emits_state_unavailable_when_alternative_selection_is_unavailable()
 {
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
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let selected_account = AccountId::new("acct_selected")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: selected_account,
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let usage_limit_frame =
        r#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#;

    let router_task = async {
        forward_duplex_until_complete(
            router_local_websocket,
            router_upstream_websocket,
            WebSocketForwardingContext {
                session_registration: session,
                affinity_owner_recorder: None,
                async_affinity_owner_recorder: None,
                affinity_record_tasks: TaskTracker::new(),
                affinity_owner_context: Some(&affinity_owner_context),
                provider_error_observer: Some(Arc::new(
                    DefaultAlternativeSelectionProviderErrorObserver,
                )),
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown,
            },
        )
        .await
    };
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("local frame should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
            None => panic!("upstream should receive first frame"),
        }
        upstream_websocket
            .send(Message::text(usage_limit_frame))
            .await
            .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
        let client_message =
            match tokio::time::timeout(Duration::from_millis(250), client_websocket.next())
                .await
                .unwrap_or_else(|_elapsed| {
                    panic!("state-unavailable signal should arrive after runtime quarantine")
                }) {
                Some(Ok(message)) => message,
                Some(Err(error)) => {
                    panic!("client should receive state-unavailable signal: {error}")
                }
                None => panic!("client should receive state-unavailable signal"),
            };
        assert_eq!(
            client_message.to_string(),
            super::ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL
        );
        assert!(!client_message.to_string().contains("usage_limit_reached"));
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should emit state-unavailable when alternative selection is unavailable, got {router_result:?}"
    );
}

#[tokio::test]
async fn upstream_usage_limit_frame_emits_state_unavailable_when_observer_uses_default_alternative_selection()
 {
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
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let selected_account = AccountId::new("acct_selected")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: selected_account,
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let usage_limit_frame =
        r#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#;

    let router_task = async {
        forward_duplex_until_complete(
            router_local_websocket,
            router_upstream_websocket,
            WebSocketForwardingContext {
                session_registration: session,
                affinity_owner_recorder: None,
                async_affinity_owner_recorder: None,
                affinity_record_tasks: TaskTracker::new(),
                affinity_owner_context: Some(&affinity_owner_context),
                provider_error_observer: Some(Arc::new(
                    DefaultAlternativeSelectionProviderErrorObserver,
                )),
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown,
            },
        )
        .await
    };
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("local frame should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
            None => panic!("upstream should receive first frame"),
        }
        upstream_websocket
            .send(Message::text(usage_limit_frame))
            .await
            .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
        let client_message =
            match tokio::time::timeout(Duration::from_millis(250), client_websocket.next())
                .await
                .unwrap_or_else(|_elapsed| {
                    panic!(
                        "state-unavailable signal should arrive for implicit alternative selection"
                    )
                }) {
                Some(Ok(message)) => message,
                Some(Err(error)) => {
                    panic!("client should receive state-unavailable signal: {error}")
                }
                None => panic!("client should receive state-unavailable signal"),
            };
        assert_eq!(
            client_message.to_string(),
            super::ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL
        );
        assert!(!client_message.to_string().contains("usage_limit_reached"));
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should emit state-unavailable without implicit alternative selection proof, got {router_result:?}"
    );
}

#[tokio::test]
async fn upstream_websocket_connection_limit_frame_is_forwarded_unchanged_and_observed() {
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
    let provider_error_observer = Arc::new(RecordingAsyncProviderErrorObserver::default());
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let selected_account = AccountId::new("acct_selected")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: selected_account.clone(),
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let connection_limit_frame = r#"{"type":"error","status":400,"error":{"type":"invalid_request_error","code":"websocket_connection_limit_reached","message":"Responses websocket connection limit reached"}}"#;

    let router_task = async {
        forward_duplex_until_complete(
            router_local_websocket,
            router_upstream_websocket,
            WebSocketForwardingContext {
                session_registration: session,
                affinity_owner_recorder: None,
                async_affinity_owner_recorder: None,
                affinity_record_tasks: TaskTracker::new(),
                affinity_owner_context: Some(&affinity_owner_context),
                provider_error_observer: Some(provider_error_observer.clone()),
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown,
            },
        )
        .await
    };
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("local frame should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
            None => panic!("upstream should receive first frame"),
        }
        upstream_websocket
            .send(Message::text(connection_limit_frame))
            .await
            .unwrap_or_else(|error| panic!("connection-limit should send: {error}"));
        let client_message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive connection-limit frame: {error}"),
            None => panic!("client should receive connection-limit frame"),
        };
        assert_eq!(client_message.to_string(), connection_limit_frame);
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should keep connection-limit as pass-through frame, got {router_result:?}"
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        provider_error_observer.observed.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("connection-limit should be observed"));
    assert_eq!(
        provider_error_observer.records(),
        vec![RecordedProviderError {
            account_id: selected_account,
            route_band: codex_router_core::routes::RouteBand::Responses,
            classification: ProviderErrorClassification::WebSocketConnectionLimit,
        }]
    );
}
