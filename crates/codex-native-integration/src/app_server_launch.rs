//! Managed app-server launch projection.

use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

use crate::CodexPaths;
use crate::CodexRouterProfile;

/// Exact executable and arguments for one managed app-server child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppServerCommandSpec {
    executable: PathBuf,
    root_overrides: Vec<String>,
    app_server_socket: PathBuf,
    remote_control: bool,
}

impl AppServerCommandSpec {
    /// Builds the native app-server command from the shared router projection.
    #[must_use]
    pub fn new(paths: &CodexPaths, profile: &CodexRouterProfile, app_server_socket: &Path) -> Self {
        Self {
            executable: paths.managed_executable(),
            root_overrides: profile.root_overrides(),
            app_server_socket: app_server_socket.to_owned(),
            remote_control: true,
        }
    }
    #[must_use]
    pub fn with_debug_profile(mut self, profile: &crate::DebugCodexProfile) -> Self {
        self.root_overrides = profile.root_overrides();
        self.remote_control = false;
        self
    }

    /// The disabled marker is consumed by native CLI before its worker threads start.
    #[must_use]
    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        if self.remote_control {
            Vec::new()
        } else {
            vec![(
                "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED".into(),
                "1".into(),
            )]
        }
    }
    #[must_use]
    pub const fn with_remote_control(mut self, enabled: bool) -> Self {
        self.remote_control = enabled;
        self
    }

    /// Returns the managed executable.
    #[must_use]
    pub fn executable(&self) -> PathBuf {
        self.executable.clone()
    }

    /// Returns the exact child arguments.
    #[must_use]
    pub fn arguments(&self) -> Vec<OsString> {
        let mut arguments = Vec::new();
        for root_override in &self.root_overrides {
            arguments.push(OsString::from("-c"));
            arguments.push(OsString::from(root_override));
        }
        arguments.push(OsString::from("app-server"));
        if self.remote_control {
            arguments.push(OsString::from("--remote-control"));
        }
        arguments.extend([
            OsString::from("--listen"),
            OsString::from(format!("unix://{}", self.app_server_socket.display())),
        ]);
        arguments
    }
}
