//! Secret store backend contract.

use codex_router_core::redaction::SecretString;

use crate::model::SecretKey;
use crate::model::SecretStoreError;

/// Secret storage behavior.
pub trait SecretStore {
    /// Writes a secret value.
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError>;

    /// Reads a secret value.
    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError>;
}
