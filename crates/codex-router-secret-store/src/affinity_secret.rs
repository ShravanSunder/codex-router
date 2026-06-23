//! Router-owned previous-response affinity hash secret.

use std::fs::File;
use std::io::Read;

use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::redaction::SecretString;

use crate::backend::SecretStore;
use crate::model::SecretKey;
use crate::model::SecretStoreError;

/// Stable secret-store key for the router affinity HMAC secret.
pub const ROUTER_AFFINITY_HASH_SECRET_KEY: &str = "router_affinity_hash_secret.v1";

/// Origin of the loaded affinity secret.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouterAffinityHashSecretOrigin {
    /// Existing secret was loaded from the store.
    LoadedExisting,
    /// New secret was generated and written to the store.
    CreatedNew,
}

/// Loaded router affinity hash secret and lifecycle metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedRouterAffinityHashSecret {
    secret: RouterAffinityHashSecret,
    origin: RouterAffinityHashSecretOrigin,
}

impl LoadedRouterAffinityHashSecret {
    /// Creates a loaded secret result.
    #[must_use]
    pub const fn new(
        secret: RouterAffinityHashSecret,
        origin: RouterAffinityHashSecretOrigin,
    ) -> Self {
        Self { secret, origin }
    }

    /// Returns the secret.
    #[must_use]
    pub const fn secret(&self) -> &RouterAffinityHashSecret {
        &self.secret
    }

    /// Returns lifecycle origin.
    #[must_use]
    pub const fn origin(&self) -> RouterAffinityHashSecretOrigin {
        self.origin
    }
}

/// Loads the router affinity secret or creates it once.
pub fn load_or_create_router_affinity_hash_secret(
    store: &impl SecretStore,
) -> Result<LoadedRouterAffinityHashSecret, SecretStoreError> {
    let key = router_affinity_hash_secret_key()?;
    match store.read_secret(&key) {
        Ok(secret) => parse_loaded_secret(secret, RouterAffinityHashSecretOrigin::LoadedExisting),
        Err(error) if secret_is_missing(&error) => {
            let secret = generate_router_affinity_hash_secret()?;
            store.write_secret(&key, &SecretString::new(secret.expose_secret()))?;
            Ok(LoadedRouterAffinityHashSecret::new(
                secret,
                RouterAffinityHashSecretOrigin::CreatedNew,
            ))
        }
        Err(error) => Err(error),
    }
}

/// Returns the stable affinity secret key.
pub fn router_affinity_hash_secret_key() -> Result<SecretKey, SecretStoreError> {
    SecretKey::new(ROUTER_AFFINITY_HASH_SECRET_KEY)
}

fn parse_loaded_secret(
    secret: SecretString,
    origin: RouterAffinityHashSecretOrigin,
) -> Result<LoadedRouterAffinityHashSecret, SecretStoreError> {
    let secret = RouterAffinityHashSecret::new(secret.expose_secret()).map_err(|error| {
        SecretStoreError::InvalidSecretPayload {
            message: error.to_string(),
        }
    })?;

    Ok(LoadedRouterAffinityHashSecret::new(secret, origin))
}

fn generate_router_affinity_hash_secret() -> Result<RouterAffinityHashSecret, SecretStoreError> {
    let mut bytes = [0_u8; 32];
    read_os_random(&mut bytes)?;
    RouterAffinityHashSecret::new(lowercase_hex(&bytes)).map_err(|error| {
        SecretStoreError::InvalidSecretPayload {
            message: error.to_string(),
        }
    })
}

fn read_os_random(buffer: &mut [u8]) -> Result<(), SecretStoreError> {
    let path = std::path::PathBuf::from("/dev/urandom");
    let mut file = File::open(&path).map_err(|source| SecretStoreError::Filesystem {
        path: path.clone(),
        source,
    })?;
    file.read_exact(buffer)
        .map_err(|source| SecretStoreError::Filesystem { path, source })
}

fn secret_is_missing(error: &SecretStoreError) -> bool {
    matches!(
        error,
        SecretStoreError::Filesystem { source, .. }
            if source.kind() == std::io::ErrorKind::NotFound
    )
}

fn lowercase_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    output
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'a' + (nibble - 10)),
        _ => unreachable!("nibble is masked to four bits"),
    }
}
