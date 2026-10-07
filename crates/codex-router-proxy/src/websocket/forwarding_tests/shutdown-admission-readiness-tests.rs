//! Tunnel shutdown wins an admission success ready in the same pump poll.

use super::super::duplex_forwarding::pump_upstream_to_local;
use super::floor_switch_write_readiness_tests::AbortOwnedTasks;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct HeldPermittedSource {
    entered: Notify,
    release: Notify,
    completed: AtomicUsize,
}

impl LiveAccountAdmissionAssessor for HeldPermittedSource {
    fn assess_peer<'a>(
        &'a self,
        _account_id: &'a AccountId,
        _route_band: codex_router_core::routes::RouteBand,
    ) -> BoxFuture<'a, FloorSwitchPeerAssessment> {
        Box::pin(async { FloorSwitchPeerAssessment::NoPeer })
    }

    fn assess_source_account<'a>(
        &'a self,
        _account_id: &'a AccountId,
        _generation: u64,
        _route_band: codex_router_core::routes::RouteBand,
        _credit_backed: bool,
    ) -> BoxFuture<'a, crate::account_selection::AccountSourceAdmission> {
        Box::pin(async {
            self.entered.notify_one();
            self.release.notified().await;
            self.completed.fetch_add(1, Ordering::SeqCst);
            crate::account_selection::AccountSourceAdmission::Permitted
        })
    }
}

#[tokio::test]
async fn simultaneously_ready_shutdown_prevents_admitted_create_from_forwarding() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let router_local =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let router_upstream =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let mut upstream = WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let (local_write, local_read) = router_local.split();
    let (upstream_write, upstream_read) = router_upstream.split();
    let registry = WebSocketRevocationRegistry::new();
    let session = registry.register_cancellation(TokenGeneration::new(1));
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();
    let tunnel_shutdown = CancellationToken::new();
    let source = Arc::new(HeldPermittedSource {
        entered: Notify::new(),
        release: Notify::new(),
        completed: AtomicUsize::new(0),
    });
    let admission = AccountTurnAdmission::new(
        session.graceful_floor_switch.clone(),
        session.early_floor_reconnect.clone(),
        session.quota_floor_reconnect.clone(),
        Some(AccountId::new("acct_ready_admission").expect("fixture account id")),
        Some(1),
        Some(source.clone()),
        false,
    );
    let local_task = tokio::spawn(pump_local_to_upstream(
        local_read,
        upstream_write,
        LocalToUpstreamPumpContext {
            revocation: revocation.clone(),
            session_shutdown: session_shutdown.clone(),
            tunnel_shutdown: tunnel_shutdown.clone(),
            active_turn_reservation: ActiveTurnReservationState::new(None),
            session_affinity_activity_handle: None,
            account_turn_admission: admission.clone(),
            early_floor_reconnect: session.early_floor_reconnect.clone(),
            quota_floor_reconnect: session.quota_floor_reconnect.clone(),
        },
    ));
    let local_abort = local_task.abort_handle();
    let upstream_task = tokio::spawn(pump_upstream_to_local(
        upstream_read,
        local_write,
        UpstreamToLocalPumpContext {
            revocation: revocation.clone(),
            session_shutdown: session_shutdown.clone(),
            tunnel_shutdown: tunnel_shutdown.clone(),
            session_registry: registry.clone(),
            session_id: session.session_id,
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            affinity_owner_context: None,
            active_turn_reservation: ActiveTurnReservationState::new(None),
            provider_error_observer: None,
            quota_floor_reconnect: session.quota_floor_reconnect.clone(),
            early_floor_reconnect: session.early_floor_reconnect.clone(),
            graceful_floor_switch: session.graceful_floor_switch.clone(),
            account_turn_admission: admission,
        },
    ));
    let upstream_abort = upstream_task.abort_handle();
    let _abort_on_unwind = AbortOwnedTasks(vec![local_abort.clone(), upstream_abort.clone()]);
    client
        .send(Message::text(r#"{"type":"response.create","turn":2}"#))
        .await
        .expect("actual next create sends");
    tokio::time::timeout(Duration::from_secs(2), source.entered.notified())
        .await
        .expect("actual pump parks in source admission");
    // No await between these: both futures are ready before the pump resumes.
    source.release.notify_one();
    tunnel_shutdown.cancel();
    let supervisor = supervise_websocket_pumps(
        &revocation,
        &session_shutdown,
        &tunnel_shutdown,
        local_task,
        upstream_task,
    );
    let peers = async {
        let upstream_frame = tokio::time::timeout(Duration::from_secs(2), upstream.next())
            .await
            .expect("bounded actual upstream frame read");
        upstream
            .flush()
            .await
            .expect("actual peer Close acknowledgement flushes");
        let client_frame = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("bounded actual client Close read");
        (upstream_frame, client_frame)
    };
    let results = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(supervisor, peers)
    })
    .await;
    if results.is_err() {
        local_abort.abort();
        upstream_abort.abort();
    }
    let (supervisor_result, (upstream_frame, client_frame)) =
        results.expect("actual pumps and supervisor settle after cancellation");
    drop(session);

    assert!(
        supervisor_result.is_ok(),
        "actual supervisor: {supervisor_result:?}"
    );
    assert!(
        local_abort.is_finished() && upstream_abort.is_finished(),
        "owned pumps joined"
    );
    assert_eq!(registry.snapshot().active_sessions, 0);
    assert_eq!(
        source.completed.load(Ordering::SeqCst),
        0,
        "cancelled admission success is not consumed"
    );
    assert!(
        matches!(upstream_frame, Some(Ok(Message::Close(_)))),
        "queued create must not forward when shutdown is already ready: {upstream_frame:?}"
    );
    assert!(
        matches!(client_frame, Some(Ok(Message::Close(_)))),
        "actual client Close: {client_frame:?}"
    );
}
