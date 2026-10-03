use super::*;

#[tokio::test]
async fn upstream_usage_limit_frame_is_hidden_behind_codex_reconnect_signal() {
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
    let active_reservations = RouteBandReservationBooks::default();
    let reservation_handle = {
        let mut reservations = active_reservations
            .lock()
            .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
        reservations
            .entry("responses".to_owned())
            .or_insert_with(ReservationBook::default)
            .reserve_next_at(selected_account.clone(), 1, 1)
    };
    let active_reservation_guard = ActiveReservationGuard::new(
        active_reservations.clone(),
        "responses".to_owned(),
        reservation_handle,
    );
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: selected_account.clone(),
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: Some(active_reservation_guard),
        session_affinity_activity_handle: None,
    };
    let active_sessions = || {
        let reservations = active_reservations
            .lock()
            .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
        reservations
            .get("responses")
            .map_or(0, |book| book.active_session_count(&selected_account))
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
            .send(Message::text(usage_limit_frame))
            .await
            .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
        let client_message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive reconnect signal: {error}"),
            None => panic!("client should receive reconnect signal"),
        };
        assert_eq!(
            client_message.to_string(),
            super::CODEX_WEBSOCKET_RECONNECT_SIGNAL
        );
        assert!(
            !client_message.to_string().contains("usage_limit_reached"),
            "single-account quota exhaustion must not leak to Codex while router can rotate"
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while active_sessions() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("quota replacement should release exhausted account active session")
        });
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should hide provider quota exhaustion behind reconnect signal, got {router_result:?}"
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        provider_error_observer.observed.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("provider error should be observed"));
    assert_eq!(
        provider_error_observer.records(),
        vec![RecordedProviderError {
            account_id: selected_account,
            route_band: codex_router_core::routes::RouteBand::Responses,
            classification: ProviderErrorClassification::AccountQuotaExhausted,
        }]
    );
}

#[tokio::test]
async fn oversized_upstream_usage_limit_error_frame_is_hidden_behind_codex_reconnect_signal() {
    let (router_local_stream, client_stream) = duplex(256 * 1024);
    let (router_upstream_stream, upstream_stream) = duplex(256 * 1024);
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
    let padding = "x".repeat(128 * 1024);
    let usage_limit_frame = format!(
        r#"{{"type":"error","error":{{"type":"usage_limit_reached","code":"usage_limit_reached","message":"{padding}"}}}}"#
    );

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
            .send(Message::text(usage_limit_frame.clone()))
            .await
            .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
        let client_message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive reconnect signal: {error}"),
            None => panic!("client should receive reconnect signal"),
        };
        assert_eq!(
            client_message.to_string(),
            super::CODEX_WEBSOCKET_RECONNECT_SIGNAL,
            "valid oversized quota error envelopes must be hidden behind reconnect signal"
        );
        assert!(
            !client_message.to_string().contains("usage_limit_reached"),
            "single-account quota exhaustion must not leak to Codex while router can rotate"
        );
        drop(upstream_websocket);
        usage_limit_frame
    };

    let (router_result, usage_limit_frame) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should hide oversized provider quota exhaustion behind reconnect signal, got {router_result:?}"
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        provider_error_observer.observed.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("provider error should be observed"));
    assert_eq!(
        provider_error_observer.records(),
        vec![RecordedProviderError {
            account_id: selected_account,
            route_band: codex_router_core::routes::RouteBand::Responses,
            classification: ProviderErrorClassification::AccountQuotaExhausted,
        }]
    );
    assert!(usage_limit_frame.contains("usage_limit_reached"));
}

#[tokio::test]
async fn oversized_spaced_usage_limit_error_frame_is_hidden_behind_codex_reconnect_signal() {
    let (router_local_stream, client_stream) = duplex(256 * 1024);
    let (router_upstream_stream, upstream_stream) = duplex(256 * 1024);
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
    let padding = "x".repeat(128 * 1024);
    let usage_limit_frame = format!(
        r#"{{
                "type" : "error",
                "error" :
                {{
                    "type" : "usage_limit_reached",
                    "code" : "usage_limit_reached",
                    "message" : "{padding}"
                }}
            }}"#
    );

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
            .send(Message::text(usage_limit_frame.clone()))
            .await
            .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
        let client_message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive reconnect signal: {error}"),
            None => panic!("client should receive reconnect signal"),
        };
        assert_eq!(
            client_message.to_string(),
            super::CODEX_WEBSOCKET_RECONNECT_SIGNAL,
            "valid oversized quota error envelopes with JSON whitespace must be hidden behind reconnect signal"
        );
        assert!(
            !client_message.to_string().contains("usage_limit_reached"),
            "single-account quota exhaustion must not leak to Codex while router can rotate"
        );
        drop(upstream_websocket);
        usage_limit_frame
    };

    let (router_result, usage_limit_frame) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should hide spaced oversized provider quota exhaustion behind reconnect signal, got {router_result:?}"
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        provider_error_observer.observed.notified(),
    )
    .await
    .unwrap_or_else(|_elapsed| panic!("provider error should be observed"));
    assert_eq!(
        provider_error_observer.records(),
        vec![RecordedProviderError {
            account_id: selected_account,
            route_band: codex_router_core::routes::RouteBand::Responses,
            classification: ProviderErrorClassification::AccountQuotaExhausted,
        }]
    );
    assert!(usage_limit_frame.contains("usage_limit_reached"));
}

#[tokio::test]
async fn oversized_non_error_quota_text_frame_is_forwarded_without_quota_policy() {
    let (router_local_stream, client_stream) = duplex(256 * 1024);
    let (router_upstream_stream, upstream_stream) = duplex(256 * 1024);
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
        account_id: selected_account,
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let padding = "x".repeat(128 * 1024);
    let model_text_frame = format!(
        r#"{{"type":"response.output_text.delta","delta":"usage_limit_reached is model text {padding}"}}"#
    );

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
            .send(Message::text(model_text_frame.clone()))
            .await
            .unwrap_or_else(|error| panic!("model text should send: {error}"));
        let client_message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive model text: {error}"),
            None => panic!("client should receive model text"),
        };
        assert_eq!(
            client_message.to_string(),
            model_text_frame,
            "oversized non-error WebSocket text remains Codex-owned payload"
        );
        drop(upstream_websocket);
        model_text_frame
    };

    let (router_result, model_text_frame) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should pass oversized non-error WebSocket text through, got {router_result:?}"
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(25),
            provider_error_observer.observed.notified(),
        )
        .await
        .is_err(),
        "oversized non-error WebSocket text must not be observed as provider quota"
    );
    assert_eq!(
        provider_error_observer.records(),
        Vec::<RecordedProviderError>::new(),
        "oversized non-error WebSocket text must not mark account quota"
    );
    assert!(model_text_frame.contains("usage_limit_reached"));
}
