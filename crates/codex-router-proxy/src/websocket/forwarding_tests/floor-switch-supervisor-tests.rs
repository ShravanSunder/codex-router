use super::floor_switch_tests::{HeldSelectableFloorPeer, ImmediateSelectableFloorPeer};
use super::*;
use crate::websocket::transport_cleanup::close_websocket_sink_best_effort;

#[derive(Clone, Copy)]
enum LocalFloorExit {
    PreCancelledHard,
    EarlyDecision,
    HardDuringPeerAssessment,
}

#[tokio::test]
async fn generic_tunnel_shutdown_closes_upstream_while_next_create_waits_for_active_turn() {
    use std::{future::Future, task::Poll};

    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_upstream =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut upstream_peer =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let (_local_write, local_read) = router_local.split();
    let (upstream_write, _upstream_read) = router_upstream.split();
    let (_intent_sender, intent) = watch::channel(FloorSwitchIntent {
        epoch: 1,
        pending: true,
    });
    let early_reconnect = CancellationToken::new();
    let hard_reconnect = CancellationToken::new();
    let tunnel_shutdown = CancellationToken::new();
    let admission = AccountTurnAdmission::new(
        intent,
        early_reconnect.clone(),
        hard_reconnect.clone(),
        None,
        None,
        None,
        true,
    );
    client
        .send(Message::text(r#"{"type":"response.create"}"#))
        .await
        .expect("next create must be buffered before polling the actual pump");
    let mut local_pump = Box::pin(pump_local_to_upstream(
        local_read,
        upstream_write,
        LocalToUpstreamPumpContext {
            revocation: CancellationToken::new(),
            session_shutdown: CancellationToken::new(),
            tunnel_shutdown: tunnel_shutdown.clone(),
            active_turn_reservation: ActiveTurnReservationState::new(None),
            session_affinity_activity_handle: None,
            account_turn_admission: admission,
            early_floor_reconnect: early_reconnect.clone(),
            quota_floor_reconnect: hard_reconnect.clone(),
        },
    ));
    std::future::poll_fn(|context| {
        assert!(
            local_pump.as_mut().poll(context).is_pending(),
            "pending floor intent must hold the next create behind the active turn"
        );
        Poll::Ready(())
    })
    .await;
    let mut local_task = tokio::spawn(local_pump);
    tunnel_shutdown.cancel();
    let completed = tokio::time::timeout(Duration::from_secs(2), async {
        let close = upstream_peer
            .next()
            .await
            .expect("upstream Close frame")
            .expect("upstream Close must decode");
        assert!(matches!(close, Message::Close(_)), "{close:?}");
        let result = (&mut local_task)
            .await
            .expect("actual local pump must join");
        assert!(result.is_ok(), "local cleanup result: {result:?}");
    })
    .await;
    if completed.is_err() {
        local_task.abort();
        let _cleanup_join = local_task.await;
    }
    assert!(!early_reconnect.is_cancelled());
    assert!(!hard_reconnect.is_cancelled());
    assert!(
        completed.is_ok(),
        "generic tunnel shutdown must send upstream Close and join the pump while next-create admission is held: {completed:?}"
    );
}

#[tokio::test]
async fn floor_reconnect_supervisor_preserves_upstream_close_when_signal_pump_finishes_first() {
    use std::{future::Future, task::Poll};

    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_upstream =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut upstream_peer =
        WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let (mut local_write, _local_read) = router_local.split();
    let (mut upstream_write, upstream_read) = router_upstream.split();
    let (close_permit_sender, close_permit_receiver) = tokio::sync::oneshot::channel();
    let local_task = tokio::spawn(async move {
        close_permit_receiver
            .await
            .map_err(|_| WebSocketTunnelError::TaskJoin("close permit dropped".to_owned()))?;
        close_websocket_sink_best_effort(&mut upstream_write).await
    });
    let tunnel_shutdown = CancellationToken::new();
    let signal_shutdown = tunnel_shutdown.clone();
    let upstream_task = tokio::spawn(async move {
        signal_shutdown.cancel();
        local_write
            .send(Message::text(CODEX_WEBSOCKET_RECONNECT_SIGNAL))
            .await?;
        let result = close_websocket_sink_best_effort(&mut local_write).await;
        drop(upstream_read);
        result
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !upstream_task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("signal pump must finish before close is permitted");
    let revocation = CancellationToken::new();
    let session_shutdown = CancellationToken::new();
    let supervisor = supervise_websocket_pumps(
        &revocation,
        &session_shutdown,
        &tunnel_shutdown,
        local_task,
        upstream_task,
    );
    tokio::pin!(supervisor);
    std::future::poll_fn(|context| {
        assert!(
            supervisor.as_mut().poll(context).is_pending(),
            "upstream Close is still outstanding"
        );
        Poll::Ready(())
    })
    .await;
    close_permit_sender
        .send(())
        .expect("closing pump must retain its permit receiver");
    let observe_peers = async {
        let reconnect = client
            .next()
            .await
            .expect("client reconnect")
            .expect("decoded reconnect");
        assert_eq!(reconnect.to_string(), CODEX_WEBSOCKET_RECONNECT_SIGNAL);
        let close = upstream_peer
            .next()
            .await
            .expect("upstream closing frame")
            .expect("upstream must close with a handshake");
        assert!(matches!(close, Message::Close(_)));
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(supervisor, observe_peers)
    })
    .await
    .expect("both signal delivery and upstream Close must complete");
    assert!(result.is_ok(), "supervised close result: {result:?}");
}

#[derive(Clone, Copy)]
enum FirstCleanupSide {
    Local,
    Upstream,
}

#[derive(Clone, Copy)]
enum ForcedCleanupExit {
    Revocation,
    SessionShutdown,
}

struct CleanupDropNotice(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for CleanupDropNotice {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

fn held_cleanup_task() -> (
    tokio::task::JoinHandle<Result<(), WebSocketTunnelError>>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let (drop_sender, drop_receiver) = tokio::sync::oneshot::channel();
    let drop_notice = CleanupDropNotice(Some(drop_sender));
    let task = tokio::spawn(async move {
        let _drop_notice = drop_notice;
        std::future::pending::<Result<(), WebSocketTunnelError>>().await
    });
    (task, drop_receiver)
}

#[tokio::test]
async fn hard_revocation_interrupts_held_close_drain_for_either_completion_order() {
    for side in [FirstCleanupSide::Local, FirstCleanupSide::Upstream] {
        assert_forced_exit_interrupts_cleanup(side, ForcedCleanupExit::Revocation).await;
    }
}

#[tokio::test]
async fn session_shutdown_interrupts_held_close_drain_for_either_completion_order() {
    for side in [FirstCleanupSide::Local, FirstCleanupSide::Upstream] {
        assert_forced_exit_interrupts_cleanup(side, ForcedCleanupExit::SessionShutdown).await;
    }
}

async fn assert_forced_exit_interrupts_cleanup(
    first_side: FirstCleanupSide,
    forced_exit: ForcedCleanupExit,
) {
    use std::{future::Future, task::Poll};

    let (held_task, dropped) = held_cleanup_task();
    let completed = tokio::spawn(async { Ok(()) });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !completed.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first clean pump must finish");
    let (local_task, upstream_task) = match first_side {
        FirstCleanupSide::Local => (completed, held_task),
        FirstCleanupSide::Upstream => (held_task, completed),
    };
    let revocation = CancellationToken::new();
    let session_shutdown = CancellationToken::new();
    let tunnel_shutdown = CancellationToken::new();
    tunnel_shutdown.cancel();
    let supervisor = supervise_websocket_pumps(
        &revocation,
        &session_shutdown,
        &tunnel_shutdown,
        local_task,
        upstream_task,
    );
    tokio::pin!(supervisor);
    std::future::poll_fn(|context| {
        assert!(supervisor.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    match forced_exit {
        ForcedCleanupExit::Revocation => revocation.cancel(),
        ForcedCleanupExit::SessionShutdown => session_shutdown.cancel(),
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        assert!(supervisor.await.is_ok());
        dropped
            .await
            .expect("forced stop must drop the held survivor");
    })
    .await
    .expect("hard stop must interrupt cleanup without a close permit");
}

#[tokio::test]
async fn pump_error_aborts_survivor_for_either_completion_order() {
    for side in [FirstCleanupSide::Local, FirstCleanupSide::Upstream] {
        assert_non_draining_completion_aborts_survivor(side, true).await;
    }
}

#[tokio::test]
async fn ordinary_completion_without_tunnel_shutdown_aborts_survivor() {
    for side in [FirstCleanupSide::Local, FirstCleanupSide::Upstream] {
        assert_non_draining_completion_aborts_survivor(side, false).await;
    }
}

async fn assert_non_draining_completion_aborts_survivor(
    first_side: FirstCleanupSide,
    with_failure: bool,
) {
    let (held_task, dropped) = held_cleanup_task();
    let completed = tokio::spawn(async move {
        if with_failure {
            Err(WebSocketTunnelError::TaskJoin(
                "injected pump failure".to_owned(),
            ))
        } else {
            Ok(())
        }
    });
    let (local_task, upstream_task) = match first_side {
        FirstCleanupSide::Local => (completed, held_task),
        FirstCleanupSide::Upstream => (held_task, completed),
    };
    let revocation = CancellationToken::new();
    let session_shutdown = CancellationToken::new();
    let tunnel_shutdown = CancellationToken::new();
    if with_failure {
        tunnel_shutdown.cancel();
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        let result = supervise_websocket_pumps(
            &revocation,
            &session_shutdown,
            &tunnel_shutdown,
            local_task,
            upstream_task,
        )
        .await;
        if with_failure {
            assert!(
                matches!(result, Err(WebSocketTunnelError::TaskJoin(message))
                if message == "injected pump failure")
            );
        } else {
            assert!(result.is_ok());
        }
        dropped.await.expect("aborted survivor must be dropped");
    })
    .await
    .expect("non-draining completion must retain the fast-abort path");
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
