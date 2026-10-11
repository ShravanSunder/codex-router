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

    /// Writes a new credential generation without activating it.
    ///
    /// Pooled writes remain encrypted except in explicitly declared debug plaintext roots.
    /// Activation belongs to the auth coordinator after the staged value has
    /// been written successfully.
    fn write_staged(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        self.write_secret(key, secret)
    }

    /// Removes an inactive staged credential after its durable claim was restored.
    fn delete_staged(&self, key: &SecretKey) -> Result<(), SecretStoreError>;

    /// Keeps the previously active generation and deletes only generations below it.
    fn prune_obsolete_generations(
        &self,
        _provider: codex_router_core::provider::Provider,
        _account_id: &codex_router_core::ids::AccountId,
        _previously_active_generation: u64,
    ) -> Result<Vec<u64>, SecretStoreError> {
        Ok(Vec::new())
    }
}
