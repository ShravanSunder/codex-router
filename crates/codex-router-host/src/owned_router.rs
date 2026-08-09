//! Retained owned-router child and bounded SIGTERM-only shutdown.

use std::time::Duration;

use thiserror::Error;

use crate::ProcessGroupChild;
use crate::ProcessGroupError;

/// Repository-owned SIGTERM-only router shutdown bound.
pub const ROUTER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether the compatible router is external or retained by this host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouterOwnership {
    /// Compatible process observed only through its health endpoint.
    External,
    /// Exact child handle retained by this host.
    Owned,
}

/// Terminal owned-router shutdown result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouterShutdownOutcome {
    /// Exact retained child exited after SIGTERM or was already exited.
    Graceful,
    /// Ten-second bound elapsed; child remains retained and receives no force signal.
    TimedOutStillRunning,
}

/// Retained router child with SIGTERM progression.
pub struct RouterChild {
    process: ProcessGroupChild,
    term_sent: bool,
}

impl RouterChild {
    /// Spawns the existing router command in an isolated process group.
    pub fn spawn(command: &mut tokio::process::Command) -> Result<Self, ProcessGroupError> {
        Ok(Self {
            process: ProcessGroupChild::spawn(command)?,
            term_sent: false,
        })
    }

    /// Returns the exact retained router PID.
    #[must_use]
    pub fn process_id(&self) -> u32 {
        self.process.process_id()
    }

    /// Sends one SIGTERM and waits without any SIGKILL policy.
    pub async fn shutdown(&mut self) -> Result<RouterShutdownOutcome, RouterShutdownError> {
        if self.process.try_wait()?.is_some() {
            return Ok(RouterShutdownOutcome::Graceful);
        }
        if self.term_sent {
            return Ok(RouterShutdownOutcome::TimedOutStillRunning);
        }
        self.term_sent = true;
        self.process.send_terminate()?;
        match tokio::time::timeout(ROUTER_SHUTDOWN_TIMEOUT, self.process.wait()).await {
            Ok(result) => {
                let _status = result?;
                Ok(RouterShutdownOutcome::Graceful)
            }
            Err(_elapsed) => Ok(RouterShutdownOutcome::TimedOutStillRunning),
        }
    }

    /// Waits for the retained router child to exit unexpectedly or intentionally.
    pub async fn wait_for_exit(&mut self) -> Result<std::process::ExitStatus, ProcessGroupError> {
        self.process.wait().await
    }
}

/// Exact owned-router stop failure.
#[derive(Debug, Error)]
pub enum RouterShutdownError {
    /// Process signal or wait failed.
    #[error(transparent)]
    Process(#[from] ProcessGroupError),
}
