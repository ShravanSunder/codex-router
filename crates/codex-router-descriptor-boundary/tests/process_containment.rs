use codex_router_descriptor_boundary::{
    DescriptorGate, OwnedPipe, OwnedSocket, RecordWriter, SocketReadRecord, SocketReadStream,
    UnixReceipt, supervise_reader,
};
use std::{
    io::{IoSlice, Read, Write},
    mem::MaybeUninit,
    os::fd::{AsFd, OwnedFd},
    process::Stdio,
};
use tokio::time::{Duration, timeout};
use tokio_util::task::TaskTracker;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn child_fixture(
    mode: &str,
    carrier: OwnedFd,
    stdout: Stdio,
) -> Result<tokio::process::Child, Box<dyn std::error::Error + Send + Sync>> {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args(["--ignored", "--exact", "disposable_receiver", "--nocapture"])
        .env("DESCRIPTOR_FIXTURE_MODE", mode)
        .stdin(Stdio::from(carrier))
        .stdout(Stdio::null())
        .stderr(stdout)
        .kill_on_drop(true);
    Ok(DescriptorGate::global().spawn_child(&mut command).await?)
}

#[tokio::test]
#[ignore = "fixture entry is executed by permanent process scenarios"]
async fn disposable_receiver() -> TestResult {
    let gate = DescriptorGate::global();
    let receipt = UnixReceipt::new(OwnedSocket::inherit_stdin(gate).await?);
    let mode = std::env::var("DESCRIPTOR_FIXTURE_MODE")?;
    if mode == "idle" {
        std::future::pending::<()>().await;
    }
    if mode == "stream" {
        let fd = {
            let _creation = gate.creation().await;
            rustix::io::dup(rustix::stdio::stderr())?
        };
        let pipe = codex_router_descriptor_boundary::PipeWriter::from_owned(fd, gate).await?;
        let mut records = RecordWriter::new(pipe);
        loop {
            let mut bytes = vec![0; 16384];
            let chunk = receipt.read(&mut bytes, false, gate).await?;
            if chunk.bytes == 0 {
                records.send(SocketReadRecord::ReadHalfClosed).await?;
                break;
            }
            bytes.truncate(chunk.bytes);
            records.send(SocketReadRecord::data(bytes)?).await?;
        }
    } else if mode == "poison" {
        let _chunk = receipt.read(&mut [0], false, gate).await?;
        return Err("poison receiver forwarded input".into());
    }
    Ok(())
}

#[tokio::test]
async fn original_128_into_64_witness_reaches_eof_after_reap_and_all_carrier_close() -> TestResult {
    for _ in 0..3 {
        let gate = DescriptorGate::global();
        let (sender, receiver) = OwnedSocket::pair(gate).await?;
        let child = child_fixture("poison", receiver.into_owned(), Stdio::null()).await?;
        let (witness_read, witness_write) = OwnedPipe::pair(gate).await?;
        let mut copies = Vec::new();
        {
            let _creation = gate.creation().await;
            for _ in 0..128 {
                copies.push(rustix::io::dup(witness_write.as_fd())?);
            }
        }
        let rights: Vec<_> = copies.iter().map(AsFd::as_fd).collect();
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(128))];
        let mut control = rustix::net::SendAncillaryBuffer::new(&mut space);
        if !control.push(rustix::net::SendAncillaryMessage::ScmRights(&rights)) {
            return Err("128 sender fixture failed".into());
        }
        let sent = rustix::net::sendmsg(
            sender.as_fd(),
            &[IoSlice::new(b"x")],
            &mut control,
            rustix::net::SendFlags::empty(),
        )?;
        if sent != 1 {
            return Err("poison sender failed to send input".into());
        }
        drop(rights);
        drop(copies);
        drop(witness_write);
        let tracker = TaskTracker::new();
        let mut lease = supervise_reader(child, &tracker);
        let status = timeout(Duration::from_secs(3), lease.wait()).await??;
        use std::os::unix::process::ExitStatusExt;
        if status.code() != Some(70) && status.signal().is_none() {
            return Err("poison input must kill receiving process".into());
        }
        drop(sender);
        tracker.close();
        timeout(Duration::from_secs(3), tracker.wait()).await?;
        if timeout(Duration::from_secs(3), witness_read.read(&mut [0])).await?? != 0 {
            return Err("128-right witness leaked after reap/carrier closure".into());
        }
    }
    Ok(())
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
