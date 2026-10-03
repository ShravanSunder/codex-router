use super::*;

#[tokio::test]
async fn response_completed_releases_active_reservation_before_socket_closes() {
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
    let active_reservation_guard = ActiveReservationGuard::new_with_active_client_leases(
        active_reservations.clone(),
        "responses".to_owned(),
        reservation_handle,
        None,
    );
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
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
    assert_eq!(active_sessions(), 1);

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
                provider_error_observer: None,
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
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("completion should send: {error}"));
        match client_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("client should receive completion: {error}"),
            None => panic!("client should receive completion"),
        }

        tokio::time::timeout(Duration::from_secs(1), async {
            while active_sessions() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("response.completed should release active session"));

        client_websocket
            .send(Message::Ping(Bytes::from_static(b"still-open")))
            .await
            .unwrap_or_else(|error| panic!("socket should remain open after completion: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(Message::Ping(_))) => {}
            Some(Ok(message)) => {
                panic!("upstream should receive post-completion ping, got {message:?}")
            }
            Some(Err(error)) => panic!("post-completion ping should be valid: {error}"),
            None => panic!("upstream should receive post-completion ping"),
        }
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "completion release should not close the websocket by itself, got {router_result:?}"
    );
    assert_eq!(active_sessions(), 0);
}

#[tokio::test]
async fn same_socket_request_after_completion_reserves_pinned_account_again() {
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
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
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
                provider_error_observer: None,
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown,
            },
        )
        .await
    };
    let peer_task = async {
        assert_eq!(active_sessions(), 1);
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("first request should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first request: {error}"),
            None => panic!("upstream should receive first request"),
        }
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("first completion should send: {error}"));
        match client_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("client should receive first completion: {error}"),
            None => panic!("client should receive first completion"),
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while active_sessions() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("first completion should release active load"));

        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await
            .unwrap_or_else(|error| panic!("second request should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive second request: {error}"),
            None => panic!("upstream should receive second request"),
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while active_sessions() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("second request should re-reserve active session"));

        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":2}"#))
            .await
            .unwrap_or_else(|error| panic!("second completion should send: {error}"));
        match client_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("client should receive second completion: {error}"),
            None => panic!("client should receive second completion"),
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while active_sessions() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("second completion should release active load"));
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "same-socket re-reservation flow should complete: {router_result:?}"
    );
}

#[tokio::test]
async fn same_socket_immediate_request_after_completion_has_fresh_active_load() {
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
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
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
                provider_error_observer: None,
                account_admission_assessor: None,
                initial_turn_active: false,
                revocation: &revocation,
                session_shutdown: &session_shutdown,
            },
        )
        .await
    };
    let peer_task = async {
        assert_eq!(active_sessions(), 1);
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("first request should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive first request: {error}"),
            None => panic!("upstream should receive first request"),
        }
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .unwrap_or_else(|error| panic!("first completion should send: {error}"));
        match client_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("client should receive first completion: {error}"),
            None => panic!("client should receive first completion"),
        }
        assert_eq!(
            active_sessions(),
            0,
            "completion must release the active session synchronously before the next same-socket request"
        );

        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await
            .unwrap_or_else(|error| panic!("second request should send: {error}"));
        match upstream_websocket.next().await {
            Some(Ok(_message)) => {}
            Some(Err(error)) => panic!("upstream should receive second request: {error}"),
            None => panic!("upstream should receive second request"),
        }
        assert_eq!(
            active_sessions(),
            1,
            "second same-socket request should acquire a fresh active reservation"
        );
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "same-socket immediate re-reservation flow should complete: {router_result:?}"
    );
}
