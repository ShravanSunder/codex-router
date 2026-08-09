//! Native macOS launch-session policy for Codex Desktop daemon reuse.

use std::ffi::OsString;
use std::path::PathBuf;
use thiserror::Error;
use tokio::process::Command;

/// Exact `launchctl` command that enables the conventional local app-server daemon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopLaunchPolicyCommand {
    executable: PathBuf,
    arguments: Vec<OsString>,
}

impl DesktopLaunchPolicyCommand {
    /// Projects the native macOS launch-session environment mutation.
    #[must_use]
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            arguments: vec![
                OsString::from("setenv"),
                OsString::from("CODEX_APP_SERVER_USE_LOCAL_DAEMON"),
                OsString::from("1"),
            ],
        }
    }

    /// Returns the exact launchctl executable.
    #[must_use]
    pub fn executable(&self) -> PathBuf {
        self.executable.clone()
    }

    /// Returns the exact launchctl arguments.
    #[must_use]
    pub fn arguments(&self) -> Vec<OsString> {
        self.arguments.clone()
    }

    /// Applies the launch-session policy and waits for its terminal result.
    pub async fn apply(&self) -> Result<(), DesktopLaunchPolicyError> {
        let status = Command::new(&self.executable)
            .args(&self.arguments)
            .status()
            .await
            .map_err(DesktopLaunchPolicyError::Spawn)?;
        if status.success() {
            Ok(())
        } else {
            Err(DesktopLaunchPolicyError::Rejected)
        }
    }
}

/// Desktop launch-session policy could not be enabled.
#[derive(Debug, Error)]
pub enum DesktopLaunchPolicyError {
    /// The native launchctl command could not be started.
    #[error("failed starting launchctl for Codex Desktop attachment: {0}")]
    Spawn(#[source] std::io::Error),
    /// launchctl rejected the requested login-session environment value.
    #[error("launchctl rejected Codex Desktop local-daemon attachment")]
    Rejected,
}
