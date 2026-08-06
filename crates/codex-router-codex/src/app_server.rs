//! Managed app-server child command projection.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::CodexPaths;
use crate::CodexRouterProfile;

/// Exact executable and arguments for one managed app-server child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppServerCommandSpec {
    executable: PathBuf,
    arguments: Vec<OsString>,
}

impl AppServerCommandSpec {
    /// Builds the native app-server command from the shared router projection.
    #[must_use]
    pub fn new(paths: &CodexPaths, profile: &CodexRouterProfile) -> Self {
        let mut arguments = Vec::new();
        for root_override in profile.root_overrides() {
            arguments.push(OsString::from("-c"));
            arguments.push(OsString::from(root_override));
        }
        arguments.extend([
            OsString::from("app-server"),
            OsString::from("--remote-control"),
            OsString::from("--listen"),
            OsString::from("unix://"),
        ]);
        Self {
            executable: paths.managed_executable(),
            arguments,
        }
    }

    /// Returns the managed executable.
    #[must_use]
    pub fn executable(&self) -> PathBuf {
        self.executable.clone()
    }

    /// Returns the exact child arguments.
    #[must_use]
    pub fn arguments(&self) -> Vec<OsString> {
        self.arguments.clone()
    }
}
