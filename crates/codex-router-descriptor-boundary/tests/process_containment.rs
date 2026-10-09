use codex_router_descriptor_boundary::{
    DescriptorGate, MAX_DATA_BYTES, OwnedPipe, OwnedSocket, ReceiptByteStream, RecordWriter,
    SocketReadRecord, SocketReadStream, UnixReceipt, supervise_reader,
};
use std::{
    future::Future,
    io::{IoSlice, Read, Write},
    mem::MaybeUninit,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    process::Stdio,
    task::{Context, Poll, Waker},
};
use tokio::{
    io::AsyncReadExt,
    time::{Duration, timeout},
};
use tokio_util::task::TaskTracker;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn child_fixture(
    mode: &str,
    carrier: OwnedFd,
    stderr: Stdio,
) -> Result<tokio::process::Child, Box<dyn std::error::Error + Send + Sync>> {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args(["--ignored", "--exact", "disposable_receiver", "--nocapture"])
        .env("DESCRIPTOR_FIXTURE_MODE", mode)
        .stdin(Stdio::from(carrier))
        .stdout(Stdio::piped())
        .stderr(stderr)
        .kill_on_drop(true);
    Ok(DescriptorGate::global().spawn_child(&mut command).await?)
}

#[tokio::test]
#[ignore = "fixture entry is executed by permanent process scenarios"]
async fn disposable_receiver() -> TestResult {
    let gate = DescriptorGate::global();
    let raw_receipt = UnixReceipt::new(OwnedSocket::inherit_stdin(gate).await?);
    let mode = std::env::var("DESCRIPTOR_FIXTURE_MODE")?;
    if mode == "idle" {
        std::future::pending::<()>().await;
    }
    if mode == "poison-direct-one-byte" {
        direct_poison_read(&raw_receipt, 1, gate).await?;
    } else if mode == "poison-direct-max-data" {
        direct_poison_read(&raw_receipt, MAX_DATA_BYTES, gate).await?;
    }
    let mut receipt = ReceiptByteStream::new(raw_receipt);
    if mode == "stream" {
        let fd = {
            let _creation = gate.creation().await;
            rustix::io::dup(rustix::stdio::stderr())?
        };
        let pipe = codex_router_descriptor_boundary::PipeWriter::from_owned(fd, gate).await?;
        let mut records = RecordWriter::new(pipe);
        loop {
            let mut bytes = vec![0; 16384];
            let count = receipt.read(&mut bytes).await?;
            if count == 0 {
                records.send(SocketReadRecord::ReadHalfClosed).await?;
                break;
            }
            bytes.truncate(count);
            records.send(SocketReadRecord::data(bytes)?).await?;
        }
    } else if mode == "poison" {
        adapter_poison_read(&mut receipt).await?;
    } else if mode == "poison-after-prefix" {
        let mut prefix = [0; 6];
        receipt.read_exact(&mut prefix).await?;
        write_receipt_marker(&prefix)?;
        let mut bytes = [0; 16];
        let count = receipt.read(&mut bytes).await?;
        let Some(received_bytes) = bytes.get(..count) else {
            return Err("late poison fixture count exceeded its buffer".into());
        };
        write_receipt_marker(received_bytes)?;
        return Err("poison receiver forwarded input".into());
    }
    Ok(())
}

async fn direct_poison_read(
    receipt: &UnixReceipt,
    capacity: usize,
    gate: &DescriptorGate,
) -> TestResult {
    let mut bytes = vec![0; capacity];
    let mut read = Box::pin(receipt.read(&mut bytes, false, gate));
    let mut context = Context::from_waker(Waker::noop());
    if !matches!(read.as_mut().poll(&mut context), Poll::Pending) {
        return Err("direct poison read was not pending before its readiness marker".into());
    }
    write_receiver_ready_marker()?;
    let received = read.await?;
    let Some(received_bytes) = bytes.get(..received.bytes) else {
        return Err("direct poison count exceeded its buffer".into());
    };
    write_receipt_marker(received_bytes)?;
    Err("direct receipt poison returned bytes".into())
}

async fn adapter_poison_read(receipt: &mut ReceiptByteStream) -> TestResult {
    let mut bytes = [0; 1];
    let mut read = Box::pin(receipt.read(&mut bytes));
    let mut context = Context::from_waker(Waker::noop());
    if !matches!(read.as_mut().poll(&mut context), Poll::Pending) {
        return Err("adapter poison read was not pending before its readiness marker".into());
    }
    write_receiver_ready_marker()?;
    let count = read.await?;
    let Some(received_bytes) = bytes.get(..count) else {
        return Err("adapter poison count exceeded its buffer".into());
    };
    write_receipt_marker(received_bytes)?;
    Err("adapter receipt poison returned bytes".into())
}

fn write_receiver_ready_marker() -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    output.write_all(b"RECEIPT-READY")?;
    output.flush()
}

fn write_receipt_marker(bytes: &[u8]) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    output.write_all(b"RECEIPT-DATA:")?;
    output.write_all(bytes)?;
    output.flush()
}

fn send_rights(
    sender: BorrowedFd<'_>,
    bytes: &[u8],
    rights: &[BorrowedFd<'_>],
) -> Result<usize, rustix::io::Errno> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(128))];
    let mut control = rustix::net::SendAncillaryBuffer::new(&mut space);
    if !control.push(rustix::net::SendAncillaryMessage::ScmRights(rights)) {
        return Err(rustix::io::Errno::INVAL);
    }
    rustix::net::sendmsg(
        sender,
        &[IoSlice::new(bytes)],
        &mut control,
        rustix::net::SendFlags::empty(),
    )
}

#[derive(Debug)]
struct PoisonObservation {
    mode: &'static str,
    exit_code: Option<i32>,
    signal: Option<i32>,
    forwarded_marker: bool,
    witness_eof: bool,
    diagnostics: String,
}

async fn observe_poisoned_rights(
    mode: &'static str,
    right_count: usize,
    payload: &[u8],
) -> TestResult<PoisonObservation> {
    let gate = DescriptorGate::global();
    let (sender, receiver) = OwnedSocket::pair(gate).await?;
    let mut child = child_fixture(mode, receiver.into_owned(), Stdio::piped()).await?;
    let mut marker_output = child.stdout.take().ok_or("poison child stdout absent")?;
    let mut diagnostic_output = child.stderr.take().ok_or("poison child stderr absent")?;
    let mut output = read_until_marker(&mut marker_output, b"RECEIPT-READY").await?;
    let (witness_read, witness_write) = OwnedPipe::pair(gate).await?;
    let mut copies = Vec::new();
    {
        let _creation = gate.creation().await;
        for _ in 0..right_count {
            copies.push(rustix::io::dup(witness_write.as_fd())?);
        }
    }
    let rights: Vec<_> = copies.iter().map(AsFd::as_fd).collect();
    let sent = send_rights(sender.as_fd(), payload, &rights)?;
    if sent != payload.len() {
        return Err("poison sender failed to send input".into());
    }
    drop(rights);
    drop(copies);
    drop(witness_write);

    let tracker = TaskTracker::new();
    let mut lease = supervise_reader(child, &tracker);
    let status = timeout(Duration::from_secs(3), lease.wait()).await??;
    drop(sender);
    use std::os::unix::process::ExitStatusExt;
    let exit_code = status.code();
    let signal = status.signal();
    let mut trailing_output = Vec::new();
    timeout(
        Duration::from_secs(3),
        marker_output.read_to_end(&mut trailing_output),
    )
    .await??;
    output.extend_from_slice(&trailing_output);
    let mut diagnostic_bytes = Vec::new();
    timeout(
        Duration::from_secs(3),
        diagnostic_output.read_to_end(&mut diagnostic_bytes),
    )
    .await??;
    tracker.close();
    timeout(Duration::from_secs(3), tracker.wait()).await?;
    let witness_eof = timeout(Duration::from_secs(3), witness_read.read(&mut [0])).await?? == 0;
    Ok(PoisonObservation {
        mode,
        exit_code,
        signal,
        forwarded_marker: contains_marker(&output, b"RECEIPT-DATA:"),
        witness_eof,
        diagnostics: String::from_utf8_lossy(&diagnostic_bytes).into_owned(),
    })
}

fn verify_poison_observation(observation: &PoisonObservation) -> TestResult {
    let disposition = match (observation.exit_code, observation.signal) {
        (Some(70), None) => "explicit-exit-70".to_owned(),
        (None, Some(signal)) => format!("abnormal-signal-{signal}; typed exit unproven"),
        (Some(code), None) => format!("unexpected-exit-{code}"),
        (None, None) => "missing-process-disposition".to_owned(),
        (Some(code), Some(signal)) => format!("ambiguous-exit-{code}-signal-{signal}"),
    };
    eprintln!(
        "poison observation: mode={}, disposition={}, forwarded_marker={}, witness_eof={}, diagnostics={:?}",
        observation.mode,
        disposition,
        observation.forwarded_marker,
        observation.witness_eof,
        observation.diagnostics
    );
    if observation.exit_code != Some(70) && observation.signal.is_none() {
        return Err(format!(
            "poison receiver was not terminated by explicit exit 70 or abnormal signal: {disposition}"
        )
        .into());
    }
    if observation.forwarded_marker {
        return Err("poisoned socket bytes reached the child output marker".into());
    }
    if !observation.witness_eof {
        return Err("poison witness did not reach EOF after reaping and carrier closure".into());
    }
    Ok(())
}

fn contains_marker(output: &[u8], marker: &[u8]) -> bool {
    !marker.is_empty() && output.windows(marker.len()).any(|window| window == marker)
}

async fn read_until_marker(
    output: &mut tokio::process::ChildStdout,
    marker: &[u8],
) -> TestResult<Vec<u8>> {
    let mut collected = Vec::new();
    let mut chunk = [0; 256];
    while !contains_marker(&collected, marker) {
        let count = timeout(Duration::from_secs(3), output.read(&mut chunk)).await??;
        if count == 0 {
            return Err("child output closed before the expected marker".into());
        }
        let Some(bytes) = chunk.get(..count) else {
            return Err("child output exceeded its read buffer".into());
        };
        collected.extend_from_slice(bytes);
    }
    Ok(collected)
}

#[tokio::test]
async fn one_unexpected_right_kills_the_receiving_adapter_before_output() -> TestResult {
    verify_poison_observation(&observe_poisoned_rights("poison", 1, b"x").await?)
}

#[tokio::test]
async fn original_128_into_64_witness_reaches_eof_after_reap_and_all_carrier_close() -> TestResult {
    for _ in 0..3 {
        let observation = observe_poisoned_rights("poison-direct-one-byte", 128, b"x").await?;
        verify_poison_observation(&observation)?;
    }
    Ok(())
}

#[tokio::test]
async fn raw_receipt_and_adapter_capture_128_right_truncation_disposition() -> TestResult {
    let observations = [
        observe_poisoned_rights("poison-direct-one-byte", 128, b"x").await?,
        observe_poisoned_rights("poison-direct-max-data", 128, b"x").await?,
        observe_poisoned_rights("poison", 128, b"x").await?,
    ];
    let mut failures = Vec::new();
    for observation in &observations {
        if let Err(error) = verify_poison_observation(observation) {
            failures.push(error.to_string());
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; ").into());
    }
    Ok(())
}

#[tokio::test]
async fn poison_after_an_ordinary_prefix_does_not_forward_the_poisoned_bytes() -> TestResult {
    let gate = DescriptorGate::global();
    let (sender, receiver) = OwnedSocket::pair(gate).await?;
    let (witness_read, witness_write) = OwnedPipe::pair(gate).await?;
    let mut child =
        child_fixture("poison-after-prefix", receiver.into_owned(), Stdio::piped()).await?;
    let mut marker_output = child.stdout.take().ok_or("poison child stdout absent")?;
    let mut diagnostic_output = child.stderr.take().ok_or("poison child stderr absent")?;
    let tracker = TaskTracker::new();
    let mut lease = supervise_reader(child, &tracker);

    let prefix = b"prefix";
    if sender.send(prefix, &[]).await? != prefix.len() {
        return Err("ordinary prefix send was partial".into());
    }
    let mut expected_prefix_marker = b"RECEIPT-DATA:".to_vec();
    expected_prefix_marker.extend_from_slice(prefix);
    let mut output = read_until_marker(&mut marker_output, &expected_prefix_marker).await?;
    if !contains_marker(&output, &expected_prefix_marker) {
        return Err("ordinary prefix marker changed before poison".into());
    }

    let mut copies = Vec::new();
    {
        let _creation = gate.creation().await;
        copies.push(rustix::io::dup(witness_write.as_fd())?);
    }
    let rights: Vec<_> = copies.iter().map(AsFd::as_fd).collect();
    if send_rights(sender.as_fd(), b"poison", &rights)? != b"poison".len() {
        return Err("late poison sender failed to send input".into());
    }
    drop(rights);
    drop(copies);
    drop(witness_write);

    let status = timeout(Duration::from_secs(3), lease.wait()).await??;
    drop(sender);
    let trailing_start = output.len();
    let mut trailing_output = Vec::new();
    timeout(
        Duration::from_secs(3),
        marker_output.read_to_end(&mut trailing_output),
    )
    .await??;
    let mut diagnostics = Vec::new();
    timeout(
        Duration::from_secs(3),
        diagnostic_output.read_to_end(&mut diagnostics),
    )
    .await??;
    verify_fatal_termination(status, "late poison", &diagnostics)?;
    output.extend_from_slice(&trailing_output);
    let Some(trailing_bytes) = output.get(trailing_start..) else {
        return Err("child output offset exceeded its buffer".into());
    };
    if contains_marker(trailing_bytes, b"RECEIPT-DATA:poison") {
        return Err("poisoned bytes followed the already-forwarded ordinary prefix".into());
    }
    tracker.close();
    timeout(Duration::from_secs(3), tracker.wait()).await?;
    if timeout(Duration::from_secs(3), witness_read.read(&mut [0])).await?? != 0 {
        return Err("late poison witness leaked after reap and carrier closure".into());
    }
    Ok(())
}

fn verify_fatal_termination(
    status: std::process::ExitStatus,
    scenario: &str,
    diagnostics: &[u8],
) -> TestResult {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(70), None) => {
            eprintln!("{scenario} disposition: explicit-exit-70");
            Ok(())
        }
        (None, Some(signal)) => {
            eprintln!("{scenario} disposition: abnormal-signal-{signal}; typed exit unproven");
            Ok(())
        }
        (code, signal) => Err(format!(
            "{scenario} expected exit 70 or abnormal signal, observed code {code:?}, signal {signal:?}; diagnostics: {}",
            String::from_utf8_lossy(diagnostics)
        )
        .into()),
    }
}

#[tokio::test]
async fn explicit_socket_half_close_keeps_parent_direct_write_side_alive() -> TestResult {
    let gate = DescriptorGate::global();
    let (parent, child) = OwnedSocket::pair(gate).await?;
    let direct = child.duplicate(gate).await?.into_writer();
    let (read, write) = OwnedPipe::pair(gate).await?;
    let child = child_fixture(
        "stream",
        child.into_owned(),
        Stdio::from(write.into_owned()),
    )
    .await?;
    let tracker = TaskTracker::new();
    let mut lease = supervise_reader(child, &tracker);
    let peer = std::os::unix::net::UnixStream::from(parent.into_owned());
    peer.set_nonblocking(false)?;
    let mut peer_writer = peer.try_clone()?;
    peer_writer.write_all(b"request")?;
    peer_writer.shutdown(std::net::Shutdown::Write)?;
    let mut stream = SocketReadStream::new(read);
    let mut received = Vec::new();
    while let SocketReadRecord::Data(bytes) =
        timeout(Duration::from_secs(3), stream.next_record()).await??
    {
        received.extend(bytes);
    }
    if received != b"request" {
        return Err("reader changed request bytes".into());
    }
    if !timeout(Duration::from_secs(3), lease.wait())
        .await??
        .success()
    {
        return Err("normal half-close receiver did not exit normally".into());
    }
    direct.write_all(b"terminal reply").await?;
    direct.shutdown_write()?;
    let mut reply = String::new();
    peer_writer.read_to_string(&mut reply)?;
    if reply != "terminal reply" {
        return Err("read half-close suppressed direct terminal response".into());
    }
    tracker.close();
    tracker.wait().await;
    Ok(())
}

#[tokio::test]
async fn outer_codec_abort_notifies_retained_waiter_to_stop_and_reap_reader() -> TestResult {
    let gate = DescriptorGate::global();
    let (parent, child) = OwnedSocket::pair(gate).await?;
    let child = child_fixture("idle", child.into_owned(), Stdio::null()).await?;
    let pid = child.id().ok_or("child pid absent")?;
    let parent_group = rustix::process::getpgid(None)?;
    let child_pid =
        rustix::process::Pid::from_raw(i32::try_from(pid)?).ok_or("invalid child pid")?;
    if rustix::process::getpgid(Some(child_pid))? != parent_group {
        return Err("reader moved to a new process group".into());
    }
    let tracker = TaskTracker::new();
    let lease = supervise_reader(child, &tracker);
    let (started_send, started_recv) = tokio::sync::oneshot::channel();
    let codec = tokio::spawn(async move {
        let _lease = lease;
        let _send = started_send.send(());
        std::future::pending::<()>().await;
    });
    started_recv.await?;
    codec.abort();
    let _cancelled = codec.await;
    tracker.close();
    timeout(Duration::from_secs(3), tracker.wait()).await?;
    if !matches!(
        rustix::process::waitpid(Some(child_pid), rustix::process::WaitOptions::NOHANG),
        Err(rustix::io::Errno::CHILD)
    ) {
        return Err("cleanup task failed to reap child after outer abort".into());
    }
    let receipt = UnixReceipt::new(parent);
    if timeout(Duration::from_secs(3), receipt.read(&mut [0], false, gate))
        .await??
        .bytes
        != 0
    {
        return Err("reader still holds carrier after reap".into());
    }
    if rustix::process::getpgid(None)? != parent_group {
        return Err("cleanup altered parent process group".into());
    }
    Ok(())
}
