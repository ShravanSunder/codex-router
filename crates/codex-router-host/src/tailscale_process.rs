//! Bounded Tailscale CLI calls and ownership of the foreground Serve child.

use crate::ChildCommandSpec;
use std::{ffi::OsString, io, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, BufReader},
    process::{Child, Command},
    sync::oneshot,
    task::JoinSet,
};

const MAX_OUTPUT_BYTES: u64 = 64 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const SERVE_START_TIMEOUT: Duration = Duration::from_secs(20);
const SERVE_STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
#[path = "tailscale_process/tests.rs"]
mod tests;

pub(super) struct TailscaleCommand {
    specification: ChildCommandSpec,
}

impl Default for TailscaleCommand {
    fn default() -> Self {
        Self {
            specification: ChildCommandSpec::new(PathBuf::from("tailscale")),
        }
    }
}

impl TailscaleCommand {
    #[cfg(test)]
    pub(super) fn fixture(specification: ChildCommandSpec) -> Self {
        Self { specification }
    }

    fn command(&self) -> Command {
        let mut command = self.specification.command();
        command
            .env_remove(crate::inherited_lock_environment())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    pub(super) async fn capture(&self, arguments: &[&str]) -> io::Result<Vec<u8>> {
        // Only fixed read-only command names are safe diagnostic context. Never
        // format arbitrary argv, captured output, or private daemon metadata.
        let command_name = match arguments {
            ["status", "--json"] => "tailscale status --json",
            ["serve", "status", "--json"] => "tailscale serve status --json",
            _ => "Tailscale finite command",
        };
        let failure = |phase: &str, error: io::Error| {
            io::Error::new(
                error.kind(),
                format!("{command_name} during {phase}: {error}"),
            )
        };
        let mut child = self.command().args(arguments).spawn().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("{command_name} during spawn: could not run Tailscale; install its CLI and verify operator access"),
            )
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| failure("stdout setup", io::Error::other("missing Tailscale stdout")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| failure("stderr setup", io::Error::other("missing Tailscale stderr")))?;
        let mut stdout_finished = false;
        let mut stderr_finished = false;
        let mut exit_finished = false;
        let completed = tokio::time::timeout(COMMAND_TIMEOUT, async {
            tokio::try_join!(
                async {
                    let result = read_bounded(stdout).await;
                    stdout_finished = true;
                    result.map_err(|error| failure("stdout capture", error))
                },
                async {
                    let result = read_bounded(stderr).await;
                    stderr_finished = true;
                    result.map_err(|error| failure("stderr capture", error))
                },
                async {
                    let result = child.wait().await;
                    exit_finished = true;
                    result.map_err(|error| failure("process exit", error))
                }
            )
        })
        .await;
        match completed {
            Ok(Ok((stdout, _, status))) if status.success() => Ok(stdout),
            result => {
                let _reaped = child.kill().await;
                match result {
                    Ok(Err(error)) => Err(error),
                    Ok(Ok(_)) => Err(failure(
                        "process exit",
                        io::Error::other(
                            "command failed; check daemon status and operator permissions",
                        ),
                    )),
                    Err(_) => Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "{command_name} timed out during finite capture (stdout {}, stderr {}, exit {}); no command was retried",
                            if stdout_finished {
                                "complete"
                            } else {
                                "pending"
                            },
                            if stderr_finished {
                                "complete"
                            } else {
                                "pending"
                            },
                            if exit_finished { "complete" } else { "pending" },
                        ),
                    )),
                }
            }
        }
    }

    pub(super) fn spawn_serve(
        &self,
        port: std::num::NonZeroU16,
        backend: std::net::SocketAddr,
        node: &str,
    ) -> io::Result<OwnedServeProcess> {
        if !backend.ip().is_loopback() || backend.port() == 0 {
            return Err(io::Error::other(
                "Tailscale Serve requires an actual bound loopback MCP address",
            ));
        }
        let mut child = self
            .command()
            .args(serve_arguments(port, backend))
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing Serve stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("missing Serve stderr"))?;
        let (advertised, advertisement) = oneshot::channel();
        let prefix = format!("https://{node}");
        let mut readers = JoinSet::new();
        readers.spawn(async move {
            let mut output = BufReader::new(stdout.take(MAX_OUTPUT_BYTES + 1));
            let mut total = 0;
            let mut advertised = Some(advertised);
            loop {
                let mut line = String::new();
                let read = output.read_line(&mut line).await?;
                if read == 0 {
                    return Ok(());
                }
                total += read;
                if total as u64 > MAX_OUTPUT_BYTES {
                    return Err(io::Error::other(
                        "Tailscale Serve output exceeded its bound",
                    ));
                }
                if line.contains(&prefix)
                    && let Some(sender) = advertised.take()
                {
                    let _delivered = sender.send(());
                }
            }
        });
        readers.spawn(async move { read_bounded(stderr).await.map(|_| ()) });
        Ok(OwnedServeProcess {
            child,
            readers,
            advertisement: Some(advertisement),
        })
    }
}

pub(super) fn serve_arguments(
    port: std::num::NonZeroU16,
    backend: std::net::SocketAddr,
) -> Vec<OsString> {
    vec![
        "serve".into(),
        "--bg=false".into(),
        format!("--https={port}").into(),
        "--set-path=/".into(),
        format!("http://{backend}").into(),
    ]
}

async fn read_bounded<TRead: AsyncRead + Unpin>(reader: TRead) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader
        .take(MAX_OUTPUT_BYTES + 1)
        .read_to_end(&mut output)
        .await?;
    if output.len() as u64 > MAX_OUTPUT_BYTES {
        return Err(io::Error::other(
            "Tailscale command output exceeded its bound",
        ));
    }
    Ok(output)
}

pub(super) struct OwnedServeProcess {
    child: Child,
    readers: JoinSet<io::Result<()>>,
    advertisement: Option<oneshot::Receiver<()>>,
}

impl OwnedServeProcess {
    pub(super) async fn advertised(&mut self) -> io::Result<()> {
        let advertisement = self
            .advertisement
            .take()
            .ok_or_else(|| io::Error::other("Serve startup was already observed"))?;
        tokio::time::timeout(SERVE_START_TIMEOUT, async {
            tokio::select! {
                ready = advertisement => ready.map_err(|_| io::Error::other("Tailscale Serve stopped before publishing its private URL")),
                failure = self.failure() => Err(failure),
            }
        }).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Tailscale Serve did not publish its private URL before the startup deadline"))?
    }

    pub(super) async fn failure(&mut self) -> io::Error {
        loop {
            tokio::select! {
                status = self.child.wait() => return match status {
                    Ok(status) => io::Error::other(format!("Tailscale Serve exited unexpectedly ({status})")),
                    Err(error) => error,
                },
                output = self.readers.join_next(), if !self.readers.is_empty() => match output {
                    Some(Ok(Ok(()))) => {},
                    Some(Ok(Err(error))) => return error,
                    Some(Err(error)) => return io::Error::other(error),
                    None => {},
                }
            }
        }
    }

    pub(super) async fn stop(&mut self) -> io::Result<()> {
        let mut failure = None;
        if self.child.try_wait()?.is_none() {
            if let Some(pid) = self
                .child
                .id()
                .and_then(|pid| i32::try_from(pid).ok())
                .and_then(rustix::process::Pid::from_raw)
            {
                // Tailscale's foreground CLI handles SIGINT and closes its IPN watcher.
                if let Err(error) = rustix::process::kill_process(pid, rustix::process::Signal::INT)
                {
                    failure = Some(io::Error::other(error));
                }
            }
            if tokio::time::timeout(SERVE_STOP_TIMEOUT, self.child.wait())
                .await
                .is_err()
            {
                self.child.kill().await?;
                failure.get_or_insert_with(|| {
                    io::Error::other("Tailscale Serve required forced termination")
                });
            }
        }
        // Explicitly reap even when failure() already observed an exit.
        self.child.wait().await?;
        if tokio::time::timeout(SERVE_STOP_TIMEOUT, async {
            while let Some(result) = self.readers.join_next().await {
                result.map_err(io::Error::other)??;
            }
            Ok::<(), io::Error>(())
        })
        .await
        .map_err(|_| io::Error::other("Tailscale output readers did not settle"))?
        .is_err()
        {
            failure.get_or_insert_with(|| io::Error::other("Tailscale output reader failed"));
        }
        failure.map_or(Ok(()), Err)
    }

    #[cfg(test)]
    pub(super) fn process_id(&self) -> Option<u32> {
        self.child.id()
    }
}
