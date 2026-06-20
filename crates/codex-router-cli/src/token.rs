//! Local router token commands.

use std::fmt::Write as _;
use std::io::Read;

use codex_router_core::ids::TokenGeneration;
use codex_router_core::local_auth::LocalRouterAuth;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::file_backend::SecretStore;
use codex_router_secret_store::model::SecretKey;
use codex_router_secret_store::model::SecretStoreError;
use thiserror::Error;

/// Shell dialect for token export.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shell {
    /// POSIX-compatible shell assignment.
    Posix,
}

/// Token command error.
#[derive(Debug, Error)]
pub enum TokenCommandError {
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
}

/// Service for local router token persistence.
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
    pub fn rotate(&self) -> Result<LocalRouterTokenRecord, TokenCommandError> {
        let token = generate_token()?;
        self.rotate_with_token(token)
    }

    /// Creates an initial local token if one does not already exist.
    pub fn initialize(&self) -> Result<LocalRouterTokenRecord, TokenCommandError> {
        if let Some(current) = self.load_current_optional()? {
            return Ok(current);
        }

        self.rotate()
    }

    /// Rotates the local token using a caller-supplied token value.
    pub fn rotate_with_token(
        &self,
        token: impl Into<String>,
    ) -> Result<LocalRouterTokenRecord, TokenCommandError> {
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
            .map_err(TokenCommandError::SecretStore)?;
        self.store
            .write_secret(
                &local_generation_key()?,
                &SecretString::new(generation.as_u64().to_string()),
            )
            .map_err(TokenCommandError::SecretStore)?;

        Ok(LocalRouterTokenRecord::new(token, generation))
    }

    /// Loads the current token record.
    pub fn load_current(&self) -> Result<LocalRouterTokenRecord, TokenCommandError> {
        self.load_current_optional()?
            .ok_or_else(|| TokenCommandError::InvalidGeneration {
                value: "<missing>".to_owned(),
            })
    }

    /// Loads the current and previous-token auth snapshot.
    pub fn load_auth(&self) -> Result<LocalRouterAuth, TokenCommandError> {
        let current = self.load_current()?;
        let previous = self.load_previous()?;

        Ok(LocalRouterAuth::new(current, previous))
    }

    fn load_current_optional(&self) -> Result<Option<LocalRouterTokenRecord>, TokenCommandError> {
        let token = match self.store.read_secret(&local_token_key()?) {
            Ok(token) => token,
            Err(SecretStoreError::Filesystem { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(TokenCommandError::SecretStore(error)),
        };
        let generation =
            self.read_generation()?
                .ok_or_else(|| TokenCommandError::InvalidGeneration {
                    value: "<missing>".to_owned(),
                })?;

        Ok(Some(LocalRouterTokenRecord::new(token, generation)))
    }

    fn read_generation(&self) -> Result<Option<TokenGeneration>, TokenCommandError> {
        match self.store.read_secret(&local_generation_key()?) {
            Ok(value) => parse_generation(value.expose_secret()).map(Some),
            Err(SecretStoreError::Filesystem { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            Err(error) => Err(TokenCommandError::SecretStore(error)),
        }
    }

    fn load_previous(&self) -> Result<Vec<LocalRouterTokenRecord>, TokenCommandError> {
        let token = match self.store.read_secret(&previous_local_token_key()?) {
            Ok(token) => token,
            Err(SecretStoreError::Filesystem { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(Vec::new());
            }
            Err(error) => return Err(TokenCommandError::SecretStore(error)),
        };
        let generation = match self.store.read_secret(&previous_local_generation_key()?) {
            Ok(generation) => parse_generation(generation.expose_secret())?,
            Err(error) => return Err(TokenCommandError::SecretStore(error)),
        };

        Ok(vec![LocalRouterTokenRecord::new(token, generation)])
    }

    fn write_previous(&self, previous: &LocalRouterTokenRecord) -> Result<(), TokenCommandError> {
        self.store
            .write_secret(&previous_local_token_key()?, previous.token())
            .map_err(TokenCommandError::SecretStore)?;
        self.store
            .write_secret(
                &previous_local_generation_key()?,
                &SecretString::new(previous.generation().as_u64().to_string()),
            )
            .map_err(TokenCommandError::SecretStore)
    }
}

/// Renders a shell assignment for the local router token.
#[must_use]
pub fn export_token_assignment(env_var: &str, token: &str, shell: Shell) -> String {
    match shell {
        Shell::Posix => format!("{env_var}={}\n", quote_posix(token)),
    }
}

fn quote_posix(value: &str) -> String {
    let mut quoted = String::from("'");
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

fn parse_generation(value: &str) -> Result<TokenGeneration, TokenCommandError> {
    let generation = value
        .parse::<u64>()
        .map_err(|_| TokenCommandError::InvalidGeneration {
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

fn generate_token() -> Result<String, TokenCommandError> {
    let mut file = std::fs::File::open("/dev/urandom").map_err(TokenCommandError::Random)?;
    let mut bytes = [0_u8; 32];
    file.read_exact(&mut bytes)
        .map_err(TokenCommandError::Random)?;

    let mut token = String::with_capacity(64);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").map_err(|_| TokenCommandError::InvalidGeneration {
            value: "failed to render token".to_owned(),
        })?;
    }

    Ok(token)
}
