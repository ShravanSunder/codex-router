//! Secret-store model types.

use std::path::PathBuf;

use thiserror::Error;

/// Secret key used as a safe file stem under the router-owned root.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SecretKey(String);

impl SecretKey {
    /// Builds a secret key from a conservative file-safe string.
    pub fn new(value: impl Into<String>) -> Result<Self, SecretStoreError> {
        let value = value.into();
        if value.is_empty() || !value.chars().all(is_allowed_key_char) {
            return Err(SecretStoreError::InvalidSecretKey { value });
        }

        Ok(Self(value))
    }

    /// Returns the key's file-safe representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Secret-store operation error.
#[derive(Debug, Error)]
pub enum SecretStoreError {
    /// Secret key contained unsupported characters.
    #[error("invalid secret key: {value}")]
    InvalidSecretKey {
        /// Rejected key.
        value: String,
    },

    /// Router root must never live inside Codex home.
    #[error("secret store root must not use .codex path: {path}")]
    CodexHomePath {
        /// Rejected path.
        path: PathBuf,
    },

    /// Symlinks are rejected for secret-store paths.
    #[error("secret store refuses symlink path: {path}")]
    SymlinkPath {
        /// Rejected path.
        path: PathBuf,
    },

    /// Filesystem operation failed.
    #[error("secret store filesystem error at {path}: {source}")]
    Filesystem {
        /// Path being accessed.
        path: PathBuf,
        /// Source IO error.
        #[source]
        source: std::io::Error,
    },
}

fn is_allowed_key_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
}
