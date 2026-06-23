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
env_key = "CODEX_ROUTER_TOKEN"
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
    preview_token: String,
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

    /// Returns the preview token required to confirm this exact write.
    #[must_use]
    pub fn preview_token(&self) -> &str {
        &self.preview_token
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
        let preview_token = preview_token(&target_path, existing_content.as_deref(), &content);

        Ok(ProfileDryRun {
            target_path,
            content,
            existing_content,
            preview_token,
        })
    }

    /// Writes the profile only when explicit approval and preview confirmation are provided.
    pub fn write(
        &self,
        profile: &CodexRouterProfile,
        approved: bool,
        supplied_preview_token: Option<&str>,
    ) -> Result<PathBuf, ProfileWriteError> {
        if !approved {
            return Err(ProfileWriteError::ApprovalRequired);
        }
        let preview = self.dry_run(profile)?;
        let supplied_preview_token =
            supplied_preview_token.ok_or(ProfileWriteError::PreviewTokenRequired)?;
        if supplied_preview_token != preview.preview_token() {
            return Err(ProfileWriteError::PreviewTokenMismatch);
        }
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

fn preview_token(
    target_path: &Path,
    existing_content: Option<&str>,
    proposed_content: &str,
) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in target_path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash ^= 0xff;
    hash = hash.wrapping_mul(FNV_PRIME);
    if let Some(existing_content) = existing_content {
        hash ^= 0x01;
        hash = hash.wrapping_mul(FNV_PRIME);
        for byte in existing_content.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    } else {
        hash ^= 0x00;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash ^= 0xfe;
    hash = hash.wrapping_mul(FNV_PRIME);
    for byte in proposed_content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    format!("{hash:016x}")
}

/// Profile write failure.
#[derive(Debug, Error)]
pub enum ProfileWriteError {
    /// Writing was attempted without explicit approval.
    #[error("explicit approval is required before writing Codex profile files")]
    ApprovalRequired,

    /// Writing was attempted without a preview token.
    #[error("profile preview token is required before writing Codex profile files")]
    PreviewTokenRequired,

    /// The supplied preview token did not match the current target and content.
    #[error("profile preview token does not match the current Codex profile preview")]
    PreviewTokenMismatch,

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
                | (Self::PreviewTokenRequired, Self::PreviewTokenRequired)
                | (Self::PreviewTokenMismatch, Self::PreviewTokenMismatch)
        )
    }
}
