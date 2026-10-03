use super::*;

#[tokio::test]
async fn exact_forwarded_response_create_renews_session_affinity() {
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
    let cache = SessionAccountAffinityCache::shared(
        crate::session_account_affinity_cache::DEFAULT_SESSION_PIN_IDLE_TTL,
    );
    let account_id = AccountId::new("acct-affinity-activity")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let published = publish_session_account_affinity(
        &cache,
        Provider::Openai,
        "session-activity",
        &account_id,
        codex_router_core::routes::RouteBand::Responses,
        None,
        0,
    )
    .unwrap_or_else(|_| panic!("test affinity publication should succeed"));
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext::new(affinity_secret, account_id, 1)
        .with_session_affinity_activity_handle(Some(published.activity_handle().clone()));
    let registry = WebSocketRevocationRegistry::new();
    let session = registry.register_cancellation(TokenGeneration::new(1));
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();

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
            .send(Message::text(r#"{"type":"conversation.item.create"}"#))
            .await
            .unwrap_or_else(|error| panic!("unrelated frame should send: {error}"));
        let unrelated = upstream_websocket
            .next()
            .await
            .unwrap_or_else(|| panic!("upstream should receive unrelated frame"))
            .unwrap_or_else(|error| panic!("unrelated frame should forward: {error}"));
        assert_eq!(
            unrelated,
            Message::text(r#"{"type":"conversation.item.create"}"#)
        );
        assert!(
            lookup_session_account_affinity(
                &cache,
                Provider::Openai,
                "session-activity",
                codex_router_core::routes::RouteBand::Responses,
                None,
                current_unix_seconds(),
            )
            .unwrap_or_else(|_| panic!("lookup should succeed"))
            .is_none(),
            "unrelated forwarded traffic must not renew an expired entry"
        );

        let response_create = Message::text(r#"{"type":"response.create","turn":1}"#);
        client_websocket
            .send(response_create.clone())
            .await
            .unwrap_or_else(|error| panic!("response.create should send: {error}"));
        let forwarded = upstream_websocket
            .next()
            .await
            .unwrap_or_else(|| panic!("upstream should receive response.create"))
            .unwrap_or_else(|error| panic!("response.create should forward: {error}"));
        assert_eq!(
            forwarded, response_create,
            "forwarded bytes must remain unchanged"
        );
        assert!(
            lookup_session_account_affinity(
                &cache,
                Provider::Openai,
                "session-activity",
                codex_router_core::routes::RouteBand::Responses,
                None,
                current_unix_seconds(),
            )
            .unwrap_or_else(|_| panic!("lookup should succeed"))
            .is_some(),
            "successful exact response.create forwarding should renew affinity"
        );
        client_websocket
            .close(None)
            .await
            .unwrap_or_else(|error| panic!("client should close cleanly: {error}"));
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "forwarding should succeed: {router_result:?}"
    );
}

#[tokio::test]
async fn large_and_malformed_suffix_response_create_frames_renew_session_affinity() {
    let valid_large = Message::text(format!(
        r#"{{"type":"response.create","input":"{}"}}"#,
        "x".repeat(70 * 1024)
    ));
    assert_forwarded_recognized_frame_renews(
        valid_large,
        "session-large-response-create",
        "acct-large-response-create",
    )
    .await;

    let malformed_suffix = Message::text(format!(
        r#"{{"type":"response.create","input":"{}" malformed"#,
        "x".repeat(70 * 1024)
    ));
    assert_forwarded_recognized_frame_renews(
        malformed_suffix,
        "session-malformed-response-create",
        "acct-malformed-response-create",
    )
    .await;
}

async fn assert_forwarded_recognized_frame_renews(
    frame: Message,
    session_id: &str,
    account_id: &str,
) {
    let (router_local_stream, client_stream) = duplex(160 * 1024);
    let (router_upstream_stream, upstream_stream) = duplex(160 * 1024);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let router_upstream_websocket =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let mut upstream_websocket =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let cache = SessionAccountAffinityCache::shared(
        crate::session_account_affinity_cache::DEFAULT_SESSION_PIN_IDLE_TTL,
    );
    let account_id = AccountId::new(account_id)
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let published = publish_session_account_affinity(
        &cache,
        Provider::Openai,
        session_id,
        &account_id,
        codex_router_core::routes::RouteBand::Responses,
        None,
        0,
    )
    .unwrap_or_else(|_| panic!("test affinity publication should succeed"));
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext::new(affinity_secret, account_id, 1)
        .with_session_affinity_activity_handle(Some(published.activity_handle().clone()));
    let registry = WebSocketRevocationRegistry::new();
    let session = registry.register_cancellation(TokenGeneration::new(1));
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();

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
    let expected_frame = frame.clone();
    let peer_task = async {
        client_websocket
            .send(frame)
            .await
            .unwrap_or_else(|error| panic!("recognized frame should send: {error}"));
        let forwarded = upstream_websocket
            .next()
            .await
            .unwrap_or_else(|| panic!("upstream should receive recognized frame"))
            .unwrap_or_else(|error| panic!("recognized frame should forward: {error}"));
        assert_eq!(
            forwarded, expected_frame,
            "forwarded bytes must remain unchanged"
        );
        assert!(
            lookup_session_account_affinity(
                &cache,
                Provider::Openai,
                session_id,
                codex_router_core::routes::RouteBand::Responses,
                None,
                current_unix_seconds(),
            )
            .unwrap_or_else(|_| panic!("lookup should succeed"))
            .is_some(),
            "successfully forwarded recognized activity should renew affinity"
        );
        client_websocket
            .close(None)
            .await
            .unwrap_or_else(|error| panic!("client should close cleanly: {error}"));
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "forwarding should succeed: {router_result:?}"
    );
}

#[tokio::test]
async fn failed_response_create_forward_does_not_renew_session_affinity() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let router_upstream_websocket =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let upstream_websocket =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let (upstream_write, _upstream_read) = router_upstream_websocket.split();
    let (_local_write, local_read) = router_local_websocket.split();
    drop(upstream_websocket);

    let cache = SessionAccountAffinityCache::shared(
        crate::session_account_affinity_cache::DEFAULT_SESSION_PIN_IDLE_TTL,
    );
    let account_id = AccountId::new("acct-failed-forward")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let published = publish_session_account_affinity(
        &cache,
        Provider::Openai,
        "session-failed-forward",
        &account_id,
        codex_router_core::routes::RouteBand::Responses,
        None,
        0,
    )
    .unwrap_or_else(|_| panic!("test affinity publication should succeed"));
    client_websocket
        .send(Message::text(r#"{"type":"response.create"}"#))
        .await
        .unwrap_or_else(|error| panic!("client frame should enter local socket: {error}"));

    let (_floor_intent_sender, floor_intent) = watch::channel(FloorSwitchIntent::default());
    let early_reconnect = CancellationToken::new();
    let hard_reconnect = CancellationToken::new();
    let floor_admission = AccountTurnAdmission::new(
        floor_intent,
        early_reconnect.clone(),
        hard_reconnect.clone(),
        Some(AccountId::new("acct_forward_test").expect("fixture account id")),
        None,
        None,
        false,
    );

    let result = pump_local_to_upstream(
        local_read,
        upstream_write,
        LocalToUpstreamPumpContext {
            revocation: CancellationToken::new(),
            session_shutdown: CancellationToken::new(),
            tunnel_shutdown: CancellationToken::new(),
            active_turn_reservation: ActiveTurnReservationState::new(None),
            session_affinity_activity_handle: Some(published.activity_handle().clone()),
            account_turn_admission: floor_admission,
            early_floor_reconnect: early_reconnect,
            quota_floor_reconnect: hard_reconnect,
        },
    )
    .await;

    assert!(
        result.is_err(),
        "closed upstream must fail the forwarding send"
    );
    assert!(
        lookup_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-failed-forward",
            codex_router_core::routes::RouteBand::Responses,
            None,
            current_unix_seconds(),
        )
        .unwrap_or_else(|_| panic!("lookup should succeed"))
        .is_none(),
        "failed forwarding must not renew the expired affinity"
    );
}
