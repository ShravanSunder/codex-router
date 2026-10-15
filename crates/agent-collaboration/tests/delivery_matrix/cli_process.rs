//! Owned CLI child capture: concurrent pipes and explicit wait on every returned failure.
use super::proof_context::ProofResult;
use std::{
    fmt,
    future::Future,
    io,
    process::{ExitStatus, Output, Stdio},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt as _,
    process::{Child, Command},
};

#[derive(Debug)]
struct CliProcessError {
    pid: Option<u32>,
    cause: io::Error,
    termination_error: Option<io::Error>,
    wait_result: io::Result<ExitStatus>,
}

impl fmt::Display for CliProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "CLI child observation failed: {}; pid={:?}; termination={:?}; wait={:?}",
            self.cause, self.pid, self.termination_error, self.wait_result
        )
    }
}

impl std::error::Error for CliProcessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

pub(super) async fn run_cli_output(
    command: Command,
    observation_timeout: Duration,
) -> ProofResult<Output> {
    run_until_deadline(command, tokio::time::sleep(observation_timeout)).await
}

async fn run_until_deadline(
    mut command: Command,
    deadline: impl Future<Output = ()>,
) -> ProofResult<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.kill_on_drop(true);
    let mut child = command.spawn()?;
    let pid = child.id();
    let pipes = child.stdout.take().zip(child.stderr.take());
    let Some((mut stdout, mut stderr)) = pipes else {
        let cause = io::Error::other("CLI child did not expose both requested output pipes");
        return Err(stop_after_failure(child, pid, cause).await.into());
    };
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let captured = {
        let capture = async {
            let (status, _, _) = tokio::try_join!(
                child.wait(),
                stdout.read_to_end(&mut stdout_bytes),
                stderr.read_to_end(&mut stderr_bytes),
            )?;
            Ok::<ExitStatus, io::Error>(status)
        };
        tokio::pin!(deadline);
        tokio::select! {
            result = capture => result,
            () = &mut deadline => Err(io::Error::new(io::ErrorKind::TimedOut,
                "CLI child observation deadline elapsed")),
        }
    };
    // No reader tasks exist. Close both owned pipes after their local futures settle/cancel.
    drop(stdout);
    drop(stderr);
    match captured {
        Ok(status) => Ok(Output {
            status,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
        }),
        Err(cause) => Err(stop_after_failure(child, pid, cause).await.into()),
    }
}

async fn stop_after_failure(
    mut child: Child,
    pid: Option<u32>,
    cause: io::Error,
) -> CliProcessError {
    let termination_error = child.start_kill().err();
    // A signal request alone is not settlement. Always explicitly wait, even if it failed.
    let wait_result = child.wait().await;
    CliProcessError {
        pid,
        cause,
        termination_error,
        wait_result,
    }
}

#[cfg(test)]
#[path = "cli_process_tests.rs"]
mod tests;
