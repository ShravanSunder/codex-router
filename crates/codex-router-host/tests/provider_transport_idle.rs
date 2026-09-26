//! Regression coverage for idle ACP transport scheduler wakeups.
//! The vtable regression requires an optimized build; CI runs this target in release mode.
#![cfg(unix)]
#![allow(clippy::expect_used)]

use agent_client_protocol::{AcpAgent, AcpAgentConfig, Client, Lines};
use futures_util::StreamExt as _;
use std::future::Future as _;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Wake};
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use tokio_util::compat::{FuturesAsyncReadCompatExt as _, FuturesAsyncWriteCompatExt as _};

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn idle_provider_transport_stops_scheduling_itself() {
    // This real provider keeps stdout open but never writes to it. Its stdin
    // remains open too; there is no EOF, malformed frame, or failed request.
    let agent = AcpAgent::new(AcpAgentConfig::new("/bin/cat"));
    let (stdin, stdout, stderr, mut child) = agent.spawn_process().expect("spawn idle provider");
    let outgoing = futures_util::SinkExt::<String>::sink_map_err(
        FramedWrite::new(stdin.compat_write(), LinesCodec::new()),
        std::io::Error::other,
    );
    let read_polls = Arc::new(AtomicUsize::new(0));
    let clone_mismatches = Arc::new(AtomicUsize::new(0));
    let reader_waker = Arc::new(Mutex::new(None::<std::task::Waker>));
    let incoming = FramedRead::new(
        stdout.compat(),
        LinesCodec::new_with_max_length(64 * 1024 * 1024),
    )
    .map(|line| line.map_err(std::io::Error::other));
    let mut incoming = Box::pin(incoming);
    let counted_incoming = futures_util::stream::poll_fn({
        let read_polls = Arc::clone(&read_polls);
        let reader_waker = Arc::clone(&reader_waker);
        let clone_mismatches = Arc::clone(&clone_mismatches);
        move |context| {
            let cloned = clone_transport_waker(context.waker());
            if !context.waker().will_wake(&cloned) {
                clone_mismatches.fetch_add(1, Ordering::SeqCst);
            }
            *reader_waker.lock().expect("reader waker lock") = Some(cloned);
            read_polls.fetch_add(1, Ordering::SeqCst);
            futures_util::Stream::poll_next(incoming.as_mut(), context)
        }
    });
    let connection = Client
        .builder()
        .connect_with(Lines::new(outgoing, counted_incoming), async |_| {
            std::future::pending::<Result<(), agent_client_protocol::Error>>().await
        });
    let wake_counter = Arc::new(WakeCounter::default());
    let waker = std::task::Waker::from(Arc::clone(&wake_counter));
    let mut context = Context::from_waker(&waker);
    let mut connection = pin!(connection);
    // Prime stdout readiness registration, then deliver one spurious readiness
    // wake to the actual transport task. Async IO must tolerate spurious wakes.
    assert!(connection.as_mut().poll(&mut context).is_pending());
    reader_waker
        .lock()
        .expect("reader waker lock")
        .as_ref()
        .expect("transport registered its reader waker")
        .wake_by_ref();
    let mut quiescent = false;
    let mut scheduled_polls = 0;
    let mut total_wakes = 0;
    for _ in 0..32 {
        wake_counter.0.store(0, Ordering::SeqCst);
        scheduled_polls += 1;
        assert!(connection.as_mut().poll(&mut context).is_pending());
        let wakes = wake_counter.0.load(Ordering::SeqCst);
        total_wakes += wakes;
        if wakes == 0 {
            quiescent = true;
            break;
        }
    }
    // Always reap our fixture before asserting, even on the failing baseline.
    child.kill().expect("stop owned idle fixture");
    child.status().await.expect("reap owned idle fixture");
    drop(stderr);
    eprintln!(
        "scheduled_polls={scheduled_polls} total_wakes={total_wakes} read_polls={} clone_mismatches={} quiescent={quiescent}",
        read_polls.load(Ordering::SeqCst),
        clone_mismatches.load(Ordering::SeqCst)
    );
    assert!(
        quiescent,
        "idle connection exhausted 32 scheduled polls; stdout was polled {} times",
        read_polls.load(Ordering::SeqCst)
    );
}

#[test]
// Keep the exact SDK task type: a unique closure type can hide cross-crate
// vtable duplication and make the broken dependency appear healthy.
fn provider_task_waker_clone_preserves_identity() {
    let mut tasks = futures_util::stream::FuturesUnordered::<
        futures_util::future::BoxFuture<'static, Result<(), agent_client_protocol::Error>>,
    >::new();
    tasks.push(Box::pin(futures_util::future::poll_fn(|context| {
        let cloned = clone_transport_waker(context.waker());
        assert!(
            context.waker().will_wake(&cloned),
            "cloning a task waker changed its identity"
        );
        std::task::Poll::Ready(Ok(()))
    })));
    let mut context = Context::from_waker(std::task::Waker::noop());
    assert!(tasks.poll_next_unpin(&mut context).is_ready());
}

// Cross the same opaque callback boundary as an IO readiness registration.
#[inline(never)]
fn clone_transport_waker(waker: &std::task::Waker) -> std::task::Waker {
    waker.clone()
}
