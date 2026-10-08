//! Real WebSocket cleanup must survive a peer write becoming ready later.

use super::super::duplex_forwarding::pump_upstream_to_local;
use super::*;
use futures_util::task::AtomicWaker;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::task::AbortHandle;

struct WriteReadinessGate {
    held: AtomicBool,
    entered: Notify,
    forwarded_bytes: AtomicUsize,
    waker: AtomicWaker,
}

impl WriteReadinessGate {
    fn new(held: bool) -> Self {
        Self {
            held: AtomicBool::new(held),
            entered: Notify::new(),
            forwarded_bytes: AtomicUsize::new(0),
            waker: AtomicWaker::new(),
        }
    }

    fn release(&self) {
        self.held.store(false, Ordering::SeqCst);
        self.waker.wake();
    }
}

struct WriteReadinessIo {
    inner: DuplexStream,
    gate: Arc<WriteReadinessGate>,
}

impl AsyncRead for WriteReadinessIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl AsyncWrite for WriteReadinessIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.gate.waker.register(context.waker());
        if self.gate.held.load(Ordering::SeqCst) {
            self.gate.entered.notify_one();
            return Poll::Pending;
        }
        let result = Pin::new(&mut self.inner).poll_write(context, bytes);
        if let Poll::Ready(Ok(written)) = result {
            self.gate
                .forwarded_bytes
                .fetch_add(written, Ordering::SeqCst);
        }
        result
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

pub(super) struct AbortOwnedTasks(pub(super) Vec<AbortHandle>);

impl Drop for AbortOwnedTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

#[derive(Clone, Copy)]
enum FirstPump {
    Local,
    Upstream,
}

#[derive(Clone, Copy)]
enum ForcedSignal {
    Revocation,
    SessionShutdown,
}

#[derive(Clone, Copy)]
enum CloseTrigger {
    HardFloor,
    ParkedQuota,
}

struct CloseCase {
    first_pump: FirstPump,
    hold_write: bool,
    forced_signal: Option<ForcedSignal>,
    trigger: CloseTrigger,
}

impl CloseCase {
    fn hard(first_pump: FirstPump, forced_signal: Option<ForcedSignal>) -> Self {
        Self {
            first_pump,
            hold_write: true,
            forced_signal,
            trigger: CloseTrigger::HardFloor,
        }
    }

    fn parked(trigger: CloseTrigger) -> Self {
        Self {
            first_pump: FirstPump::Upstream,
            hold_write: true,
            forced_signal: None,
            trigger,
        }
    }
}

struct ObservedSourceDenial(Arc<Notify>);

impl LiveAccountAdmissionAssessor for ObservedSourceDenial {
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
            self.0.notify_one();
            crate::account_selection::AccountSourceAdmission::ReconnectRequired
        })
    }
}

#[tokio::test]
async fn upstream_first_cleanup_delivers_peer_close_after_write_becomes_ready() {
    assert_close_case(CloseCase::hard(FirstPump::Upstream, None)).await;
}

#[tokio::test]
async fn ready_peer_write_delivers_both_floor_close_frames() {
    let mut case = CloseCase::hard(FirstPump::Upstream, None);
    case.hold_write = false;
    assert_close_case(case).await;
}

#[tokio::test]
async fn local_first_cleanup_delivers_client_close_after_write_becomes_ready() {
    assert_close_case(CloseCase::hard(FirstPump::Local, None)).await;
}

#[tokio::test]
async fn quota_close_cancels_parked_create_and_delivers_both_closes() {
    assert_close_case(CloseCase::parked(CloseTrigger::ParkedQuota)).await;
}

#[tokio::test]
async fn revocation_reaps_held_local_cleanup_after_upstream_finishes() {
    assert_close_case(CloseCase::hard(
        FirstPump::Upstream,
        Some(ForcedSignal::Revocation),
    ))
    .await;
}

#[tokio::test]
async fn shutdown_reaps_held_local_cleanup_after_upstream_finishes() {
    assert_close_case(CloseCase::hard(
        FirstPump::Upstream,
        Some(ForcedSignal::SessionShutdown),
    ))
    .await;
}

#[tokio::test]
async fn revocation_reaps_held_upstream_cleanup_after_local_finishes() {
    assert_close_case(CloseCase::hard(
        FirstPump::Local,
        Some(ForcedSignal::Revocation),
    ))
    .await;
}

#[tokio::test]
async fn shutdown_reaps_held_upstream_cleanup_after_local_finishes() {
    assert_close_case(CloseCase::hard(
        FirstPump::Local,
        Some(ForcedSignal::SessionShutdown),
    ))
    .await;
}

async fn assert_close_case(case: CloseCase) {
    use std::future::Future as _;

    let parked = !matches!(case.trigger, CloseTrigger::HardFloor);
    let upstream_first = matches!(case.first_pump, FirstPump::Upstream);
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let client_gate = Arc::new(WriteReadinessGate::new(case.hold_write && !upstream_first));
    let upstream_gate = Arc::new(WriteReadinessGate::new(case.hold_write && upstream_first));
    let router_local = WebSocketStream::from_raw_socket(
        WriteReadinessIo {
            inner: router_local_stream,
            gate: Arc::clone(&client_gate),
        },
        Role::Server,
        None,
    )
    .await;
    let mut client = WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_upstream = WebSocketStream::from_raw_socket(
        WriteReadinessIo {
            inner: router_upstream_stream,
            gate: Arc::clone(&upstream_gate),
        },
        Role::Client,
        None,
    )
    .await;
    let mut upstream = WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let (local_write, local_read) = router_local.split();
    let (upstream_write, upstream_read) = router_upstream.split();
    let registry = WebSocketRevocationRegistry::new();
    let selected_account = AccountId::new("acct_write_readiness").expect("fixture account id");
    let session = registry.register_cancellation_with_peer_addr(
        TokenGeneration::new(1),
        selected_account.clone(),
        None,
    );
    let session_id = session.session_id;
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();
    let tunnel_shutdown = CancellationToken::new();
    let hard_reconnect = session.quota_floor_reconnect.clone();
    let early_reconnect = session.early_floor_reconnect.clone();
    let intent = session.graceful_floor_switch.clone();
    let reservations = RouteBandReservationBooks::default();
    let reservation_handle = reservations
        .lock()
        .expect("reservation books")
        .entry("responses".to_owned())
        .or_default()
        .reserve_next_at(selected_account.clone(), 1, 1);
    let reservation = ActiveReservationGuard::new(
        reservations.clone(),
        "responses".to_owned(),
        reservation_handle,
    );
    let active_reservation = ActiveTurnReservationState::new(Some(reservation.clone()));
    let source_entered = Arc::new(Notify::new());
    let assessor: Option<Arc<dyn LiveAccountAdmissionAssessor>> = parked.then(|| {
        Arc::new(ObservedSourceDenial(Arc::clone(&source_entered)))
            as Arc<dyn LiveAccountAdmissionAssessor>
    });
    let admission = AccountTurnAdmission::new(
        intent.clone(),
        early_reconnect.clone(),
        hard_reconnect.clone(),
        Some(selected_account.clone()),
        Some(1),
        assessor,
        parked,
    );
    let affinity_context = WebSocketAffinityOwnerContext {
        affinity_secret: RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("fixture affinity secret"),
        account_id: selected_account.clone(),
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: Some(reservation),
        session_affinity_activity_handle: None,
    };
    let local_context = LocalToUpstreamPumpContext {
        revocation: revocation.clone(),
        session_shutdown: session_shutdown.clone(),
        tunnel_shutdown: tunnel_shutdown.clone(),
        active_turn_reservation: active_reservation.clone(),
        session_affinity_activity_handle: None,
        account_turn_admission: admission.clone(),
        early_floor_reconnect: early_reconnect.clone(),
        quota_floor_reconnect: hard_reconnect.clone(),
    };
    let upstream_context = UpstreamToLocalPumpContext {
        revocation: revocation.clone(),
        session_shutdown: session_shutdown.clone(),
        tunnel_shutdown: tunnel_shutdown.clone(),
        session_registry: registry.clone(),
        session_id,
        affinity_owner_recorder: None,
        async_affinity_owner_recorder: None,
        affinity_record_tasks: TaskTracker::new(),
        affinity_owner_context: Some(affinity_context),
        active_turn_reservation: active_reservation,
        provider_error_observer: Some(Arc::new(RecordingAsyncProviderErrorObserver::default())),
        quota_floor_reconnect: hard_reconnect.clone(),
        early_floor_reconnect: early_reconnect,
        graceful_floor_switch: intent,
        account_turn_admission: admission,
    };
    let (local_done_sender, local_done) = tokio::sync::oneshot::channel();
    let local_future = async move {
        let result = pump_local_to_upstream(local_read, upstream_write, local_context).await;
        let _ = local_done_sender.send(result.is_ok());
        result
    };
    let (upstream_done_sender, upstream_done) = tokio::sync::oneshot::channel();
    let upstream_future = async move {
        let result = pump_upstream_to_local(upstream_read, local_write, upstream_context).await;
        let _ = upstream_done_sender.send(result.is_ok());
        result
    };
    if !parked {
        hard_reconnect.cancel();
    }
    let mut abort_on_unwind = AbortOwnedTasks(Vec::new());
    let (local_task, upstream_task) = if upstream_first {
        let local_task = tokio::spawn(local_future);
        abort_on_unwind.0.push(local_task.abort_handle());
        if parked {
            client
                .send(Message::text(r#"{"type":"response.create","turn":2}"#))
                .await
                .expect("queued next create sends");
            tokio::time::timeout(Duration::from_secs(2), source_entered.notified())
                .await
                .expect(
                    "actual next create enters source admission while first turn remains active",
                );
        } else if case.hold_write {
            tokio::time::timeout(Duration::from_secs(2), upstream_gate.entered.notified())
                .await
                .expect("local Close poll_write held BEFORE upstream pump starts");
        }
        let upstream_task = tokio::spawn(upstream_future);
        (local_task, upstream_task)
    } else {
        let upstream_task = tokio::spawn(upstream_future);
        abort_on_unwind.0.push(upstream_task.abort_handle());
        tokio::time::timeout(Duration::from_secs(2), client_gate.entered.notified())
            .await
            .expect("client signal poll_write held BEFORE local pump starts");
        let local_task = tokio::spawn(local_future);
        (local_task, upstream_task)
    };
    let local_abort = local_task.abort_handle();
    let upstream_abort = upstream_task.abort_handle();
    abort_on_unwind
        .0
        .extend([local_abort.clone(), upstream_abort.clone()]);
    let expected_client_signal = match case.trigger {
        CloseTrigger::HardFloor => CODEX_WEBSOCKET_RECONNECT_SIGNAL,
        CloseTrigger::ParkedQuota => {
            upstream.send(Message::text(r#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#))
                .await.expect("actual upstream quota classifier input sends");
            CODEX_WEBSOCKET_RECONNECT_SIGNAL
        }
    };
    let first_finished_ok = tokio::time::timeout(
        Duration::from_secs(2),
        if upstream_first {
            upstream_done
        } else {
            local_done
        },
    )
    .await
    .expect("first actual pump completes while opposite write is held")
    .expect("actual first pump completion receipt");
    let held_gate = if upstream_first {
        &upstream_gate
    } else {
        &client_gate
    };
    if case.hold_write && parked {
        tokio::time::timeout(Duration::from_secs(2), held_gate.entered.notified())
            .await
            .expect("survivor peer write reaches real readiness gate");
    }
    let held_bytes = held_gate.forwarded_bytes.load(Ordering::SeqCst);
    let first_abort = if upstream_first {
        &upstream_abort
    } else {
        &local_abort
    };
    let survivor_abort = if upstream_first {
        &local_abort
    } else {
        &upstream_abort
    };
    let first_task_finished = first_abort.is_finished();
    let survivor_was_live = !survivor_abort.is_finished();
    let mut supervisor = Box::pin(supervise_websocket_pumps(
        &revocation,
        &session_shutdown,
        &tunnel_shutdown,
        local_task,
        upstream_task,
    ));
    let mut forced_completed_while_held = true;
    let supervisor_result = if let Some(signal) = case.forced_signal {
        // First is already Ready; this poll must enter the real retained-survivor branch.
        std::future::poll_fn(|context| {
            assert!(
                supervisor.as_mut().poll(context).is_pending(),
                "actual held survivor must keep supervisor pending"
            );
            Poll::Ready(())
        })
        .await;
        match signal {
            ForcedSignal::Revocation => revocation.cancel(),
            ForcedSignal::SessionShutdown => session_shutdown.cancel(),
        }
        match tokio::time::timeout(Duration::from_secs(2), &mut supervisor).await {
            Ok(result) => result,
            Err(_) => {
                forced_completed_while_held = false;
                survivor_abort.abort();
                tokio::time::timeout(Duration::from_secs(2), &mut supervisor)
                    .await
                    .expect("aborted actual survivor is reaped for fixture cleanup")
            }
        }
    } else {
        held_gate.release();
        tokio::time::timeout(Duration::from_secs(2), &mut supervisor)
            .await
            .expect("actual supervisor settles after write release")
    };
    drop(supervisor);
    drop(session);
    let client_signal = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("bounded client frame read");
    let client_close = if matches!(client_signal, Some(Ok(Message::Text(_)))) {
        Some(
            tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("bounded client Close read"),
        )
    } else {
        None
    };
    let peer_close = tokio::time::timeout(Duration::from_secs(2), upstream.next())
        .await
        .expect("bounded upstream frame read");

    assert!(
        first_finished_ok && first_task_finished,
        "first real pump succeeds and finishes before supervision"
    );
    assert!(
        supervisor_result.is_ok(),
        "actual supervisor result: {supervisor_result:?}"
    );
    assert!(
        local_abort.is_finished() && upstream_abort.is_finished(),
        "both owned pumps were reaped"
    );
    assert_eq!(registry.snapshot().active_sessions, 0);
    assert_eq!(
        reservations
            .lock()
            .expect("reservation books")
            .get("responses")
            .expect("response reservation book")
            .active_session_count(&selected_account),
        0
    );
    if case.hold_write {
        assert_eq!(held_bytes, 0, "no bytes reach held peer before readiness");
    }
    if case.forced_signal.is_some() {
        assert!(
            survivor_was_live && forced_completed_while_held,
            "forced signal must reap actual held cleanup without releasing its peer write"
        );
        if upstream_first {
            assert!(
                matches!(client_signal, Some(Ok(ref message)) if message.to_string() == expected_client_signal),
                "actual original client payload: {client_signal:?}"
            );
            assert!(
                matches!(client_close, Some(Some(Ok(Message::Close(_))))),
                "actual completed client Close: {client_close:?}"
            );
            assert!(
                matches!(peer_close, Some(Err(_)) | None),
                "forced held upstream emits no extra frame: {peer_close:?}"
            );
        } else {
            assert!(
                matches!(peer_close, Some(Ok(Message::Close(_)))),
                "actual completed upstream Close: {peer_close:?}"
            );
            assert!(
                matches!(client_signal, Some(Err(_)) | None),
                "forced held client emits no extra frame: {client_signal:?}"
            );
        }
    } else {
        assert!(
            matches!(client_signal, Some(Ok(ref message)) if message.to_string() == expected_client_signal),
            "actual client payload: {client_signal:?}"
        );
        assert!(
            matches!(client_close, Some(Some(Ok(Message::Close(_))))),
            "actual client Close: {client_close:?}"
        );
        assert!(
            matches!(peer_close, Some(Ok(Message::Close(_)))),
            "actual upstream peer Close, never queued create: {peer_close:?}"
        );
        assert_eq!(registry.snapshot().quota_reconnect_signal_count, 1);
    }
}
