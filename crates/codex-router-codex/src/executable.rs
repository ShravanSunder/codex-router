//! Managed executable resolution, content identity, and updater projection.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;

const HASH_CHUNK_BYTES: usize = 64 * 1024;

/// Canonical managed executable and its content identity.
#[derive(Clone, Eq, PartialEq)]
pub struct ExecutableIdentity {
    canonical_path: PathBuf,
    digest: [u8; 32],
}

impl std::fmt::Debug for ExecutableIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutableIdentity")
            .field("canonical_path", &self.canonical_path)
            .field("digest", &"[REDACTED]")
            .finish()
    }
}

impl ExecutableIdentity {
    /// Returns the canonical executable path captured with this identity.
    #[must_use]
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }
}

/// Exact official updater command for one resolved managed executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdaterCommandSpec {
    executable: PathBuf,
}

impl UpdaterCommandSpec {
    /// Creates an updater command tied to the captured executable identity.
    #[must_use]
    pub fn new(identity: &ExecutableIdentity) -> Self {
        Self {
            executable: identity.canonical_path.clone(),
        }
    }

    /// Returns the captured executable without another path lookup.
    #[must_use]
    pub fn executable(&self) -> PathBuf {
        self.executable.clone()
    }

    /// Returns the official updater subcommand.
    #[must_use]
    pub fn arguments(&self) -> Vec<OsString> {
        vec![OsString::from("update")]
    }
}

/// Managed executable resolution or observation failure.
#[derive(Debug, Error)]
pub enum ExecutableIdentityError {
    /// Canonicalization or file reading failed.
    #[error("managed Codex executable filesystem operation failed: {0}")]
    Filesystem(#[source] std::io::Error),
    /// Bounded blocking work failed to join.
    #[error("managed Codex executable identity task failed: {0}")]
    Join(#[source] tokio::task::JoinError),
    /// Invoking `--version` failed.
    #[error("managed Codex version command failed: {0}")]
    VersionCommand(#[source] std::io::Error),
    /// `--version` returned failure.
    #[error("managed Codex version command exited with {status}")]
    VersionExit {
        /// Redacted process exit status.
        status: std::process::ExitStatus,
    },
    /// `--version` output was not UTF-8.
    #[error("managed Codex version output was not UTF-8")]
    VersionEncoding,
    /// `--version` output omitted its version token.
    #[error("managed Codex version output was malformed")]
    VersionFormat,
}

/// Resolves and hashes the managed executable on Tokio's blocking pool.
pub async fn executable_identity(
    executable: &Path,
) -> Result<ExecutableIdentity, ExecutableIdentityError> {
    let executable = executable.to_path_buf();
    tokio::task::spawn_blocking(move || hash_executable(executable))
        .await
        .map_err(ExecutableIdentityError::Join)?
}

/// Reads the installed version from the same resolved managed executable.
pub async fn managed_executable_version(
    executable: &Path,
) -> Result<String, ExecutableIdentityError> {
    let output = tokio::process::Command::new(executable)
        .arg("--version")
        .output()
        .await
        .map_err(ExecutableIdentityError::VersionCommand)?;
    if !output.status.success() {
        return Err(ExecutableIdentityError::VersionExit {
            status: output.status,
        });
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_error| ExecutableIdentityError::VersionEncoding)?;
    stdout
        .split_whitespace()
        .nth(1)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .ok_or(ExecutableIdentityError::VersionFormat)
}

fn hash_executable(executable: PathBuf) -> Result<ExecutableIdentity, ExecutableIdentityError> {
    let canonical_path =
        std::fs::canonicalize(executable).map_err(ExecutableIdentityError::Filesystem)?;
    let mut file =
        std::fs::File::open(&canonical_path).map_err(ExecutableIdentityError::Filesystem)?;
    let mut hasher = Sha256::new();
    let mut chunk = [0_u8; HASH_CHUNK_BYTES];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(ExecutableIdentityError::Filesystem)?;
        if read == 0 {
            break;
        }
        hasher.update(chunk.get(..read).ok_or_else(|| {
            ExecutableIdentityError::Filesystem(std::io::Error::other(
                "managed executable hash chunk was out of bounds",
            ))
        })?);
    }
    Ok(ExecutableIdentity {
        canonical_path,
        digest: hasher.finalize().into(),
    })
}
