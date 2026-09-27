use super::floor_switch_tests::ImmediateSelectableFloorPeer;
use super::*;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::task::Waker;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::DuplexStream;
use tokio::io::ReadBuf;

struct HeldFlushGate {
    held: AtomicBool,
    entered: Notify,
    observe_reads: AtomicBool,
    read_observed: Notify,
    waker: Mutex<Option<Waker>>,
}

impl HeldFlushGate {
    fn new() -> Self {
        Self {
            held: AtomicBool::new(true),
            entered: Notify::new(),
            observe_reads: AtomicBool::new(false),
            read_observed: Notify::new(),
            waker: Mutex::new(None),
        }
    }

    fn release(&self) {
        self.held.store(false, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().expect("flush waker lock").take() {
            waker.wake();
        }
    }

    fn observe_next_read(&self) {
        self.observe_reads.store(true, Ordering::SeqCst);
    }
}

struct HeldFlushIo {
    inner: DuplexStream,
    gate: Arc<HeldFlushGate>,
}

impl AsyncRead for HeldFlushIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let filled_before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(context, buffer);
        if matches!(result, Poll::Ready(Ok(())))
            && buffer.filled().len() > filled_before
            && self.gate.observe_reads.swap(false, Ordering::SeqCst)
        {
            self.gate.read_observed.notify_one();
        }
        result
    }
}

impl AsyncWrite for HeldFlushIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(context, bytes)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.gate.held.load(Ordering::SeqCst) {
            *self.gate.waker.lock().expect("flush waker lock") = Some(context.waker().clone());
            self.gate.entered.notify_one();
            return Poll::Pending;
        }
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

#[tokio::test]
async fn terminal_delivery_blocks_next_create_until_turn_state_commits() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let flush_gate = Arc::new(HeldFlushGate::new());
    let router_local = WebSocketStream::from_raw_socket(
        HeldFlushIo {
            inner: router_local_stream,
            gate: Arc::clone(&flush_gate),
        },
        Role::Server,
        None,
    )
    .await;
    let mut client = WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_upstream =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut upstream = WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let registry = WebSocketRevocationRegistry::new();
    let account_id = AccountId::new("acct_terminal_turn_gate").expect("fixture account id");
    let session = registry.register_cancellation_with_peer_addr(
        TokenGeneration::new(1),
        account_id.clone(),
        None,
    );
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();
    let notifier = WebSocketQuotaFloorNotifier::new(registry);
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .expect("fixture affinity secret");
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id,
        credential_generation: 1,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let router_task = forward_duplex_until_complete(
        router_local,
        router_upstream,
        WebSocketForwardingContext {
            session_registration: session,
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            affinity_owner_context: Some(&affinity_owner_context),
            provider_error_observer: None,
            floor_switch_peer_assessor: Some(Arc::new(ImmediateSelectableFloorPeer)),
            initial_turn_active: true,
            revocation: &revocation,
            session_shutdown: &session_shutdown,
        },
    );
    let peer_task = async {
        upstream
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .expect("first terminal frame should send");
        let first_terminal = client
            .next()
            .await
            .expect("client should see first terminal")
            .expect("first terminal should decode");
        assert_eq!(
            first_terminal.to_string(),
            r#"{"type":"response.completed","turn":1}"#
        );
        flush_gate.entered.notified().await;
        client
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await
            .expect("second turn should queue");
        assert!(
            tokio::time::timeout(Duration::from_millis(250), upstream.next())
                .await
                .is_err(),
            "second create must not reach upstream before terminal state commits"
        );
        flush_gate.release();
        let second_create = tokio::time::timeout(Duration::from_secs(2), upstream.next())
            .await
            .expect("second turn should reach upstream after release")
            .expect("second create should exist")
            .expect("second create should decode");
        assert_eq!(
            second_create.to_string(),
            r#"{"type":"response.create","turn":2}"#
        );
        notifier.request_weekly_quota_floor_switch(&affinity_owner_context.account_id);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), client.next())
                .await
                .is_err(),
            "later floor intent must not interrupt active second turn"
        );
        upstream
            .send(Message::text(r#"{"type":"response.completed","turn":2}"#))
            .await
            .expect("second terminal frame should send");
        let second_terminal = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("second terminal should arrive")
            .expect("second terminal should exist")
            .expect("second terminal should decode");
        assert_eq!(
            second_terminal.to_string(),
            r#"{"type":"response.completed","turn":2}"#
        );
        let reconnect = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("floor reconnect should follow second terminal")
            .expect("reconnect should exist")
            .expect("reconnect should decode");
        assert_eq!(reconnect.to_string(), CODEX_WEBSOCKET_RECONNECT_SIGNAL);
        drop(client);
        drop(upstream);
    };
    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(4), async {
        tokio::join!(router_task, peer_task)
    })
    .await
    .expect("terminal turn gate fixture should finish");
    assert!(
        router_result.is_ok(),
        "router should finish: {router_result:?}"
    );
}

#[tokio::test]
async fn hard_floor_closes_old_upstream_while_terminal_delivery_holds_turn_gate() {
    let (router_local_stream, client_stream) = duplex(4096);
    let (router_upstream_stream, upstream_stream) = duplex(4096);
    let flush_gate = Arc::new(HeldFlushGate::new());
    let router_local = WebSocketStream::from_raw_socket(
        HeldFlushIo {
            inner: router_local_stream,
            gate: Arc::clone(&flush_gate),
        },
        Role::Server,
        None,
    )
    .await;
    let mut client = WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_upstream =
        WebSocketStream::from_raw_socket(router_upstream_stream, Role::Client, None).await;
    let mut upstream = WebSocketStream::from_raw_socket(upstream_stream, Role::Server, None).await;
    let registry = WebSocketRevocationRegistry::new();
    let account_id = AccountId::new("acct_hard_turn_gate").expect("fixture account id");
    let session = registry.register_cancellation_with_peer_addr(
        TokenGeneration::new(1),
        account_id.clone(),
        None,
    );
    let revocation = session.cancellation().clone();
    let session_shutdown = CancellationToken::new();
    let notifier = WebSocketQuotaFloorNotifier::new(registry);
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .expect("fixture affinity secret");
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: account_id.clone(),
        credential_generation: 1,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let router_task = forward_duplex_until_complete(
        router_local,
        router_upstream,
        WebSocketForwardingContext {
            session_registration: session,
            affinity_owner_recorder: None,
            async_affinity_owner_recorder: None,
            affinity_record_tasks: TaskTracker::new(),
            affinity_owner_context: Some(&affinity_owner_context),
            provider_error_observer: None,
            floor_switch_peer_assessor: None,
            initial_turn_active: true,
            revocation: &revocation,
            session_shutdown: &session_shutdown,
        },
    );
    let peer_task = async {
        upstream
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .expect("terminal frame should send");
        let terminal = client
            .next()
            .await
            .expect("client should see terminal frame")
            .expect("terminal frame should decode");
        assert_eq!(
            terminal.to_string(),
            r#"{"type":"response.completed","turn":1}"#
        );
        flush_gate.entered.notified().await;
        flush_gate.observe_next_read();
        client
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await
            .expect("second create should queue behind terminal delivery");
        flush_gate.read_observed.notified().await;
        notifier.signal_weekly_quota_floor_reached(&account_id);

        let old_upstream_exit = tokio::time::timeout(Duration::from_secs(1), upstream.next())
            .await
            .expect("hard floor must close old upstream without terminal flush release")
            .expect("old upstream should receive a close frame")
            .expect("old upstream close should decode");
        assert!(
            matches!(old_upstream_exit, Message::Close(_)),
            "queued second create must never reach the hard-stopped account: {old_upstream_exit:?}"
        );

        flush_gate.release();
        let reconnect = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("hard floor reconnect should arrive after terminal flush")
            .expect("reconnect frame should exist")
            .expect("reconnect frame should decode");
        assert_eq!(reconnect.to_string(), CODEX_WEBSOCKET_RECONNECT_SIGNAL);
        if let Ok(Some(Ok(frame))) =
            tokio::time::timeout(Duration::from_millis(200), upstream.next()).await
        {
            assert!(
                !is_response_create(&frame),
                "queued second create must not forward after terminal release"
            );
        }
        drop(client);
        drop(upstream);
    };
    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(4), async {
        tokio::join!(router_task, peer_task)
    })
    .await
    .expect("hard floor turn-gate fixture should finish");
    assert!(
        router_result.is_ok(),
        "router should finish: {router_result:?}"
    );
}
