use super::*;

#[tokio::test]
async fn quota_reconnect_signal_stops_new_upstream_work_before_persistence_completes() {
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
    let provider_error_observer = Arc::new(BlockingAsyncProviderErrorObserver::default());
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
            .unwrap_or_else(|error| panic!("first local frame should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first frame: {error}"),
            None => panic!("upstream should receive first frame"),
        }
        upstream_websocket
            .send(Message::text(usage_limit_frame))
            .await
            .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
        tokio::time::timeout(
            Duration::from_secs(1),
            provider_error_observer.observed.notified(),
        )
        .await
        .unwrap_or_else(|_elapsed| panic!("quota observer should be entered"));
        let client_message =
            match tokio::time::timeout(Duration::from_millis(100), client_websocket.next())
                .await
                .unwrap_or_else(|_elapsed| {
                    panic!("reconnect signal should arrive before durable persistence completes")
                }) {
                Some(Ok(message)) => message,
                Some(Err(error)) => panic!("client should receive reconnect signal: {error}"),
                None => panic!("client should receive reconnect signal"),
            };
        assert_eq!(
            client_message.to_string(),
            super::CODEX_WEBSOCKET_RECONNECT_SIGNAL
        );
        if client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await
            .is_ok()
            && let Ok(Some(Ok(message))) =
                tokio::time::timeout(Duration::from_millis(100), upstream_websocket.next()).await
        {
            assert!(
                !message.to_string().contains(r#""turn":2"#),
                "second turn must not reach exhausted upstream while quota observation is pending"
            );
        }
        provider_error_observer.release_observation();
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should stop exhausted upstream work before persistence completes: {router_result:?}"
    );
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
async fn quota_reconnect_signal_does_not_wait_for_provider_error_persistence() {
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
    let provider_error_observer = Arc::new(BlockingAsyncProviderErrorObserver::default());
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
        tokio::time::timeout(
            Duration::from_secs(1),
            provider_error_observer.observed.notified(),
        )
        .await
        .unwrap_or_else(|_elapsed| panic!("quota observer should be entered"));

        let client_message =
            tokio::time::timeout(Duration::from_millis(100), client_websocket.next())
                .await
                .unwrap_or_else(|_elapsed| {
                    panic!("reconnect signal should not wait for provider error persistence")
                });
        let client_message = match client_message {
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
        provider_error_observer.release_observation();
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should emit reconnect signal before durable quota observation completes: {router_result:?}"
    );
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
async fn quota_exhaustion_emits_all_exhausted_when_projection_has_no_alternative() {
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
    let provider_error_observer =
        Arc::new(BlockingAsyncProviderErrorObserver::with_selectable_alternative(false));
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
        tokio::time::timeout(
            Duration::from_secs(1),
            provider_error_observer.observed.notified(),
        )
        .await
        .unwrap_or_else(|_elapsed| panic!("quota observer should be entered"));

        let client_message =
            tokio::time::timeout(Duration::from_millis(100), client_websocket.next())
                .await
                .unwrap_or_else(|_elapsed| {
                    panic!("all-exhausted signal should arrive after runtime quarantine")
                });
        let client_message = match client_message {
            Some(Ok(message)) => message,
            Some(Err(error)) => {
                panic!("client should receive all-exhausted signal: {error}")
            }
            None => panic!("client should receive all-exhausted signal"),
        };
        assert_eq!(
            client_message.to_string(),
            super::ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL
        );
        assert_ne!(
            client_message.to_string(),
            super::CODEX_WEBSOCKET_RECONNECT_SIGNAL,
            "last-account quota exhaustion must not ask Codex to reconnect when no alternative can be selected"
        );
        assert!(
            client_message.to_string().contains("usage_limit_reached"),
            "single-account quota exhaustion must use Codex's usage-limit error shape"
        );
        assert!(
            client_message
                .to_string()
                .contains("codex_router_all_accounts_exhausted"),
            "single-account quota exhaustion should preserve the router-specific code"
        );
        provider_error_observer.release_observation();
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should emit all-exhausted signal after runtime quarantine: {router_result:?}"
    );
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
async fn quota_reconnect_signal_closes_old_socket_before_more_work() {
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

        let second_write_result = client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await;
        if second_write_result.is_ok() {
            let forwarded_after_reconnect =
                tokio::time::timeout(Duration::from_millis(100), upstream_websocket.next()).await;
            assert!(
                forwarded_after_reconnect.is_err(),
                "old exhausted tunnel must not forward another request after reconnect signal"
            );
        }
        assert_eq!(
            active_sessions(),
            0,
            "old exhausted tunnel must not reacquire active session after reconnect signal"
        );
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should close/retire exhausted tunnel after reconnect signal, got {router_result:?}"
    );
}
