use codex_router_descriptor_boundary::{
    DescriptorGate, MAX_DATA_BYTES, OwnedSocket, ReceiptByteStream, UnixReceipt,
};
use std::{
    error::Error,
    future::Future,
    io::IoSlice,
    mem::MaybeUninit,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::time::{Duration, timeout};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

struct WriteWakeCounter {
    wake_count: AtomicUsize,
    notification: tokio::sync::Notify,
}

impl WriteWakeCounter {
    fn new() -> Self {
        Self {
            wake_count: AtomicUsize::new(0),
            notification: tokio::sync::Notify::new(),
        }
    }
}

impl Wake for WriteWakeCounter {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_count.fetch_add(1, Ordering::SeqCst);
        self.notification.notify_one();
    }
}

fn try_prefill_socket(socket: &OwnedSocket, bytes: &[u8]) -> std::io::Result<usize> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(64))];
    let mut control = rustix::net::SendAncillaryBuffer::new(&mut space);
    let flags = rustix::net::SendFlags::DONTWAIT;
    #[cfg(not(target_vendor = "apple"))]
    let flags = flags | rustix::net::SendFlags::NOSIGNAL;
    rustix::net::sendmsg(socket.as_fd(), &[IoSlice::new(bytes)], &mut control, flags)
        .map_err(std::io::Error::from)
}

async fn send_all(socket: &OwnedSocket, bytes: &[u8]) -> TestResult {
    let mut written = 0;
    while written < bytes.len() {
        let Some(remaining) = bytes.get(written..) else {
            return Err("send offset exceeded its buffer".into());
        };
        let count = socket.send(remaining, &[]).await?;
        if count == 0 {
            return Err("socket send made no progress".into());
        }
        written += count;
    }
    Ok(())
}

async fn send_fragmented(socket: &OwnedSocket, bytes: &[u8]) -> TestResult {
    let fragment_sizes = [1, 7, 31, 1024, 3];
    let mut written = 0;
    let mut next_fragment = 0;
    while written < bytes.len() {
        let Some(fragment_size) = fragment_sizes.get(next_fragment % fragment_sizes.len()) else {
            return Err("fragment-size fixture was empty".into());
        };
        let length = (*fragment_size).min(bytes.len() - written);
        let Some(end) = written.checked_add(length) else {
            return Err("fragmented send offset overflowed".into());
        };
        let Some(fragment) = bytes.get(written..end) else {
            return Err("fragmented send offset exceeded its buffer".into());
        };
        send_all(socket, fragment).await?;
        written = end;
        next_fragment += 1;
        tokio::task::yield_now().await;
    }
    Ok(())
}

async fn read_exact_receipt(receipt: &UnixReceipt, expected: &mut [u8]) -> TestResult {
    let mut received = 0;
    while received < expected.len() {
        let Some(remaining) = expected.get_mut(received..) else {
            return Err("receipt offset exceeded its buffer".into());
        };
        let count = receipt
            .read(remaining, false, DescriptorGate::global())
            .await?
            .bytes;
        if count == 0 {
            return Err("receipt reached EOF before expected bytes".into());
        }
        received += count;
    }
    Ok(())
}

async fn receive_fragmented(
    stream: &mut ReceiptByteStream,
    expected_length: usize,
) -> TestResult<Vec<u8>> {
    let read_sizes = [1, 2, 7, 31, 1023, 8192];
    let mut received = vec![0; expected_length];
    let mut position = 0;
    let mut next_size = 0;
    while position < received.len() {
        let Some(read_size) = read_sizes.get(next_size % read_sizes.len()) else {
            return Err("read-size fixture was empty".into());
        };
        let length = (*read_size).min(received.len() - position);
        let Some(end) = position.checked_add(length) else {
            return Err("fragmented read offset overflowed".into());
        };
        let Some(read_buffer) = received.get_mut(position..end) else {
            return Err("fragmented read offset exceeded its buffer".into());
        };
        let count = stream.read(read_buffer).await?;
        if count == 0 {
            return Err("receipt stream ended before the literal payload".into());
        }
        position += count;
        next_size += 1;
    }
    Ok(received)
}

#[tokio::test]
async fn literal_bytes_cross_receipt_chunks_and_half_close_only_the_read_side() -> TestResult {
    let gate = DescriptorGate::global();
    let (connected, peer) = OwnedSocket::pair(gate).await?;
    let mut stream = ReceiptByteStream::new(UnixReceipt::new(connected));
    let payload = vec![b'R'; MAX_DATA_BYTES + 513];

    let mut no_capacity = [];
    let mut empty_read = ReadBuf::new(&mut no_capacity);
    let mut context = Context::from_waker(Waker::noop());
    if !matches!(
        Pin::new(&mut stream).poll_read(&mut context, &mut empty_read),
        Poll::Ready(Ok(()))
    ) {
        return Err("zero-capacity read did not complete without consuming input".into());
    }

    let (send_result, receive_result) = timeout(Duration::from_secs(3), async {
        tokio::join!(
            send_fragmented(&peer, &payload),
            receive_fragmented(&mut stream, payload.len())
        )
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "payload transfer timed out"))?;
    send_result?;
    let received = receive_result?;
    if received != payload {
        return Err("receipt stream changed literal bytes".into());
    }

    rustix::net::shutdown(peer.as_fd(), rustix::net::Shutdown::Write)?;
    let mut eof_probe = [0; 1];
    if timeout(Duration::from_secs(3), stream.read(&mut eof_probe))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "read EOF timed out"))??
        != 0
    {
        return Err("peer write half-close was not observed as ordinary EOF".into());
    }

    timeout(Duration::from_secs(3), stream.write_all(b"reply"))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "reply write timed out")
        })??;
    stream.flush().await?;
    stream.shutdown().await?;
    let peer_receipt = UnixReceipt::new(peer);
    let mut reply = [0; 5];
    timeout(
        Duration::from_secs(3),
        read_exact_receipt(&peer_receipt, &mut reply),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "reply read timed out"))??;
    if &reply != b"reply" {
        return Err("read half-close suppressed the direct reply".into());
    }
    if timeout(
        Duration::from_secs(3),
        peer_receipt.read(&mut eof_probe, false, gate),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "peer EOF timed out"))??
    .bytes
        != 0
    {
        return Err("stream shutdown did not close only its write direction".into());
    }
    Ok(())
}

#[tokio::test]
async fn canceled_pending_read_keeps_receipt_progress_and_does_not_hold_creation_gate() -> TestResult
{
    let gate = DescriptorGate::global();
    let (connected, peer) = OwnedSocket::pair(gate).await?;
    let mut stream = ReceiptByteStream::new(UnixReceipt::new(connected));
    let mut context = Context::from_waker(Waker::noop());
    {
        let mut caller_bytes = [0; 8];
        let mut caller_read = ReadBuf::new(&mut caller_bytes);
        let mut caller_future = Box::pin(std::future::poll_fn(|context| {
            Pin::new(&mut stream).poll_read(context, &mut caller_read)
        }));
        if !matches!(caller_future.as_mut().poll(&mut context), Poll::Pending) {
            return Err("empty socket read did not remain pending".into());
        }
        drop(caller_future);
    }

    drop(timeout(Duration::from_secs(1), gate.spawn()).await?);
    send_all(&peer, b"pending").await?;
    let mut pending_result = [0; 7];
    stream.read_exact(&mut pending_result).await?;
    if &pending_result != b"pending" {
        return Err("dropping the caller read lost or changed pending input".into());
    }

    send_all(&peer, b"cache").await?;
    let mut first = [0; 1];
    if stream.read(&mut first).await? != 1 || first != *b"c" {
        return Err("small caller buffer did not receive the first cached byte".into());
    }
    let mut no_capacity = [];
    let mut empty_read = ReadBuf::new(&mut no_capacity);
    if !matches!(
        Pin::new(&mut stream).poll_read(&mut context, &mut empty_read),
        Poll::Ready(Ok(()))
    ) {
        return Err("zero-capacity read did not leave cached input available".into());
    }
    let mut cached_remainder = [0; 4];
    stream.read_exact(&mut cached_remainder).await?;
    if &cached_remainder != b"ache" {
        return Err("cached bytes were discarded, reset, or duplicated".into());
    }
    Ok(())
}

#[tokio::test]
async fn dropping_stream_with_pending_read_closes_its_owned_socket() -> TestResult {
    let gate = DescriptorGate::global();
    let (connected, peer) = OwnedSocket::pair(gate).await?;
    {
        let mut stream = ReceiptByteStream::new(UnixReceipt::new(connected));
        {
            let mut caller_bytes = [0; 8];
            let mut caller_read = ReadBuf::new(&mut caller_bytes);
            let mut context = Context::from_waker(Waker::noop());
            let mut caller_future = Box::pin(std::future::poll_fn(|context| {
                Pin::new(&mut stream).poll_read(context, &mut caller_read)
            }));
            if !matches!(caller_future.as_mut().poll(&mut context), Poll::Pending) {
                return Err("empty socket read did not remain pending before stream drop".into());
            }
            drop(caller_future);
        }
    }

    let peer_receipt = UnixReceipt::new(peer);
    if timeout(
        Duration::from_secs(3),
        peer_receipt.read(&mut [0; 1], false, gate),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "stream-drop EOF timed out"))??
    .bytes
        != 0
    {
        return Err("dropping the stream did not close its owned socket".into());
    }
    Ok(())
}

#[tokio::test]
async fn canceled_pending_write_cannot_replay_stale_bytes_before_replacement_write() -> TestResult {
    let gate = DescriptorGate::global();
    let (connected, peer) = OwnedSocket::pair(gate).await?;
    let mut stream = ReceiptByteStream::new(UnixReceipt::new(connected));
    let mut context = Context::from_waker(Waker::noop());
    let fill = [b'F'; MAX_DATA_BYTES];
    let mut filled = timeout(Duration::from_secs(3), stream.write(&fill))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "initial write timed out")
        })??;
    if filled == 0 {
        return Err("backpressure fill made no progress".into());
    }
    loop {
        match Pin::new(&mut stream).poll_write(&mut context, &fill) {
            Poll::Ready(Ok(0)) => return Err("backpressure fill made no progress".into()),
            Poll::Ready(Ok(count)) => {
                filled += count;
                if filled > 8 * 1024 * 1024 {
                    return Err("socketpair did not reach bounded backpressure".into());
                }
            }
            Poll::Pending => break,
            Poll::Ready(Err(error)) => return Err(error.into()),
        }
    }
    if filled == 0 {
        return Err("backpressure setup accepted no bytes".into());
    }

    let mut canceled_write = Box::pin(stream.write_all(b"A"));
    if !matches!(canceled_write.as_mut().poll(&mut context), Poll::Pending) {
        return Err("write A did not remain pending behind a full socket".into());
    }
    drop(canceled_write);

    let peer_receipt = UnixReceipt::new(peer);
    let mut fill_bytes = vec![0; filled];
    timeout(
        Duration::from_secs(3),
        read_exact_receipt(&peer_receipt, &mut fill_bytes),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "filler drain timed out"))??;
    if fill_bytes.iter().any(|byte| *byte != b'F') {
        return Err("backpressure setup changed the filler bytes".into());
    }

    timeout(Duration::from_secs(3), stream.write_all(b"B"))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "replacement write timed out")
        })??;
    stream.flush().await?;
    let mut replacement = [0; 1];
    timeout(
        Duration::from_secs(3),
        read_exact_receipt(&peer_receipt, &mut replacement),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "replacement read timed out")
    })??;
    if replacement != *b"B" {
        return Err("replacement write received stale or incorrect bytes".into());
    }
    stream.shutdown().await?;
    let mut extra = [0; 1];
    if timeout(
        Duration::from_secs(3),
        peer_receipt.read(&mut extra, false, gate),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "post-shutdown EOF timed out"))??
    .bytes
        != 0
    {
        return Err("canceled write emitted extra ghost bytes".into());
    }
    Ok(())
}

#[tokio::test]
async fn writable_waiter_is_woken_after_a_cached_ready_write_hits_backpressure() -> TestResult {
    let gate = DescriptorGate::global();
    let (connected, peer) = OwnedSocket::pair(gate).await?;
    if connected.send(b"seed", &[]).await? != b"seed".len() {
        return Err("readiness seed write was partial".into());
    }

    let fill = [b'F'; MAX_DATA_BYTES];
    let mut expected_prefill = b"seed".to_vec();
    loop {
        match try_prefill_socket(&connected, &fill) {
            Ok(0) => return Err("raw backpressure prefill made no progress".into()),
            Ok(count) => {
                let Some(new_length) = expected_prefill.len().checked_add(count) else {
                    return Err("raw backpressure prefill length overflowed".into());
                };
                if new_length > 8 * 1024 * 1024 {
                    return Err("socketpair did not reach raw-send backpressure".into());
                }
                expected_prefill.resize(new_length, b'F');
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }

    let mut stream = ReceiptByteStream::new(UnixReceipt::new(connected));
    let wake_counter = Arc::new(WriteWakeCounter::new());
    let waker = Waker::from(Arc::clone(&wake_counter));
    let mut context = Context::from_waker(&waker);
    if !matches!(
        Pin::new(&mut stream).poll_write(&mut context, b"A"),
        Poll::Pending
    ) {
        return Err("full socket write did not become pending".into());
    }
    if wake_counter.wake_count.load(Ordering::SeqCst) != 0 {
        return Err("full socket write woke before the peer drained bytes".into());
    }

    let peer_receipt = UnixReceipt::new(peer);
    let mut observed_prefill = vec![0; expected_prefill.len()];
    timeout(
        Duration::from_secs(3),
        read_exact_receipt(&peer_receipt, &mut observed_prefill),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "backpressure drain timed out")
    })??;
    if observed_prefill != expected_prefill {
        return Err("peer drain changed cached-ready prefill bytes".into());
    }
    timeout(Duration::from_secs(3), wake_counter.notification.notified())
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "write waker did not fire")
        })?;
    if wake_counter.wake_count.load(Ordering::SeqCst) == 0 {
        return Err("peer drain completed without waking the write owner".into());
    }

    if !matches!(
        Pin::new(&mut stream).poll_write(&mut context, b"A"),
        Poll::Ready(Ok(1))
    ) {
        return Err("woken replacement write did not send exactly one byte".into());
    }
    let mut written_byte = [0; 1];
    read_exact_receipt(&peer_receipt, &mut written_byte).await?;
    if written_byte != *b"A" {
        return Err("woken write changed its literal byte".into());
    }
    stream.shutdown().await?;
    if peer_receipt.read(&mut [0; 1], false, gate).await?.bytes != 0 {
        return Err("woken writer emitted unexpected bytes after its literal byte".into());
    }
    Ok(())
}

#[tokio::test]
async fn peer_read_eof_then_shutdown_reports_broken_pipe_without_sigpipe() -> TestResult {
    let gate = DescriptorGate::global();
    let (connected, peer) = OwnedSocket::pair(gate).await?;
    let mut stream = ReceiptByteStream::new(UnixReceipt::new(connected));
    rustix::net::shutdown(peer.as_fd(), rustix::net::Shutdown::Write)?;
    let mut eof_probe = [0; 1];
    if timeout(Duration::from_secs(3), stream.read(&mut eof_probe))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "read EOF timed out"))??
        != 0
    {
        return Err("peer write half-close was not observed before shutdown".into());
    }
    drop(peer);
    match timeout(Duration::from_secs(3), stream.write(b"child output"))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "BrokenPipe write timed out")
        })? {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(format!("peer-close write returned {:?}", error.kind()).into()),
        Ok(count) => Err(format!("peer-close write unexpectedly sent {count} bytes").into()),
    }
}
