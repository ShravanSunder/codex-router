//! Codex profile rendering helpers.

use std::path::Path;
use std::path::PathBuf;

use thiserror::Error;

/// Codex profile for routing the real Codex CLI through the local router.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodexRouterProfile {
    port: u16,
}

impl CodexRouterProfile {
    /// Creates a profile renderer for a loopback router port.
    #[must_use]
    pub const fn new(port: u16) -> Self {
        Self { port }
    }

    /// Renders the Codex custom-provider profile.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            r#"model_provider = "codex-router"

[model_providers.codex-router]
name = "codex-router"
base_url = "http://127.0.0.1:{}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = true
"#,
            self.port
        )
    }
}

/// Preview of a Codex profile write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileDryRun {
    target_path: PathBuf,
    content: String,
    existing_content: Option<String>,
}

impl ProfileDryRun {
    /// Returns the file that would be written.
    #[must_use]
    pub fn target_path(&self) -> PathBuf {
        self.target_path.clone()
    }

    /// Returns the rendered profile content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns existing target content when the target can be read.
    #[must_use]
    pub fn existing_content(&self) -> Option<&str> {
        self.existing_content.as_deref()
    }
}

/// Writes Codex router profile content into an explicit Codex home.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexRouterProfileWriter {
    codex_home: PathBuf,
}

impl CodexRouterProfileWriter {
    /// Creates a writer for a caller-provided Codex home.
    #[must_use]
    pub fn new(codex_home: impl AsRef<Path>) -> Self {
        Self {
            codex_home: codex_home.as_ref().to_path_buf(),
        }
    }

    /// Renders the target path and content without touching the filesystem.
    pub fn dry_run(
        &self,
        profile: &CodexRouterProfile,
    ) -> Result<ProfileDryRun, ProfileWriteError> {
        let target_path = self.target_path();
        let content = profile.render();
        let existing_content = read_existing_profile(&target_path)?;

        Ok(ProfileDryRun {
            target_path,
            content,
            existing_content,
        })
    }

    /// Writes the profile only when explicit approval is provided.
    pub fn write(
        &self,
        profile: &CodexRouterProfile,
        approved: bool,
    ) -> Result<PathBuf, ProfileWriteError> {
        if !approved {
            return Err(ProfileWriteError::ApprovalRequired);
        }
        let preview = self.dry_run(profile)?;
        std::fs::create_dir_all(&self.codex_home).map_err(|source| {
            ProfileWriteError::Filesystem {
                path: self.codex_home.clone(),
                source,
            }
        })?;
        let target_path = preview.target_path();
        std::fs::write(&target_path, preview.content()).map_err(|source| {
            ProfileWriteError::Filesystem {
                path: target_path.clone(),
                source,
            }
        })?;

        Ok(target_path)
    }

    fn target_path(&self) -> PathBuf {
        self.codex_home.join("codex-router.config.toml")
    }
}

fn read_existing_profile(target_path: &Path) -> Result<Option<String>, ProfileWriteError> {
    match std::fs::read_to_string(target_path) {
        Ok(content) => Ok(Some(content)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ProfileWriteError::Filesystem {
            path: target_path.to_path_buf(),
            source,
        }),
    }
}

/// Profile write failure.
#[derive(Debug, Error)]
pub enum ProfileWriteError {
    /// Writing was attempted without explicit approval.
    #[error("explicit approval is required before writing Codex profile files")]
    ApprovalRequired,

    /// Filesystem write failed.
    #[error("profile filesystem error at {path}: {source}")]
    Filesystem {
        /// Target path.
        path: PathBuf,
        /// Source error.
        #[source]
        source: std::io::Error,
    },
}

impl PartialEq for ProfileWriteError {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::ApprovalRequired, Self::ApprovalRequired)
        )
    }
}
