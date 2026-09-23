//! Expected-exit identity and bounded app-server shutdown progression.

use std::time::Duration;

use thiserror::Error;

use crate::ProcessGroupError;
use crate::managed_app_server::AppServerChild;

/// Grace period before SIGKILL escalation.
pub const APP_SERVER_GRACE_PERIOD: Duration = Duration::from_millis(750);
/// Total app-server shutdown observation bound, including forced reap.
pub const APP_SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Injected shutdown boundaries for deterministic process lifecycle tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppServerShutdownDeadlines {
    kill_after: Duration,
    total: Duration,
}

impl AppServerShutdownDeadlines {
    /// Returns the production subsecond grace and bounded reap contract.
    #[must_use]
    pub const fn production() -> Self {
        Self {
            kill_after: APP_SERVER_GRACE_PERIOD,
            total: APP_SERVER_SHUTDOWN_TIMEOUT,
        }
    }

    /// Creates valid shorter boundaries for deterministic process fixtures.
    #[must_use]
    pub fn new(kill_after: Duration, total: Duration) -> Option<Self> {
        if kill_after < total {
            Some(Self { kill_after, total })
        } else {
            None
        }
    }
}

/// Terminal result of the one shared app-server shutdown routine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownOutcome {
    /// Child exited after SIGTERM without force escalation.
    Graceful,
    /// Child exited after the SIGKILL backstop fired.
    Killed,
    /// Total bound expired while the exact child remained retained.
    TimedOutStillRunning,
}

/// Next deterministic shutdown-policy action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownAction {
    /// Send the first and only SIGTERM.
    SendTerminate,
    /// Await child exit or the next force boundary.
    Wait,
    /// Send the one SIGKILL escalation.
    SendKill,
    /// Return a terminal result for a reaped child.
    Complete(ShutdownOutcome),
    /// Retain the still-running child and progress without another signal.
    TimedOutStillRunning,
}

/// Exact-child expected-exit token and signal progress.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedExit {
    child_id: u32,
    term_sent: bool,
    kill_sent: bool,
}

impl ExpectedExit {
    /// Begins shutdown progress for one exact retained child.
    #[must_use]
    pub const fn new(child_id: u32) -> Self {
        Self {
            child_id,
            term_sent: false,
            kill_sent: false,
        }
    }

    /// Returns the next action and records signal progression before I/O.
    pub fn next_action(&mut self, elapsed: Duration, child_running: bool) -> ShutdownAction {
        self.next_action_with_deadlines(
            elapsed,
            child_running,
            AppServerShutdownDeadlines::production(),
        )
    }

    fn next_action_with_deadlines(
        &mut self,
        elapsed: Duration,
        child_running: bool,
        deadlines: AppServerShutdownDeadlines,
    ) -> ShutdownAction {
        if !child_running {
            return ShutdownAction::Complete(if self.kill_sent {
                ShutdownOutcome::Killed
            } else {
                ShutdownOutcome::Graceful
            });
        }
        if elapsed >= deadlines.total {
            return ShutdownAction::TimedOutStillRunning;
        }
        if !self.term_sent {
            self.term_sent = true;
            return ShutdownAction::SendTerminate;
        }
        if elapsed >= deadlines.kill_after && !self.kill_sent {
            self.kill_sent = true;
            return ShutdownAction::SendKill;
        }
        ShutdownAction::Wait
    }

    /// Returns the exact retained child ID.
    #[must_use]
    pub const fn child_id(&self) -> u32 {
        self.child_id
    }

    /// Returns whether SIGTERM was already recorded and sent.
    #[must_use]
    pub const fn term_sent(&self) -> bool {
        self.term_sent
    }

    /// Returns whether the SIGKILL escalation was recorded and sent.
    #[must_use]
    pub const fn kill_sent(&self) -> bool {
        self.kill_sent
    }
}

impl AppServerChild {
    /// Returns retained expected-exit progress, when shutdown began.
    #[must_use]
    pub const fn expected_exit(&self) -> Option<&ExpectedExit> {
        self.expected_exit.as_ref()
    }

    /// Runs or resumes the bounded app-server shutdown routine.
    pub async fn shutdown(&mut self) -> Result<ShutdownOutcome, AppServerShutdownError> {
        self.shutdown_with_deadlines(AppServerShutdownDeadlines::production())
            .await
    }

    /// Runs the same shutdown machine with injected valid fixture deadlines.
    pub async fn shutdown_with_deadlines(
        &mut self,
        deadlines: AppServerShutdownDeadlines,
    ) -> Result<ShutdownOutcome, AppServerShutdownError> {
        if let Some(expected_exit) = self.expected_exit.as_ref() {
            let kill_sent = expected_exit.kill_sent();
            return match self.process.try_wait()? {
                Some(_status) if kill_sent => Ok(ShutdownOutcome::Killed),
                Some(_status) => Ok(ShutdownOutcome::Graceful),
                None if kill_sent => {
                    self.process.send_group_kill()?;
                    Ok(ShutdownOutcome::TimedOutStillRunning)
                }
                None => Ok(ShutdownOutcome::TimedOutStillRunning),
            };
        }

        if self.process.try_wait()?.is_some() {
            return Ok(ShutdownOutcome::Graceful);
        }
        let mut expected_exit = ExpectedExit::new(self.process.process_id());
        let first_action =
            expected_exit.next_action_with_deadlines(Duration::ZERO, true, deadlines);
        self.expected_exit = Some(expected_exit);
        if first_action != ShutdownAction::SendTerminate {
            return Err(AppServerShutdownError::InvalidInitialAction);
        }
        self.process.send_terminate()?;
        if let Ok(result) = tokio::time::timeout(deadlines.kill_after, self.process.wait()).await {
            let _status = result?;
            return Ok(ShutdownOutcome::Graceful);
        }
        let expected_exit = self
            .expected_exit
            .as_mut()
            .ok_or(AppServerShutdownError::MissingProgress)?;
        let force_action =
            expected_exit.next_action_with_deadlines(deadlines.kill_after, true, deadlines);
        if force_action != ShutdownAction::SendKill {
            return Err(AppServerShutdownError::InvalidForceAction);
        }
        self.process.send_group_kill()?;

        let forced_wait = deadlines.total.saturating_sub(deadlines.kill_after);
        match tokio::time::timeout(forced_wait, self.process.wait()).await {
            Ok(result) => {
                let _status = result?;
                Ok(ShutdownOutcome::Killed)
            }
            Err(_elapsed) => Ok(ShutdownOutcome::TimedOutStillRunning),
        }
    }

    /// Waits for an unexpected or expected retained-child exit.
    pub async fn wait_for_exit(&mut self) -> Result<std::process::ExitStatus, ProcessGroupError> {
        self.process.wait().await
    }
}

/// Exact-child shutdown failure.
#[derive(Debug, Error)]
pub enum AppServerShutdownError {
    /// Process signal or wait failed.
    #[error(transparent)]
    Process(#[from] ProcessGroupError),
    /// Internal expected-exit state was absent after the first signal.
    #[error("app-server shutdown progress was lost")]
    MissingProgress,
    /// New shutdown progress did not request SIGTERM first.
    #[error("app-server shutdown did not begin with SIGTERM")]
    InvalidInitialAction,
    /// Grace expiry did not produce the SIGKILL action.
    #[error("app-server shutdown did not reach its force action")]
    InvalidForceAction,
}
