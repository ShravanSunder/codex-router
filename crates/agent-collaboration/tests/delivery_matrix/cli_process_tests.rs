//! Permanent child-process proof using this same integration-test executable only.
use super::{CliProcessError, run_until_deadline};
use crate::proof_context::ProofResult;
use std::{
    future::pending,
    io::{self, Write as _},
    os::unix::fs::{FileTypeExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    net::{UnixListener, UnixStream},
    process::Command,
    sync::oneshot,
};

const CONTROL_SOCKET_ENV: &str = "U6_CLI_PROCESS_FIXTURE_SOCKET";
const CHILD_TEST: &str = "cli_process::tests::controlled_cli_child_entrypoint";
const HANDSHAKE_BOUND: Duration = Duration::from_secs(5);
type ReadyWitness = ProofResult<(u32, UnixStream)>;

struct ChildFixture {
    _directory: tempfile::TempDir,
    socket: PathBuf,
}

impl ChildFixture {
    fn create() -> ProofResult<Self> {
        let directory = tempfile::Builder::new()
            .prefix("u6-cli-child-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")?;
        let socket = directory.path().join("control.sock");
        Ok(Self {
            _directory: directory,
            socket,
        })
    }

    fn command(&self) -> ProofResult<Command> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--ignored",
                "--exact",
                CHILD_TEST,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CONTROL_SOCKET_ENV, &self.socket);
        Ok(command)
    }
}

async fn readiness_deadline(
    listener: UnixListener,
    release: Option<u8>,
    witness: oneshot::Sender<ReadyWitness>,
) {
    let observation: ReadyWitness = match tokio::time::timeout(HANDSHAKE_BOUND, async {
        let (stream, _) = listener.accept().await?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let mut fields = line.split_whitespace();
        if fields.next() != Some("READY") {
            return Err("child did not report the literal READY protocol".into());
        }
        let pid: u32 = fields
            .next()
            .ok_or("READY omitted actual child PID")?
            .parse()?;
        if pid == 0 || fields.next().is_some() {
            return Err("READY contained an invalid PID or extra field".into());
        }
        let mut stream = reader.into_inner();
        if let Some(control) = release {
            stream.write_all(&[control]).await?;
        }
        Ok((pid, stream))
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(error.into()),
    };
    let released = release.is_some() && observation.is_ok();
    // The channel retains the control stream until AFTER the helper returns. Expiry is not EOF.
    let _reported = witness.send(observation);
    if released {
        pending::<()>().await;
    }
}

async fn require_ready_witness(
    witness: oneshot::Receiver<ReadyWitness>,
    child_result: &ProofResult<std::process::Output>,
) -> ReadyWitness {
    match witness.await {
        Ok(observed) => observed,
        Err(_) => match child_result {
            Ok(output) => {
                // This is only our exact controlled test executable, never a provider.
                let stderr_tail = output
                    .stderr
                    .get(output.stderr.len().saturating_sub(1_024)..)
                    .unwrap_or_default();
                Err(format!(
                    "controlled child exited before READY: status={:?}; stdoutBytes={}; stderrBytes={}; controlledStderrTail={}",
                    output.status, output.stdout.len(), output.stderr.len(),
                    String::from_utf8_lossy(stderr_tail),
                ).into())
            }
            Err(error) => {
                Err(format!("controlled child observation failed before READY: {error}").into())
            }
        },
    }
}

async fn require_reported_child_absent(pid: u32) -> ProofResult<()> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .await?;
    if output.status.code() != Some(1) || !output.stdout.is_empty() || !output.stderr.is_empty() {
        return Err("targeted ps did not confirm reported owned child PID absent".into());
    }
    Ok(())
}

#[tokio::test]
async fn successful_child_captures_both_literal_streams_and_real_status() -> ProofResult<()> {
    let fixture = ChildFixture::create()?;
    let listener = UnixListener::bind(&fixture.socket)?;
    let command = fixture.command()?;
    let (report, witness) = oneshot::channel();
    let result =
        run_until_deadline(command, readiness_deadline(listener, Some(b'X'), report)).await;
    let (pid, control_stream) = require_ready_witness(witness, &result).await?;
    let absence = require_reported_child_absent(pid).await;
    drop(control_stream);
    absence?;
    let output = result?;
    if !output.status.success() {
        return Err("released fixture child did not return successful actual wait status".into());
    }
    if std::str::from_utf8(&output.stdout)?
        .matches("U6_STDOUT_BLOCK\n")
        .count()
        != 8_192
    {
        return Err("stdout pipe did not preserve all 8192 literal blocks".into());
    }
    if std::str::from_utf8(&output.stderr)?
        .matches("U6_STDERR_BLOCK\n")
        .count()
        != 8_192
    {
        return Err("stderr pipe did not preserve all 8192 literal blocks".into());
    }
    Ok(())
}

#[tokio::test]
async fn ready_deadline_returns_only_after_exact_child_termination_and_wait() -> ProofResult<()> {
    let fixture = ChildFixture::create()?;
    let listener = UnixListener::bind(&fixture.socket)?;
    let command = fixture.command()?;
    let (report, witness) = oneshot::channel();
    let result = run_until_deadline(command, readiness_deadline(listener, None, report)).await;
    let (pid, control_stream) = require_ready_witness(witness, &result).await?;
    let wait_validation: ProofResult<()> = (|| {
        let error = match result {
            Err(error) => error,
            Ok(_) => return Err("deadline became successful CLI output".into()),
        };
        let error = error
            .downcast_ref::<CliProcessError>()
            .ok_or("deadline omitted typed child cleanup evidence")?;
        if error.pid != Some(pid) || error.cause.kind() != io::ErrorKind::TimedOut {
            return Err("deadline error did not match actual READY PID and timeout cause".into());
        }
        if error.termination_error.is_some() {
            return Err("owned child termination reported an error".into());
        }
        let status = error
            .wait_result
            .as_ref()
            .map_err(|_| "owned child wait/reap failed")?;
        if status.success() {
            return Err(
                "blocked child unexpectedly exited successfully before deadline termination".into(),
            );
        }
        Ok(())
    })();
    let absence = require_reported_child_absent(pid).await;
    // Always check the actual PID and release EOF before returning either failure.
    drop(control_stream);
    wait_validation?;
    absence
}

#[tokio::test]
async fn nonzero_exit_remains_output_for_the_existing_cli_result_consumer() -> ProofResult<()> {
    let fixture = ChildFixture::create()?;
    let listener = UnixListener::bind(&fixture.socket)?;
    let command = fixture.command()?;
    let (report, witness) = oneshot::channel();
    let result =
        run_until_deadline(command, readiness_deadline(listener, Some(b'F'), report)).await;
    let (pid, control_stream) = require_ready_witness(witness, &result).await?;
    let absence = require_reported_child_absent(pid).await;
    drop(control_stream);
    absence?;
    if result?.status.success() {
        return Err("nonzero fixture status was rewritten as successful CLI status".into());
    }
    Ok(())
}

#[tokio::test]
async fn spawn_failure_returns_original_io_error_without_child_or_fixture_effects()
-> ProofResult<()> {
    let directory = tempfile::Builder::new()
        .prefix("u6-cli-missing-")
        .tempdir_in("/tmp")?;
    let command = Command::new(directory.path().join("absent-executable"));
    let error = match run_until_deadline(command, pending::<()>()).await {
        Err(error) => error,
        Ok(_) => return Err("absent executable unexpectedly produced CLI output".into()),
    };
    if error.downcast_ref::<io::Error>().map(io::Error::kind) != Some(io::ErrorKind::NotFound) {
        return Err("spawn failure did not preserve the original not-found IO error".into());
    }
    if std::fs::read_dir(directory.path())?.next().is_some() {
        return Err("failed spawn created unexpected fixture artifacts".into());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "fixture child only; requires explicit private Unix control socket"]
async fn controlled_cli_child_entrypoint() -> ProofResult<()> {
    let socket = std::env::var_os(CONTROL_SOCKET_ENV)
        .map(PathBuf::from)
        .ok_or("fixture requires explicit private Unix socket environment")?;
    let parent = socket.parent().ok_or("fixture socket parent missing")?;
    let parent_metadata = std::fs::symlink_metadata(parent)?;
    if !socket.is_absolute()
        || !parent_metadata.file_type().is_dir()
        || parent_metadata.permissions().mode() & 0o077 != 0
        || !std::fs::symlink_metadata(&socket)?.file_type().is_socket()
    {
        return Err("fixture requires a private existing Unix control socket".into());
    }
    {
        let mut stdout = std::io::stdout().lock();
        let mut stderr = std::io::stderr().lock();
        for _ in 0..8_192 {
            stdout.write_all(b"U6_STDOUT_BLOCK\n")?;
        }
        for _ in 0..8_192 {
            stderr.write_all(b"U6_STDERR_BLOCK\n")?;
        }
        stdout.flush()?;
        stderr.flush()?;
    }
    let mut stream = UnixStream::connect(Path::new(&socket)).await?;
    stream
        .write_all(format!("READY {}\n", std::process::id()).as_bytes())
        .await?;
    let mut control = [0_u8];
    stream.read_exact(&mut control).await?;
    match control {
        [b'X'] => Ok(()),
        [b'F'] => Err("fixture requested nonzero exit".into()),
        _ => Err("fixture received an unsupported control byte".into()),
    }
}
