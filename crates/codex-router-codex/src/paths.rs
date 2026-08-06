//! Native upstream Codex path projections.

use std::path::PathBuf;

/// Paths owned by the normal Codex home rather than router state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexPaths {
    codex_home: PathBuf,
}

impl CodexPaths {
    /// Derives native paths from an already-resolved normal Codex home.
    #[must_use]
    pub fn from_codex_home(codex_home: PathBuf) -> Self {
        Self { codex_home }
    }

    /// Returns the conventional app-server control socket.
    #[must_use]
    pub fn app_server_socket(&self) -> PathBuf {
        self.codex_home
            .join("app-server-control")
            .join("app-server-control.sock")
    }

    /// Returns the managed standalone Codex executable path.
    #[must_use]
    pub fn managed_executable(&self) -> PathBuf {
        self.codex_home
            .join("packages")
            .join("standalone")
            .join("current")
            .join("codex")
    }
}
