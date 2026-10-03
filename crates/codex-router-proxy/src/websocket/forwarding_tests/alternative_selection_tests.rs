use super::*;

#[tokio::test]
async fn upstream_usage_limit_frame_emits_state_unavailable_when_alternative_selection_stalls() {
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
    let provider_error_observer = Arc::new(PendingAlternativeSelectionProviderErrorObserver);
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
        let client_message =
            tokio::time::timeout(Duration::from_millis(400), client_websocket.next())
                .await
                .unwrap_or_else(|_elapsed| {
                    panic!(
                        "state-unavailable signal should not wait forever on alternative selection"
                    )
                });
        let client_message = match client_message {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive state-unavailable: {error}"),
            None => panic!("client should receive state-unavailable"),
        };
        assert_eq!(
            client_message.to_string(),
            super::ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL
        );
        assert!(
            !client_message.to_string().contains("usage_limit_reached"),
            "stalled post-exhaustion selection must not leak provider wording"
        );
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should close/retire exhausted tunnel after state-unavailable signal, got {router_result:?}"
    );
}

#[tokio::test]
async fn upstream_usage_limit_frame_emits_reconnect_when_runtime_quarantine_leaves_selectable_alternative()
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
    let provider_error_observer =
        Arc::new(RecordingAsyncProviderErrorObserver::with_selectable_alternative(true));
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
            "single-account quota exhaustion must not leak provider wording to Codex"
        );
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should emit reconnect signal when an alternative can rotate, got {router_result:?}"
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
async fn upstream_usage_limit_frame_emits_all_exhausted_when_runtime_quarantine_leaves_no_selectable_alternative()
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
    let provider_error_observer =
        Arc::new(RecordingAsyncProviderErrorObserver::with_selectable_alternative(false));
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
        let client_message = match client_websocket.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => panic!("client should receive all-exhausted signal: {error}"),
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
            "last-account quota exhaustion must use Codex's usage-limit error shape"
        );
        assert!(
            client_message
                .to_string()
                .contains("codex_router_all_accounts_exhausted"),
            "last-account quota exhaustion should preserve the router-specific code"
        );
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should emit all-exhausted after runtime quarantine when no account can rotate, got {router_result:?}"
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
async fn upstream_usage_limit_frame_emits_retryable_wait_when_every_alternative_is_short_exhausted()
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
    let provider_error_observer = Arc::new(
        RecordingAsyncProviderErrorObserver::with_post_exhaustion_outcome(
            PostExhaustionRouteBandOutcome::ShortQuotaWait {
                retry_after_seconds: 2,
            },
        ),
    );
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
                provider_error_observer: Some(provider_error_observer),
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
        let _first_upstream_frame = upstream_websocket
            .next()
            .await
            .unwrap_or_else(|| panic!("upstream should receive first frame"))
            .unwrap_or_else(|error| panic!("upstream should receive first frame: {error}"));
        upstream_websocket
            .send(Message::text(usage_limit_frame))
            .await
            .unwrap_or_else(|error| panic!("usage limit should send: {error}"));
        let client_message = client_websocket
            .next()
            .await
            .unwrap_or_else(|| panic!("client should receive short quota wait"))
            .unwrap_or_else(|error| panic!("client should receive short quota wait: {error}"));
        assert_eq!(
            client_message.to_string(),
            r#"{"type":"response.failed","response":{"id":"resp_router_short_quota_wait","status":"failed","error":{"code":"rate_limit_exceeded","message":"Rate limit exceeded. Try again in 2 seconds."}}}"#,
        );
        drop(upstream_websocket);
    };

    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should send a retryable short-quota wait after provider exhaustion, got {router_result:?}"
    );
}
