use codex_router_descriptor_boundary::{
    BoundaryError, DescriptorGate, OwnedPipe, OwnedSocket, PipeReader, PipeWriter,
};
use std::{
    error::Error,
    fs::{File, OpenOptions},
    future::Future,
    io::{Read, Write},
    os::fd::{AsFd, BorrowedFd},
    process::Stdio,
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Child,
    time::timeout,
};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const CASE_ENV: &str = "INHERITED_PIPE_CASE";
const READER_GATE_RELEASED: &[u8] = b"INHERITED_READER_GATE_RELEASED\n";
const WRITER_GATE_RELEASED: &[u8] = b"INHERITED_WRITER_GATE_RELEASED\n";
const INVALID_CASE_COMPLETE: &[u8] = b"INVALID_PIPE_REJECTED_WITH_STDIO_UNCHANGED\n";
const INHERITED_RESPONSE_MARKER: &[u8] = b"INHERITED_PIPE_RESPONSE:";
const STDIO_REPLY_MARKER: &[u8] = b"TOKIO_STDIO_REPLY:";
const FIXTURE_JOB: &[u8] =
    br#"{"type":"observe","alias":"/run/codex/gen-0123abcd-7.sock","native_wait_ms":0,"remote_wait_ms":0}"#;
const FIXTURE_REPLY: &[u8] = br#"{"type":"failed","reason":{"type":"connect"}}"#;
const INHERITED_INPUT: &[u8] = b"literal native probe job through inherited stdin";
const BACKPRESSURE_BYTES: usize = 1_048_576;
const PROOF_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StdioSnapshot {
    device: u64,
    inode: u64,
    mode: u32,
    close_on_exec: bool,
    nonblocking: bool,
}

async fn spawn_fixture(
    fixture_name: &str,
    fixture_case: Option<&str>,
    stdin: Stdio,
    stdout: Stdio,
) -> TestResult<Child> {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            fixture_name,
            "--nocapture",
            "--test-threads=1",
        ])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(fixture_case) = fixture_case {
        command.env(CASE_ENV, fixture_case);
    }
    Ok(DescriptorGate::global().spawn_child(&mut command).await?)
}

async fn read_until_marker<TReader>(
    reader: &mut TReader,
    marker: &[u8],
    captured: &mut Vec<u8>,
) -> TestResult
where
    TReader: AsyncRead + Unpin,
{
    if marker.is_empty() {
        return Err("fixture marker must not be empty".into());
    }
    let mut chunk = [0; 512];
    while !contains_marker(captured, marker) {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Err("fixture stream closed before its marker".into());
        }
        let Some(bytes) = chunk.get(..count) else {
            return Err("fixture read exceeded its buffer".into());
        };
        captured.extend_from_slice(bytes);
    }
    Ok(())
}

async fn collect_async_bytes_to_eof<TReader>(
    reader: &mut TReader,
    captured: &mut Vec<u8>,
) -> TestResult
where
    TReader: AsyncRead + Unpin,
{
    let mut chunk = [0; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Ok(());
        }
        let Some(bytes) = chunk.get(..count) else {
            return Err("fixture read exceeded its buffer".into());
        };
        captured.extend_from_slice(bytes);
    }
}

async fn collect_pipe_bytes_to_eof(reader: &PipeReader, captured: &mut Vec<u8>) -> TestResult {
    let mut chunk = [0; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Ok(());
        }
        let Some(bytes) = chunk.get(..count) else {
            return Err("pipe read exceeded its buffer".into());
        };
        captured.extend_from_slice(bytes);
    }
}

fn contains_marker(bytes: &[u8], marker: &[u8]) -> bool {
    !marker.is_empty() && bytes.windows(marker.len()).any(|window| window == marker)
}

// The marker separates the test harness banner from the raw pipe bytes under test.
fn verify_payload_after_marker(
    bytes: &[u8],
    marker: &[u8],
    expected_payload: &[u8],
    exact_tail: bool,
) -> TestResult {
    if marker.is_empty() {
        return Err("payload marker must not be empty".into());
    }
    let marker_count = bytes
        .windows(marker.len())
        .filter(|window| *window == marker)
        .count();
    if marker_count != 1 {
        return Err(format!("expected one payload marker, found {marker_count}").into());
    }
    let Some(marker_offset) = bytes
        .windows(marker.len())
        .position(|window| window == marker)
    else {
        return Err("fixture payload marker missing".into());
    };
    let Some(payload_start) = marker_offset.checked_add(marker.len()) else {
        return Err("fixture payload marker offset overflowed".into());
    };
    let Some(payload_end) = payload_start.checked_add(expected_payload.len()) else {
        return Err("fixture payload end offset overflowed".into());
    };
    let Some(actual_payload) = bytes.get(payload_start..payload_end) else {
        return Err("fixture payload ended early".into());
    };
    if actual_payload != expected_payload {
        return Err("fixture payload bytes changed".into());
    }
    if exact_tail && payload_end != bytes.len() {
        return Err("unexpected bytes followed the inherited pipe payload".into());
    }
    Ok(())
}

fn verify_pipe_flags(descriptor: BorrowedFd<'_>, label: &str) -> TestResult {
    if !rustix::io::fcntl_getfd(descriptor)?.contains(rustix::io::FdFlags::CLOEXEC) {
        return Err(format!("{label} is missing FD_CLOEXEC").into());
    }
    if !rustix::fs::fcntl_getfl(descriptor)?.contains(rustix::fs::OFlags::NONBLOCK) {
        return Err(format!("{label} is missing O_NONBLOCK").into());
    }
    Ok(())
}

fn snapshot_stdio(descriptor: BorrowedFd<'_>) -> TestResult<StdioSnapshot> {
    let stat = rustix::fs::fstat(descriptor)?;
    let descriptor_flags = rustix::io::fcntl_getfd(descriptor)?;
    let status_flags = rustix::fs::fcntl_getfl(descriptor)?;
    Ok(StdioSnapshot {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
        mode: stat.st_mode as u32,
        close_on_exec: descriptor_flags.contains(rustix::io::FdFlags::CLOEXEC),
        nonblocking: status_flags.contains(rustix::fs::OFlags::NONBLOCK),
    })
}

fn verify_stdio_unchanged(
    descriptor: BorrowedFd<'_>,
    before: StdioSnapshot,
    label: &str,
) -> TestResult {
    if snapshot_stdio(descriptor)? != before {
        return Err(format!("{label} changed after rejecting an invalid pipe").into());
    }
    Ok(())
}

fn verify_same_file(first: BorrowedFd<'_>, second: BorrowedFd<'_>, label: &str) -> TestResult {
    let first_stat = rustix::fs::fstat(first)?;
    let second_stat = rustix::fs::fstat(second)?;
    if first_stat.st_dev != second_stat.st_dev || first_stat.st_ino != second_stat.st_ino {
        return Err(format!("{label} was not restored to /dev/null").into());
    }
    Ok(())
}

fn write_fixture_status(marker: &[u8]) -> std::io::Result<()> {
    let stderr = std::io::stderr();
    let mut output = stderr.lock();
    output.write_all(marker)?;
    output.flush()
}

async fn inherited_pipe_child_behavior() -> TestResult {
    let gate = DescriptorGate::global();
    let reader = PipeReader::inherit_stdin(gate).await?;
    let mut probe = [0; 1];
    let mut pending_read = Box::pin(reader.read(&mut probe));
    let mut context = Context::from_waker(Waker::noop());
    if !matches!(pending_read.as_mut().poll(&mut context), Poll::Pending) {
        return Err("inherited stdin read was not pending before parent input".into());
    }
    let _exclusive = timeout(PROOF_TIMEOUT, gate.spawn()).await?;
    drop(_exclusive);
    drop(pending_read);
    write_fixture_status(READER_GATE_RELEASED)?;

    let mut received_input = [0; INHERITED_INPUT.len()];
    reader.read_exact(&mut received_input).await?;
    if received_input.as_slice() != INHERITED_INPUT {
        return Err("inherited stdin changed literal input bytes".into());
    }
    let mut input_eof_probe = [0; 1];
    if reader.read(&mut input_eof_probe).await? != 0 {
        return Err("inherited stdin did not preserve half-close EOF".into());
    }
    let null_input = File::open("/dev/null")?;
    verify_same_file(rustix::stdio::stdin(), null_input.as_fd(), "stdin")?;
    if rustix::io::read(rustix::stdio::stdin(), &mut input_eof_probe)? != 0 {
        return Err("child stdin was not restored to /dev/null".into());
    }

    let writer = PipeWriter::inherit_stdout(gate).await?;
    verify_pipe_flags(reader.as_fd(), "inherited stdin reader")?;
    verify_pipe_flags(writer.as_fd(), "inherited stdout writer")?;
    let null_output = OpenOptions::new().write(true).open("/dev/null")?;
    verify_same_file(rustix::stdio::stdout(), null_output.as_fd(), "stdout")?;

    let response = vec![b'R'; BACKPRESSURE_BYTES];
    writer.write_all(INHERITED_RESPONSE_MARKER).await?;
    let mut pending_write = Box::pin(writer.write_all(&response));
    if !matches!(pending_write.as_mut().poll(&mut context), Poll::Pending) {
        return Err("inherited stdout write did not pend against a full pipe".into());
    }
    let _exclusive = timeout(PROOF_TIMEOUT, gate.spawn()).await?;
    drop(_exclusive);
    write_fixture_status(WRITER_GATE_RELEASED)?;
    pending_write.await?;
    drop(writer);
    drop(reader);
    Ok(())
}

async fn verify_rejected_stdin(gate: &DescriptorGate) -> TestResult {
    let before = snapshot_stdio(rustix::stdio::stdin())?;
    let result = PipeReader::inherit_stdin(gate).await;
    if !matches!(result, Err(BoundaryError::DescriptorKind)) {
        return Err("inherited stdin accepted a non-readable anonymous pipe".into());
    }
    verify_stdio_unchanged(rustix::stdio::stdin(), before, "stdin")?;
    let null = File::open("/dev/null")?;
    rustix::stdio::dup2_stdin(&null)?;
    Ok(())
}

async fn verify_rejected_stdout(gate: &DescriptorGate) -> TestResult {
    let before = snapshot_stdio(rustix::stdio::stdout())?;
    let result = PipeWriter::inherit_stdout(gate).await;
    if !matches!(result, Err(BoundaryError::DescriptorKind)) {
        return Err("inherited stdout accepted a non-writable anonymous pipe".into());
    }
    verify_stdio_unchanged(rustix::stdio::stdout(), before, "stdout")?;
    let null = OpenOptions::new().write(true).open("/dev/null")?;
    rustix::stdio::dup2_stdout(&null)?;
    Ok(())
}

async fn invalid_stdio_fixture_behavior(case: &str) -> TestResult {
    let gate = DescriptorGate::global();
    match case {
        "reader-regular" => {
            let regular_file = tempfile::tempfile()?;
            rustix::stdio::dup2_stdin(&regular_file)?;
            verify_rejected_stdin(gate).await?;
        }
        "reader-socket" => {
            let (socket, peer) = OwnedSocket::pair(gate).await?;
            rustix::stdio::dup2_stdin(socket.as_fd())?;
            verify_rejected_stdin(gate).await?;
            drop(peer);
            drop(socket);
        }
        "reader-write-end" => {
            let (reader, writer) = OwnedPipe::pair(gate).await?;
            rustix::stdio::dup2_stdin(writer.as_fd())?;
            verify_rejected_stdin(gate).await?;
            drop(writer);
            drop(reader);
        }
        "writer-regular" => {
            let regular_file = tempfile::tempfile()?;
            rustix::stdio::dup2_stdout(&regular_file)?;
            verify_rejected_stdout(gate).await?;
        }
        "writer-socket" => {
            let (socket, peer) = OwnedSocket::pair(gate).await?;
            rustix::stdio::dup2_stdout(socket.as_fd())?;
            verify_rejected_stdout(gate).await?;
            drop(peer);
            drop(socket);
        }
        "writer-read-end" => {
            let (reader, writer) = OwnedPipe::pair(gate).await?;
            rustix::stdio::dup2_stdout(reader.as_fd())?;
            verify_rejected_stdout(gate).await?;
            drop(writer);
            drop(reader);
        }
        _ => return Err(format!("unknown inherited stdio fixture case: {case}").into()),
    }
    write_fixture_status(INVALID_CASE_COMPLETE)?;
    Ok(())
}

#[tokio::test]
#[ignore = "compiled child fixture invoked by inherited-pipe ownership scenarios"]
async fn inherited_pipe_child_fixture() -> TestResult {
    inherited_pipe_child_behavior().await
}

#[tokio::test]
#[ignore = "compiled child fixture invoked by invalid-stdio rejection scenarios"]
async fn invalid_stdio_child_fixture() -> TestResult {
    let case = std::env::var(CASE_ENV)?;
    invalid_stdio_fixture_behavior(&case).await
}

#[test]
#[ignore = "compiled child fixture invoked by Tokio stdio-to-owned-pipe scenario"]
fn converted_stdio_child_fixture() -> TestResult {
    let mut job = Vec::new();
    std::io::stdin().read_to_end(&mut job)?;
    if job.as_slice() != FIXTURE_JOB {
        return Err("converted child stdin changed literal job bytes".into());
    }
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    output.write_all(STDIO_REPLY_MARKER)?;
    output.write_all(FIXTURE_REPLY)?;
    output.flush()?;
    Ok(())
}

#[tokio::test]
async fn inherited_stdio_pipes_preserve_bytes_half_close_devnull_flags_and_gate_release()
-> TestResult {
    let mut child = spawn_fixture(
        "inherited_pipe_child_fixture",
        None,
        Stdio::piped(),
        Stdio::piped(),
    )
    .await?;
    let mut child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("fixture child stdin missing"))?;
    let mut child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("fixture child stdout missing"))?;
    let mut child_stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("fixture child stderr missing"))?;
    let mut status_bytes = Vec::new();
    timeout(
        PROOF_TIMEOUT,
        read_until_marker(&mut child_stderr, READER_GATE_RELEASED, &mut status_bytes),
    )
    .await??;

    child_stdin.write_all(INHERITED_INPUT).await?;
    drop(child_stdin);
    timeout(
        PROOF_TIMEOUT,
        read_until_marker(&mut child_stderr, WRITER_GATE_RELEASED, &mut status_bytes),
    )
    .await??;

    let mut output_bytes = Vec::new();
    timeout(
        PROOF_TIMEOUT,
        read_until_marker(
            &mut child_stdout,
            INHERITED_RESPONSE_MARKER,
            &mut output_bytes,
        ),
    )
    .await??;
    timeout(
        PROOF_TIMEOUT,
        collect_async_bytes_to_eof(&mut child_stdout, &mut output_bytes),
    )
    .await??;
    let status = timeout(PROOF_TIMEOUT, child.wait()).await??;
    if !status.success() {
        return Err(format!("inherited pipe child exited with {status}").into());
    }
    let expected_response = vec![b'R'; BACKPRESSURE_BYTES];
    verify_payload_after_marker(
        &output_bytes,
        INHERITED_RESPONSE_MARKER,
        &expected_response,
        true,
    )
}

#[tokio::test]
async fn invalid_stdio_descriptors_are_rejected_without_altering_child_stdio() -> TestResult {
    for fixture_case in [
        "reader-regular",
        "reader-socket",
        "reader-write-end",
        "writer-regular",
        "writer-socket",
        "writer-read-end",
    ] {
        let mut child = spawn_fixture(
            "invalid_stdio_child_fixture",
            Some(fixture_case),
            Stdio::null(),
            Stdio::piped(),
        )
        .await?;
        let mut child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("invalid fixture stdout missing"))?;
        let mut child_stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("invalid fixture stderr missing"))?;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        timeout(
            PROOF_TIMEOUT,
            collect_async_bytes_to_eof(&mut child_stdout, &mut stdout_bytes),
        )
        .await??;
        timeout(
            PROOF_TIMEOUT,
            collect_async_bytes_to_eof(&mut child_stderr, &mut stderr_bytes),
        )
        .await??;
        let status = timeout(PROOF_TIMEOUT, child.wait()).await??;
        if !status.success() || !contains_marker(&stderr_bytes, INVALID_CASE_COMPLETE) {
            return Err(format!("invalid stdio fixture failed for {fixture_case}").into());
        }
    }
    Ok(())
}

#[tokio::test]
async fn tokio_child_stdio_converts_to_owned_pipes_with_restored_async_flags() -> TestResult {
    let gate = DescriptorGate::global();
    let mut child = spawn_fixture(
        "converted_stdio_child_fixture",
        None,
        Stdio::piped(),
        Stdio::piped(),
    )
    .await?;
    let child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("converted child stdin missing"))?;
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("converted child stdout missing"))?;
    if !rustix::fs::fcntl_getfl(child_stdin.as_fd())?.contains(rustix::fs::OFlags::NONBLOCK)
        || !rustix::fs::fcntl_getfl(child_stdout.as_fd())?.contains(rustix::fs::OFlags::NONBLOCK)
    {
        return Err("Tokio child stdio did not begin with nonblocking descriptors".into());
    }
    let stdin_fd = child_stdin.into_owned_fd()?;
    let stdout_fd = child_stdout.into_owned_fd()?;
    if rustix::fs::fcntl_getfl(&stdin_fd)?.contains(rustix::fs::OFlags::NONBLOCK)
        || rustix::fs::fcntl_getfl(&stdout_fd)?.contains(rustix::fs::OFlags::NONBLOCK)
    {
        return Err("Tokio into_owned_fd did not restore blocking descriptors".into());
    }
    let writer = PipeWriter::from_owned(stdin_fd, gate).await?;
    let reader = PipeReader::from_owned(stdout_fd, gate).await?;
    verify_pipe_flags(writer.as_fd(), "re-adopted Tokio child stdin")?;
    verify_pipe_flags(reader.as_fd(), "re-adopted Tokio child stdout")?;

    writer.write_all(FIXTURE_JOB).await?;
    drop(writer);
    let mut output_bytes = Vec::new();
    timeout(
        PROOF_TIMEOUT,
        collect_pipe_bytes_to_eof(&reader, &mut output_bytes),
    )
    .await??;
    drop(reader);
    let status = timeout(PROOF_TIMEOUT, child.wait()).await??;
    if !status.success() {
        return Err(format!("converted stdio child exited with {status}").into());
    }
    verify_payload_after_marker(&output_bytes, STDIO_REPLY_MARKER, FIXTURE_REPLY, false)
}
