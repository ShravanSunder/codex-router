//! Local Router token persistence shared by the CLI and Host.

use std::fmt::Write as _;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use codex_router_core::ids::TokenGeneration;
use codex_router_core::local_auth::LocalRouterAuth;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::redaction::SecretString;
use fs2::FileExt;
use thiserror::Error;

use crate::SecretStore;
use crate::model::SecretKey;
use crate::model::SecretStoreError;

const LOCAL_TOKEN_CREATION_LOCK_FILE: &str = ".token.lock";

/// Error from local Router token persistence.
#[derive(Debug, Error)]
pub enum LocalRouterTokenError {
    /// Secret-store operation failed.
    #[error("token secret-store error: {0}")]
    SecretStore(#[from] SecretStoreError),

    /// Token generation metadata could not be parsed.
    #[error("invalid token generation metadata: {value}")]
    InvalidGeneration {
        /// Raw stored value.
        value: String,
    },

    /// OS random source failed.
    #[error("failed to generate local router token: {0}")]
    Random(std::io::Error),

    /// Local-token creation lock could not be opened or acquired.
    #[error("local token creation lock error at {path}: {source}")]
    TokenCreationLock {
        /// Lock file path.
        path: PathBuf,
        /// Lock operation failure.
        source: std::io::Error,
    },
}

/// Service for local Router token persistence and rotation.
#[derive(Clone, Debug)]
pub struct LocalRouterTokenService<S>
where
    S: SecretStore,
{
    store: S,
}

impl<S> LocalRouterTokenService<S>
where
    S: SecretStore,
{
    /// Builds a token service.
    #[must_use]
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Rotates the local token using OS randomness.
    pub fn rotate(&self) -> Result<LocalRouterTokenRecord, LocalRouterTokenError> {
        let token = generate_token()?;
        self.rotate_with_token(token)
    }

    /// Creates an initial local token only if one does not already exist.
    pub fn ensure_local_token(
        &self,
        secret_root: impl AsRef<Path>,
    ) -> Result<LocalRouterTokenRecord, LocalRouterTokenError> {
        let _creation_lock = LocalTokenCreationLock::acquire(secret_root.as_ref())?;
        match self.load_current_optional()? {
            Some(current) => Ok(current),
            None => self.rotate(),
        }
    }

    /// Rotates the local token using a caller-supplied token value.
    pub fn rotate_with_token(
        &self,
        token: impl Into<String>,
    ) -> Result<LocalRouterTokenRecord, LocalRouterTokenError> {
        let previous = self.load_current_optional()?;
        let generation = previous
            .as_ref()
            .map(LocalRouterTokenRecord::generation)
            .map(TokenGeneration::next)
            .unwrap_or_else(|| TokenGeneration::new(1));
        if let Some(previous) = previous {
            self.write_previous(&previous)?;
        }
        let token = SecretString::new(token.into());
        self.store
            .write_secret(&local_token_key()?, &token)
            .map_err(LocalRouterTokenError::SecretStore)?;
        self.store
            .write_secret(
                &local_generation_key()?,
                &SecretString::new(generation.as_u64().to_string()),
            )
            .map_err(LocalRouterTokenError::SecretStore)?;

        Ok(LocalRouterTokenRecord::new(token, generation))
    }

    /// Loads the current token record.
    pub fn load_current(&self) -> Result<LocalRouterTokenRecord, LocalRouterTokenError> {
        self.load_current_optional()?
            .ok_or_else(|| LocalRouterTokenError::InvalidGeneration {
                value: "<missing>".to_owned(),
            })
    }

    /// Loads the current and previous-token auth snapshot.
    pub fn load_auth(&self) -> Result<LocalRouterAuth, LocalRouterTokenError> {
        let current = self.load_current()?;
        let previous = self.load_previous()?;

        Ok(LocalRouterAuth::new(current, previous))
    }

    fn load_current_optional(
        &self,
    ) -> Result<Option<LocalRouterTokenRecord>, LocalRouterTokenError> {
        let token = match self.store.read_secret(&local_token_key()?) {
            Ok(token) => token,
            Err(SecretStoreError::Filesystem { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(LocalRouterTokenError::SecretStore(error)),
        };
        let generation =
            self.read_generation()?
                .ok_or_else(|| LocalRouterTokenError::InvalidGeneration {
                    value: "<missing>".to_owned(),
                })?;

        Ok(Some(LocalRouterTokenRecord::new(token, generation)))
    }

    fn read_generation(&self) -> Result<Option<TokenGeneration>, LocalRouterTokenError> {
        match self.store.read_secret(&local_generation_key()?) {
            Ok(value) => parse_generation(value.expose_secret()).map(Some),
            Err(SecretStoreError::Filesystem { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            Err(error) => Err(LocalRouterTokenError::SecretStore(error)),
        }
    }

    fn load_previous(&self) -> Result<Vec<LocalRouterTokenRecord>, LocalRouterTokenError> {
        let token = match self.store.read_secret(&previous_local_token_key()?) {
            Ok(token) => token,
            Err(SecretStoreError::Filesystem { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(Vec::new());
            }
            Err(error) => return Err(LocalRouterTokenError::SecretStore(error)),
        };
        let generation = match self.store.read_secret(&previous_local_generation_key()?) {
            Ok(generation) => parse_generation(generation.expose_secret())?,
            Err(error) => return Err(LocalRouterTokenError::SecretStore(error)),
        };

        Ok(vec![LocalRouterTokenRecord::new(token, generation)])
    }

    fn write_previous(
        &self,
        previous: &LocalRouterTokenRecord,
    ) -> Result<(), LocalRouterTokenError> {
        self.store
            .write_secret(&previous_local_token_key()?, previous.token())
            .map_err(LocalRouterTokenError::SecretStore)?;
        self.store
            .write_secret(
                &previous_local_generation_key()?,
                &SecretString::new(previous.generation().as_u64().to_string()),
            )
            .map_err(LocalRouterTokenError::SecretStore)
    }
}

struct LocalTokenCreationLock {
    file: File,
}

impl LocalTokenCreationLock {
    fn acquire(secret_root: &Path) -> Result<Self, LocalRouterTokenError> {
        let path = secret_root.join(LOCAL_TOKEN_CREATION_LOCK_FILE);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(LocalRouterTokenError::TokenCreationLock {
                    path,
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "lock path must not be a symbolic link",
                    ),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(LocalRouterTokenError::TokenCreationLock { path, source });
            }
        }

        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file =
            options
                .open(&path)
                .map_err(|source| LocalRouterTokenError::TokenCreationLock {
                    path: path.clone(),
                    source,
                })?;
        FileExt::lock_exclusive(&file)
            .map_err(|source| LocalRouterTokenError::TokenCreationLock { path, source })?;

        Ok(Self { file })
    }
}

impl Drop for LocalTokenCreationLock {
    fn drop(&mut self) {
        let _result = FileExt::unlock(&self.file);
    }
}

fn parse_generation(value: &str) -> Result<TokenGeneration, LocalRouterTokenError> {
    let generation =
        value
            .parse::<u64>()
            .map_err(|_| LocalRouterTokenError::InvalidGeneration {
                value: value.to_owned(),
            })?;

    Ok(TokenGeneration::new(generation))
}

fn local_token_key() -> Result<SecretKey, SecretStoreError> {
    SecretKey::new("local_router_token")
}

fn local_generation_key() -> Result<SecretKey, SecretStoreError> {
    SecretKey::new("local_router_token_generation")
}

fn previous_local_token_key() -> Result<SecretKey, SecretStoreError> {
    SecretKey::new("local_router_token_previous")
}

fn previous_local_generation_key() -> Result<SecretKey, SecretStoreError> {
    SecretKey::new("local_router_token_previous_generation")
}

fn generate_token() -> Result<String, LocalRouterTokenError> {
    let mut file = std::fs::File::open("/dev/urandom").map_err(LocalRouterTokenError::Random)?;
    let mut bytes = [0_u8; 32];
    file.read_exact(&mut bytes)
        .map_err(LocalRouterTokenError::Random)?;

    let mut token = String::with_capacity(64);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").map_err(|_| LocalRouterTokenError::InvalidGeneration {
            value: "failed to render token".to_owned(),
        })?;
    }

    Ok(token)
}

#[cfg(test)]
#[path = "local_router_token_tests.rs"]
mod local_router_token_tests;
