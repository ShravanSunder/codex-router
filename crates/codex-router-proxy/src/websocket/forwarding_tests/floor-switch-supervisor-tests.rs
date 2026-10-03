use super::floor_switch_tests::{HeldSelectableFloorPeer, ImmediateSelectableFloorPeer};
use super::*;

#[derive(Clone, Copy)]
enum LocalFloorExit {
    PreCancelledHard,
    EarlyDecision,
    HardDuringPeerAssessment,
}

#[tokio::test]
async fn floor_reconnect_supervisor_delivers_signal_after_local_pump_finishes_first() {
    assert_local_floor_exit_delivers_signal(LocalFloorExit::PreCancelledHard).await;
}

#[tokio::test]
async fn early_floor_decision_delivers_signal_after_local_pump_finishes_first() {
    assert_local_floor_exit_delivers_signal(LocalFloorExit::EarlyDecision).await;
}

#[tokio::test]
async fn hard_floor_during_peer_assessment_delivers_signal_after_local_pump_finishes_first() {
    assert_local_floor_exit_delivers_signal(LocalFloorExit::HardDuringPeerAssessment).await;
}

async fn assert_local_floor_exit_delivers_signal(exit: LocalFloorExit) {
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
    let pending = !matches!(exit, LocalFloorExit::PreCancelledHard);
    let (_intent_sender, intent) = watch::channel(FloorSwitchIntent {
        epoch: u64::from(pending),
        pending,
    });
    let hard_reconnect = CancellationToken::new();
    if matches!(exit, LocalFloorExit::PreCancelledHard) {
        hard_reconnect.cancel();
    }
    let hard_for_observation = hard_reconnect.clone();
    let early_reconnect = CancellationToken::new();
    let tunnel_shutdown = CancellationToken::new();
    let revocation = CancellationToken::new();
    let session_shutdown = CancellationToken::new();
    let peer_entered = Arc::new(Notify::new());
    let peer_release = Arc::new(Notify::new());
    let peer_assessor: Option<Arc<dyn LiveAccountAdmissionAssessor>> = match exit {
        LocalFloorExit::PreCancelledHard => None,
        LocalFloorExit::EarlyDecision => Some(Arc::new(ImmediateSelectableFloorPeer)),
        LocalFloorExit::HardDuringPeerAssessment => Some(Arc::new(HeldSelectableFloorPeer {
            entered: Arc::clone(&peer_entered),
            release: peer_release,
        })),
    };
    let floor_admission = AccountTurnAdmission::new(
        intent,
        early_reconnect.clone(),
        hard_reconnect.clone(),
        pending.then(|| AccountId::new("acct_local_floor_exit").expect("fixture account id")),
        None,
        peer_assessor,
        false,
    );
    if pending {
        client
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .expect("next create should queue at the local pump");
    }
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
            account_turn_admission: floor_admission,
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
        if matches!(exit, LocalFloorExit::HardDuringPeerAssessment) {
            peer_entered.notified().await;
            hard_for_observation.cancel();
        }
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
