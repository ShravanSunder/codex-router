use super::*;

#[tokio::test]
async fn upstream_model_capacity_frame_emits_retryable_wait_and_closes() {
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
    registry.set_capacity_retry_thread_id(session.session_id, Some("thread_1".to_owned()));
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();
    let provider_error_observer =
        Arc::new(RecordingAsyncProviderErrorObserver::with_selectable_alternative(false));
    let provider_error_observer_for_context: Arc<dyn AsyncProviderErrorObserver> =
        provider_error_observer.clone();
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: AccountId::new("acct_selected")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}")),
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };

    let router_task = forward_duplex_until_complete(
        router_local_websocket,
        router_upstream_websocket,
        WebSocketForwardingContext {
            session_registration: session,
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            affinity_owner_context: Some(&affinity_owner_context),
            provider_error_observer: Some(provider_error_observer_for_context),
            account_admission_assessor: None,
            initial_turn_active: false,
            revocation: &revocation,
            session_shutdown: &session_shutdown,
        },
    );
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .unwrap_or_else(|error| panic!("local frame should send: {error}"));
        let _first = upstream_websocket.next().await;
        let original = r#"{"type":"error","status":503,"error":{"code":"server_is_overloaded","message":"capacity"}}"#;
        upstream_websocket
            .send(Message::text(original))
            .await
            .unwrap_or_else(|error| panic!("capacity frame should send: {error}"));
        let received = client_websocket
            .next()
            .await
            .unwrap_or_else(|| panic!("client should receive capacity wait"))
            .unwrap_or_else(|error| panic!("client should receive capacity wait: {error}"));
        assert_eq!(
            received.to_string(),
            r#"{"type":"response.failed","response":{"id":"resp_router_model_capacity_wait","status":"failed","error":{"code":"rate_limit_exceeded","message":"Rate limit exceeded. Try again in 300 seconds."}}}"#
        );
    };
    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should send then close: {router_result:?}"
    );
    assert!(
        provider_error_observer.records().is_empty(),
        "model capacity must not reach the quota observer"
    );
}

#[tokio::test]
async fn exhausted_model_capacity_retry_forwards_original_error() {
    let registry = WebSocketRevocationRegistry::new();
    let session = registry.register_cancellation(TokenGeneration::new(1));
    registry.set_capacity_retry_thread_id(session.session_id, Some("thread_1".to_owned()));
    for _ in 0..10 {
        assert!(matches!(
            registry.record_capacity_retry(session.session_id),
            Some(CapacityRetryOutcome::Retry { .. })
        ));
    }
    let original = Message::text(
        r#"{"type":"error","status":503,"error":{"code":"server_is_overloaded","message":"capacity"}}"#,
    );
    let outcome = maybe_replace_account_quota_exhaustion_with_reconnect_signal(
        original.clone(),
        ProviderErrorClassification::ModelCapacity,
        None,
        &UpstreamToLocalPumpContext {
            revocation: session.cancellation().clone(),
            session_shutdown: CancellationToken::new(),
            tunnel_shutdown: CancellationToken::new(),
            session_registry: registry,
            session_id: session.session_id,
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            affinity_owner_context: None,
            active_turn_reservation: ActiveTurnReservationState::new(None),
            provider_error_observer: None,
            quota_floor_reconnect: CancellationToken::new(),
            early_floor_reconnect: session.early_floor_reconnect.clone(),
            graceful_floor_switch: session.graceful_floor_switch.clone(),
            account_turn_admission: AccountTurnAdmission::new(
                session.graceful_floor_switch.clone(),
                session.early_floor_reconnect.clone(),
                session.quota_floor_reconnect.clone(),
                Some(AccountId::new("acct_capacity_fixture").expect("fixture account id")),
                None,
                None,
                false,
            ),
        },
    )
    .await;
    assert!(!outcome.close_after_send);
    assert_eq!(outcome.message.to_string(), original.to_string());
}

#[test]
fn completed_capacity_retry_state_resets_for_a_reconnected_session() {
    let registry = WebSocketRevocationRegistry::new();
    let first = registry.register_cancellation(TokenGeneration::new(1));
    registry.set_capacity_retry_thread_id(first.session_id, Some("thread_1".to_owned()));
    for _ in 0..10 {
        let _retry = registry.record_capacity_retry(first.session_id);
    }
    registry.clear_capacity_retry(first.session_id);
    let reconnected = registry.register_cancellation(TokenGeneration::new(1));
    registry.set_capacity_retry_thread_id(reconnected.session_id, Some("thread_1".to_owned()));
    assert!(matches!(
        registry.record_capacity_retry(reconnected.session_id),
        Some(CapacityRetryOutcome::Retry { .. })
    ));
}

#[test]
fn capacity_retry_thread_id_requires_one_bounded_nonempty_header() {
    assert_eq!(
        capacity_retry_thread_id(&HeaderCollection::new(vec![Header::new(
            "thread-id",
            "one"
        )])),
        Some("one".to_owned())
    );
    for headers in [
        HeaderCollection::new(Vec::new()),
        HeaderCollection::new(vec![Header::new("thread-id", "")]),
        HeaderCollection::new(vec![
            Header::new("thread-id", "one"),
            Header::new("thread-id", "two"),
        ]),
        HeaderCollection::new(vec![Header::new(
            "thread-id",
            "x".repeat(MAX_THREAD_ID_BYTES + 1),
        )]),
    ] {
        assert_eq!(capacity_retry_thread_id(&headers), None);
    }
}
