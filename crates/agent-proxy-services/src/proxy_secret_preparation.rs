use crate::proxy_preparation_error::ProxyPreparationError;
use codex_router_core::{affinity::RouterAffinityHashSecret, local_auth::LocalRouterTokenRecord};
use codex_router_keeper_protocol::{ChildComponent, ChildDegradation, PrepareMode};
use codex_router_secret_store::{
    affinity_secret::{
        load_existing_router_affinity_hash_secret, load_or_create_router_affinity_hash_secret,
    },
    encrypted_credential_store::{EncryptedCredentialStore, EncryptedCredentialStoreStatus},
    file_backend::FileSecretStore,
    local_router_token::LocalRouterTokenService,
};
use std::path::Path;
pub(crate) struct ProxyPreparedSecrets {
    pub(crate) credentials: EncryptedCredentialStore,
    pub(crate) local_token_store: FileSecretStore,
    pub(crate) local_token: LocalRouterTokenRecord,
    pub(crate) affinity: RouterAffinityHashSecret,
    pub(crate) degraded: Vec<(ChildComponent, ChildDegradation)>,
}
pub(crate) fn prepare_proxy_secrets(
    root: &Path,
    mode: &PrepareMode,
) -> Result<ProxyPreparedSecrets, ProxyPreparationError> {
    #[cfg(any(test, feature = "test-support", feature = "keychain-test-support"))]
    {
        codex_router_secret_store::test_support::ensure_test_credential_key_root_safe(root)?;
        prepare_with_keychain(
            root,
            mode,
            &crate::proxy_keychain_fixture::ProxyFixtureKeychain,
        )
    }
    #[cfg(not(any(test, feature = "test-support", feature = "keychain-test-support")))]
    {
        let credentials = match mode {
            PrepareMode::Fresh => {
                codex_router_secret_store::credential_migration::migrate_pooled_credentials_at_production_startup(root)?;
                EncryptedCredentialStore::open_production_for_process(root)?
            }
            PrepareMode::Replacement { .. } => {
                EncryptedCredentialStore::open_existing_production_for_process(root)?
            }
        };
        assemble_secrets(root, mode, credentials)
    }
}
#[cfg(any(test, feature = "test-support", feature = "keychain-test-support"))]
pub(crate) fn prepare_with_keychain(
    root: &Path,
    mode: &PrepareMode,
    keychain: &dyn codex_router_secret_store::keychain_data_key::KeychainAccess,
) -> Result<ProxyPreparedSecrets, ProxyPreparationError> {
    let credentials = match mode {
        PrepareMode::Fresh => {
            codex_router_secret_store::credential_migration::migrate_pooled_credentials_at_startup(
                root, keychain,
            )?;
            EncryptedCredentialStore::open_for_process_with_keychain(root, keychain)?
        }
        PrepareMode::Replacement { .. } => {
            EncryptedCredentialStore::open_existing_for_process_with_keychain(root, keychain)?
        }
    };
    assemble_secrets(root, mode, credentials)
}
pub(crate) fn assemble_secrets(
    root: &Path,
    mode: &PrepareMode,
    credentials: EncryptedCredentialStore,
) -> Result<ProxyPreparedSecrets, ProxyPreparationError> {
    let allowed = matches!(mode,PrepareMode::Replacement { active_degraded } if active_degraded.iter().any(|pair| matches!(pair,(ChildComponent::PooledCredentials,ChildDegradation::CredentialStoreUnavailable))));
    if matches!(mode, PrepareMode::Replacement { .. })
        && !matches!(credentials.status(), EncryptedCredentialStoreStatus::Ready)
        && !(allowed
            && matches!(
                credentials.status(),
                EncryptedCredentialStoreStatus::KeyUnavailable
            ))
    {
        return Err(ProxyPreparationError::CredentialsUnavailable);
    }
    let (local_token_store, local_token, affinity) = match mode {
        PrepareMode::Fresh => {
            let store = FileSecretStore::open(root)?;
            let token = LocalRouterTokenService::new(store.clone()).ensure_local_token(root)?;
            let affinity = load_or_create_router_affinity_hash_secret(&credentials)?
                .secret()
                .clone();
            (store, token, affinity)
        }
        PrepareMode::Replacement { .. } => {
            let store = FileSecretStore::open_read_only(root)?;
            let token = LocalRouterTokenService::new(store.clone()).load_current()?;
            let affinity = load_existing_router_affinity_hash_secret(&credentials)?
                .secret()
                .clone();
            (store, token, affinity)
        }
    };
    let degraded = if matches!(credentials.status(), EncryptedCredentialStoreStatus::Ready) {
        Vec::new()
    } else {
        vec![(
            ChildComponent::PooledCredentials,
            ChildDegradation::CredentialStoreUnavailable,
        )]
    };
    Ok(ProxyPreparedSecrets {
        credentials,
        local_token_store,
        local_token,
        affinity,
        degraded,
    })
}
