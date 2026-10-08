use crate::SecretStore;
use crate::model::SecretStoreError;

use super::LoadedRouterAffinityHashSecret;
use super::RouterAffinityHashSecretOrigin;

/// Loads and validates the existing affinity secret without creating it.
pub fn load_existing_router_affinity_hash_secret(
    store: &impl SecretStore,
) -> Result<LoadedRouterAffinityHashSecret, SecretStoreError> {
    let key = super::router_affinity_hash_secret_key()?;
    let secret = store.read_secret(&key)?;
    super::parse_loaded_secret(secret, RouterAffinityHashSecretOrigin::LoadedExisting)
}
