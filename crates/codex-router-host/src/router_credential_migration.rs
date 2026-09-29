//! Host-owned pooled credential migration before an owned Router process starts.

use std::path::Path;

use codex_router_secret_store::credential_migration::CredentialMigrationOutcome;
#[cfg(not(any(test, feature = "keychain-test-support")))]
use codex_router_secret_store::credential_migration::migrate_pooled_credentials_at_production_startup;
#[cfg(any(test, feature = "keychain-test-support"))]
use codex_router_secret_store::credential_migration::migrate_pooled_credentials_at_startup;
#[cfg(any(test, feature = "keychain-test-support"))]
use codex_router_secret_store::model::SecretStoreError;
#[cfg(any(test, feature = "keychain-test-support"))]
use codex_router_secret_store::test_support::DeterministicTestKeychainAccess;

/// Runs the startup migration off the executor and reports any fail-closed outcome.
pub(crate) async fn migrate_before_router_spawn(secret_root: &Path) {
    let secret_root = secret_root.to_path_buf();
    #[cfg(any(test, feature = "keychain-test-support"))]
    let migration = tokio::task::spawn_blocking(move || {
        let home_root = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .ok_or(SecretStoreError::TestCredentialKeyRootUnverifiable)?;
        migrate_with_test_keychain_for_home(&secret_root, &home_root)
    })
    .await;
    #[cfg(not(any(test, feature = "keychain-test-support")))]
    let migration = tokio::task::spawn_blocking(move || {
        migrate_pooled_credentials_at_production_startup(secret_root)
    })
    .await;

    match migration {
        Ok(Ok(CredentialMigrationOutcome::Complete)) => {}
        Ok(Ok(CredentialMigrationOutcome::Incomplete { accounts, failure })) => {
            tracing::error!(
                accounts = ?accounts,
                reason = %failure,
                "pooled credential migration is incomplete; starting Router with credentials unavailable"
            );
        }
        Ok(Ok(CredentialMigrationOutcome::KeyUnavailable)) => {
            tracing::error!(
                "pooled credential key is unavailable; starting Router without pooled credential access"
            );
        }
        Ok(Err(error)) => {
            tracing::error!(
                error = %error,
                "pooled credential migration could not run; starting Router with credentials unavailable"
            );
        }
        Err(_join_error) => {
            tracing::error!(
                "pooled credential migration task failed; starting Router with credentials unavailable"
            );
        }
    }
}

#[cfg(any(test, feature = "keychain-test-support"))]
fn migrate_with_test_keychain_for_home(
    secret_root: &Path,
    home_root: &Path,
) -> Result<CredentialMigrationOutcome, SecretStoreError> {
    codex_router_secret_store::test_support::ensure_test_credential_key_root_safe_for_home(
        secret_root,
        home_root,
    )?;
    migrate_pooled_credentials_at_startup(secret_root, &DeterministicTestKeychainAccess)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_migration_refuses_the_production_default_root() {
        let home = tempfile::TempDir::new().expect("temporary home");
        let secret_root = home.path().join(".codex-router").join("secrets");

        let result = migrate_with_test_keychain_for_home(&secret_root, home.path());

        assert!(matches!(
            result,
            Err(SecretStoreError::TestCredentialKeyOnProductionRoot)
        ));
        assert!(!secret_root.exists());
    }
}
