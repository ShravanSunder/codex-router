//! Explicit parsing, catalog, picker and native-launch failures.
use std::path::PathBuf;
use thiserror::Error;

/// Sessions command failures.
#[derive(Debug, Error)]
pub enum SessionsCommandError {
    /// Checkout scope is intentionally list-only.
    #[error(
        "--checkout requires --list because the interactive picker supports cwd, repo, and all"
    )]
    InteractiveCheckoutUnsupported,
    /// Interactive picker has not landed yet.
    #[error("sessions interactive picker is not implemented yet; use --list --format json")]
    InteractivePickerNotImplemented,
    /// No matching session was found.
    #[error("no Codex sessions matched the requested filters")]
    NoSessionsMatch,
    /// Interactive picker was canceled.
    #[error("sessions picker canceled")]
    PickerCanceled,
    /// Interactive picker failed.
    #[error("sessions picker failed: {0}")]
    Picker(std::io::Error),
    /// Interactive picker cannot run without a terminal.
    #[error("sessions interactive picker requires a terminal; use --list or --last")]
    InteractiveRequiresTerminal,
    /// Interactive picker cannot render inside the current terminal width.
    #[error("sessions interactive picker requires a wider terminal")]
    TerminalTooNarrow,
    /// Codex failed to launch.
    #[error("failed to launch codex resume command: {0}")]
    CodexLaunch(std::io::Error),
    /// Codex exited unsuccessfully.
    #[error("codex resume command exited with {status}")]
    CodexExit {
        /// Exit status string.
        status: String,
    },
    /// Current provider could not be resolved.
    #[error(
        "sessions --provider current could not find model_provider in CODEX_HOME/codex-router.config.toml or CODEX_HOME/config.toml"
    )]
    CurrentProviderUnavailable,
    /// Config read failed.
    #[error("failed to read Codex config {path}: {source}")]
    ConfigRead {
        /// Config path.
        path: PathBuf,
        /// Source error.
        #[source]
        source: std::io::Error,
    },
    /// CODEX_HOME and HOME were both unavailable.
    #[error("could not locate Codex home; set CODEX_HOME or HOME")]
    CodexHomeUnavailable,
    /// Debug app-server socket override is unsafe or ambiguous.
    #[error("invalid debug app-server socket: {0}")]
    AppServerSocket(String),
    /// Failed to initialize async runtime.
    #[error("failed to initialize sessions runtime: {0}")]
    Runtime(std::io::Error),
    /// SQLite access failed.
    #[error("failed to read Codex sessions state: {0}")]
    Sqlx(sqlx::Error),
    /// Session id from Codex state is unsafe to pass to resume.
    #[error("unsafe Codex session id in state database")]
    UnsafeSessionId,
    /// Direct resume ids must be complete UUIDs.
    #[error("--id requires a complete UUID")]
    InvalidResumeSessionId,
    /// JSON rendering failed.
    #[error("failed to render sessions JSON: {0}")]
    Json(serde_json::Error),
    /// stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),
}
