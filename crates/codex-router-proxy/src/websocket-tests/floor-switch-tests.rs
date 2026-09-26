use super::*;

struct HeldSelectableFloorPeer {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

struct ImmediateSelectableFloorPeer;

#[tokio::test]
async fn floor_reconnect_supervisor_delivers_signal_after_local_pump_finishes_first() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_upstream =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let _upstream_peer =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let (mut local_write, local_read) = router_local.split();
    let (upstream_write, _upstream_read) = router_upstream.split();
    let (_intent_sender, intent) = watch::channel(FloorSwitchIntent::default());
    let hard_reconnect = CancellationToken::new();
    hard_reconnect.cancel();
    let early_reconnect = CancellationToken::new();
    let tunnel_shutdown = CancellationToken::new();
    let revocation = CancellationToken::new();
    let session_shutdown = CancellationToken::new();
    let floor_admission = FloorSwitchAdmission::new(
        intent,
        early_reconnect.clone(),
        hard_reconnect.clone(),
        None,
        None,
        false,
    );
    let (local_done_sender, local_done_receiver) = tokio::sync::oneshot::channel();
    let local_task = tokio::spawn(pump_local_to_upstream(
        local_read,
        upstream_write,
        LocalToUpstreamPumpContext {
            revocation: revocation.clone(),
            session_shutdown: session_shutdown.clone(),
            tunnel_shutdown: tunnel_shutdown.clone(),
            active_turn_reservation: ActiveTurnReservationState::new(None),
            session_affinity_activity_handle: None,
            floor_switch_admission: floor_admission,
            early_floor_reconnect: early_reconnect,
            quota_floor_reconnect: hard_reconnect,
        },
    ));
    let local_task = tokio::spawn(async move {
        let result = local_task.await.expect("local pump should join");
        let _ = local_done_sender.send(());
        result
    });
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        release_receiver
            .await
            .map_err(|_| WebSocketTunnelError::TaskJoin("signal release missing".to_owned()))?;
        local_write
            .send(Message::text(CODEX_WEBSOCKET_RECONNECT_SIGNAL))
            .await
            .map_err(WebSocketTunnelError::Transport)?;
        local_write
            .close()
            .await
            .map_err(WebSocketTunnelError::Transport)
    });
    let supervisor = supervise_websocket_pumps(
        &revocation,
        &session_shutdown,
        &tunnel_shutdown,
        local_task,
        upstream_task,
    );
    let observe_client = async {
        local_done_receiver
            .await
            .expect("local pump should finish before signal release");
        assert!(
            tunnel_shutdown.is_cancelled(),
            "local floor exit must retain the signal pump under supervision"
        );
        release_sender
            .send(())
            .expect("signal pump should remain live");
        let reconnect = client
            .next()
            .await
            .expect("client should receive reconnect frame")
            .expect("reconnect frame should decode");
        assert_eq!(reconnect.to_string(), CODEX_WEBSOCKET_RECONNECT_SIGNAL);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(supervisor, observe_client)
    })
    .await
    .expect("supervised floor reconnect should complete");
    assert!(result.is_ok(), "floor reconnect should finish: {result:?}");
}

impl LiveFloorSwitchPeerAssessor for ImmediateSelectableFloorPeer {
    fn assess_peer<'a>(
        &'a self,
        _source_account_id: &'a AccountId,
        _route_band: codex_router_core::routes::RouteBand,
    ) -> BoxFuture<'a, FloorSwitchPeerAssessment> {
        Box::pin(async { FloorSwitchPeerAssessment::SelectablePeer })
    }
}

#[tokio::test]
async fn initial_session_update_can_switch_before_first_response_create() {
    let account_id =
        AccountId::new("acct_precreate_floor").expect("fixture account id should parse");
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
    .with_affinity_secret_provider(&affinity_secret_provider)
    .with_floor_switch_peer_assessor(Arc::new(ImmediateSelectableFloorPeer));
    let upstream_listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("fixture upstream should bind");
    let upstream_url = format!(
        "ws://{}/v1/responses",
        upstream_listener.local_addr().expect("upstream address")
    );
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
            .send(Message::text(r#"{"type":"session.update"}"#))
            .await
            .expect("first control frame should send");
        let (stream, _) = upstream_listener
            .accept()
            .await
            .expect("upstream should accept");
        let mut upstream_websocket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upstream websocket should upgrade");
        let first = upstream_websocket
            .next()
            .await
            .expect("first frame should arrive")
            .expect("first frame should decode");
        assert_eq!(first.to_string(), r#"{"type":"session.update"}"#);
        notifier.request_weekly_quota_floor_switch(&account_id);
        client_websocket
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .expect("first create should queue");
        let reconnect = tokio::time::timeout(Duration::from_secs(1), client_websocket.next())
            .await
            .expect("idle socket should reconnect before first create")
            .expect("reconnect frame should exist")
            .expect("reconnect frame should decode");
        assert_eq!(reconnect.to_string(), CODEX_WEBSOCKET_RECONNECT_SIGNAL);
        let old_upstream = tokio::time::timeout(Duration::from_secs(1), upstream_websocket.next())
            .await
            .expect("old upstream should close");
        assert!(
            !matches!(old_upstream, Some(Ok(ref frame)) if frame.to_string().contains("response.create")),
            "first create must not reach the old account"
        );
    };
    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .expect("fixture should finish");
    assert!(
        router_result.is_ok(),
        "router should reconnect cleanly: {router_result:?}"
    );
    assert_eq!(registry.snapshot().quota_reconnect_signal_count, 1);
}

impl LiveFloorSwitchPeerAssessor for HeldSelectableFloorPeer {
    fn assess_peer<'a>(
        &'a self,
        _source_account_id: &'a AccountId,
        _route_band: codex_router_core::routes::RouteBand,
    ) -> BoxFuture<'a, FloorSwitchPeerAssessment> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            FloorSwitchPeerAssessment::SelectablePeer
        })
    }
}

#[derive(Clone, Copy)]
enum HeldSwitchOutcome {
    Switch,
    Clear,
    HardFloor,
}

#[tokio::test]
async fn held_floor_peer_check_blocks_next_create_and_reconnects_before_forwarding() {
    assert_held_floor_switch_socket_outcome(HeldSwitchOutcome::Switch).await;
}

#[tokio::test]
async fn cleared_floor_intent_during_held_peer_check_keeps_next_create_on_old_socket() {
    assert_held_floor_switch_socket_outcome(HeldSwitchOutcome::Clear).await;
}

#[tokio::test]
async fn hard_floor_preempts_held_graceful_peer_check_without_waiting_for_peer() {
    assert_held_floor_switch_socket_outcome(HeldSwitchOutcome::HardFloor).await;
}

async fn assert_held_floor_switch_socket_outcome(outcome: HeldSwitchOutcome) {
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
    let selected_account = AccountId::new("acct_held_floor")
        .unwrap_or_else(|error| panic!("fixture account should parse: {error}"));
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
    .unwrap_or_else(|error| panic!("fixture affinity secret should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: selected_account.clone(),
        credential_generation: 1,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let notifier = WebSocketQuotaFloorNotifier::new(registry.clone());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let peer_assessor = Arc::new(HeldSelectableFloorPeer {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    });
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
            floor_switch_peer_assessor: Some(peer_assessor),
            initial_turn_active: false,
            revocation: &revocation,
            session_shutdown: &session_shutdown,
        },
    );
    let peer_task = async {
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .expect("first turn should send");
        let first = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
            .await
            .expect("first turn should reach upstream")
            .expect("first upstream frame should exist")
            .expect("first upstream frame should decode");
        assert_eq!(first.to_string(), r#"{"type":"response.create","turn":1}"#);
        notifier.request_weekly_quota_floor_switch(&selected_account);
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed"}"#))
            .await
            .expect("terminal turn should send");
        let completed = tokio::time::timeout(Duration::from_secs(2), client_websocket.next())
            .await
            .expect("terminal turn should reach client")
            .expect("terminal frame should exist")
            .expect("terminal frame should decode");
        assert_eq!(completed.to_string(), r#"{"type":"response.completed"}"#);
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("live peer check should begin");
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await
            .expect("next turn should queue while peer check is held");
        match outcome {
            HeldSwitchOutcome::Switch => release.notify_one(),
            HeldSwitchOutcome::Clear => {
                notifier.clear_weekly_quota_floor_switch(&selected_account);
                release.notify_one();
            }
            HeldSwitchOutcome::HardFloor => {
                notifier.signal_weekly_quota_floor_reached(&selected_account);
            }
        }
        if matches!(outcome, HeldSwitchOutcome::Clear) {
            let next = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
                .await
                .expect("cleared next turn should reach old upstream")
                .expect("next upstream frame should exist")
                .expect("next upstream frame should decode");
            assert_eq!(next.to_string(), r#"{"type":"response.create","turn":2}"#);
            upstream_websocket
                .send(Message::text(r#"{"type":"response.completed"}"#))
                .await
                .expect("second terminal turn should send");
            let second = tokio::time::timeout(Duration::from_secs(2), client_websocket.next())
                .await
                .expect("second terminal turn should reach client")
                .expect("second terminal frame should exist")
                .expect("second terminal frame should decode");
            assert_eq!(second.to_string(), r#"{"type":"response.completed"}"#);
        } else {
            let reconnect = tokio::time::timeout(Duration::from_secs(2), client_websocket.next())
                .await
                .expect("reconnect should arrive")
                .expect("reconnect frame should exist")
                .expect("reconnect frame should decode");
            assert_eq!(reconnect.to_string(), CODEX_WEBSOCKET_RECONNECT_SIGNAL);
            let old_upstream =
                tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
                    .await
                    .expect("old upstream should close after reconnect");
            assert!(
                !matches!(old_upstream, Some(Ok(ref frame)) if frame.to_string().contains("response.create")),
                "queued next turn must not forward to the old account"
            );
        }
        drop(client_websocket);
        drop(upstream_websocket);
    };
    let (router_result, ()) = tokio::join!(router_task, peer_task);
    assert!(
        router_result.is_ok(),
        "router should close cleanly: {router_result:?}"
    );
}
