//! Auth-owned login activation through the durable credential-generation claim.

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_credential_lock::AccountCredentialLock;
use codex_router_secret_store::account_tokens::first_unused_account_credential_generation;
use codex_router_secret_store::account_tokens::provider_credential_bundle_key;
use codex_router_secret_store::credential_bundle::CredentialBundle;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::credential_maintenance::ClaimPurpose;
use codex_router_state::credential_maintenance::LOGIN_CREDENTIAL_CLAIM_TIMEOUT_SECONDS;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use thiserror::Error;

/// Errors that prevent an owner login from activating its staged credential.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum CredentialActivationError {
    #[error("account provider does not match login credentials")]
    AccountProviderMismatch,
    #[error("credential provider does not match the requested account provider")]
    CredentialProviderMismatch,
    #[error("account credential lock unavailable")]
    CredentialLockUnavailable,
    #[error("credential generation claim unavailable")]
    GenerationClaimUnavailable,
    #[error("secret store unavailable")]
    CredentialStoreUnavailable,
    #[error("credential activation state unavailable")]
    StateUnavailable,
    #[error("system clock unavailable for credential activation")]
    ClockUnavailable,
}

/// Non-secret account identity and the in-memory bundle produced by an owner login.
pub struct CredentialActivationRequest {
    provider: Provider,
    account_id: AccountId,
    label: String,
    bundle: CredentialBundle,
}

impl CredentialActivationRequest {
    /// Creates a request for one provider-checked login activation.
    #[must_use]
    pub fn new(
        provider: Provider,
        account_id: AccountId,
        label: impl Into<String>,
        bundle: CredentialBundle,
    ) -> Self {
        Self {
            provider,
            account_id,
            label: label.into(),
            bundle,
        }
    }
}

/// Owns activation ordering for owner-provided login credentials.
pub struct CredentialActivation;

impl CredentialActivation {
    /// Claims a generation, writes its encrypted envelope, then activates it in SQLite.
    ///
    /// The account lock serializes login with refresh across processes. The existing
    /// credential-maintenance row is reused and removed by successful login activation.
    pub async fn activate_login<S>(
        state_store: &AsyncSqliteStateStore,
        secret_store: &S,
        request: CredentialActivationRequest,
    ) -> Result<u64, CredentialActivationError>
    where
        S: SecretStore + Clone + Send + Sync + 'static,
    {
        if request.bundle.provider() != request.provider {
            return Err(CredentialActivationError::CredentialProviderMismatch);
        }
        let database_path = state_store.database_path().to_path_buf();
        let lock_account_id = request.account_id.clone();
        let mut account_lock = tokio::task::spawn_blocking(move || {
            AccountCredentialLock::acquire(&database_path, &lock_account_id)
        })
        .await
        .map_err(|_| CredentialActivationError::CredentialLockUnavailable)?
        .map_err(|_| CredentialActivationError::CredentialLockUnavailable)?;

        let existing_account = state_store
            .load_account(&request.account_id)
            .await
            .map_err(map_state_error)?;
        if existing_account
            .as_ref()
            .is_some_and(|account| account.provider() != request.provider)
        {
            return Err(CredentialActivationError::AccountProviderMismatch);
        }

        if existing_account.is_none() {
            let new_account = AccountRecord::new(
                request.provider,
                request.account_id.clone(),
                request.label,
                AccountStatus::Disabled,
            );
            state_store
                .upsert_account(&new_account)
                .await
                .map_err(map_state_error)?;
        }

        let current_generation = existing_account
            .as_ref()
            .and_then(AccountRecord::active_credential_generation)
            .unwrap_or(0);
        let secret_store_for_generation = secret_store.clone();
        let account_for_generation = request.account_id.clone();
        let provider = request.provider;
        let (returned_lock, next_generation) = tokio::task::spawn_blocking(move || {
            let result = first_unused_account_credential_generation(
                &secret_store_for_generation,
                provider,
                &account_for_generation,
                current_generation,
            );
            (account_lock, result)
        })
        .await
        .map_err(|_| CredentialActivationError::CredentialStoreUnavailable)?;
        account_lock = returned_lock;
        let next_generation =
            next_generation.map_err(|_| CredentialActivationError::CredentialStoreUnavailable)?;
        let now_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| CredentialActivationError::ClockUnavailable)?
            .as_secs();
        state_store
            .release_stale_login_credential_claim(
                &request.account_id,
                request.provider,
                now_unix_seconds,
                LOGIN_CREDENTIAL_CLAIM_TIMEOUT_SECONDS,
            )
            .await
            .map_err(map_state_error)?;
        let previous_maintenance = state_store
            .load_credential_maintenance(&request.account_id)
            .await
            .map_err(map_state_error)?;

        let serialized_bundle = request
            .bundle
            .to_secret_string()
            .map_err(|_| CredentialActivationError::CredentialStoreUnavailable)?;

        let claimed = state_store
            .claim_credential_refresh(
                &request.account_id,
                request.provider,
                ClaimPurpose::Login,
                current_generation,
                next_generation,
                now_unix_seconds,
            )
            .await
            .map_err(map_state_error)?;
        if !claimed {
            return Err(CredentialActivationError::GenerationClaimUnavailable);
        }

        let secret_key =
            provider_credential_bundle_key(request.provider, &request.account_id, next_generation)
                .map_err(|_| CredentialActivationError::CredentialStoreUnavailable)?;
        let secret_store_for_write = secret_store.clone();
        let expected_bundle = request.bundle.clone();
        let write_key = secret_key.clone();
        let provider = request.provider;
        let write_outcome = tokio::task::spawn_blocking(move || {
            let result = secret_store_for_write
                .write_staged(&secret_key, &serialized_bundle)
                .and_then(|()| secret_store_for_write.read_secret(&write_key))
                .and_then(|stored_secret| {
                    CredentialBundle::from_secret_string(provider, stored_secret)
                })
                .and_then(|stored_bundle| {
                    if stored_bundle == expected_bundle {
                        Ok(())
                    } else {
                        Err(codex_router_secret_store::model::SecretStoreError::
                            InvalidSecretPayload {
                                message: "staged credential verification failed".to_owned(),
                            })
                    }
                });
            (account_lock, result)
        })
        .await;
        match write_outcome {
            Ok((returned_lock, Ok(()))) => {
                account_lock = returned_lock;
            }
            Ok((returned_lock, Err(_))) => {
                let release_result = release_failed_login_claim(
                    state_store,
                    &request.account_id,
                    request.provider,
                    current_generation,
                    next_generation,
                    previous_maintenance.as_ref(),
                )
                .await;
                drop(returned_lock);
                release_result?;
                return Err(CredentialActivationError::CredentialStoreUnavailable);
            }
            Err(_) => {
                let database_path = state_store.database_path().to_path_buf();
                let lock_account_id = request.account_id.clone();
                let returned_lock = tokio::task::spawn_blocking(move || {
                    AccountCredentialLock::acquire(&database_path, &lock_account_id)
                })
                .await
                .map_err(|_| CredentialActivationError::CredentialLockUnavailable)?
                .map_err(|_| CredentialActivationError::CredentialLockUnavailable)?;
                let release_result = release_failed_login_claim(
                    state_store,
                    &request.account_id,
                    request.provider,
                    current_generation,
                    next_generation,
                    previous_maintenance.as_ref(),
                )
                .await;
                drop(returned_lock);
                release_result?;
                return Err(CredentialActivationError::CredentialStoreUnavailable);
            }
        }

        let activated = state_store
            .activate_claimed_credential_generation(
                &request.account_id,
                request.provider,
                ClaimPurpose::Login,
                current_generation,
                next_generation,
                now_unix_seconds,
            )
            .await
            .map_err(map_state_error)?;
        if !activated {
            return Err(CredentialActivationError::GenerationClaimUnavailable);
        }

        let pruner_store = secret_store.clone();
        let account_for_pruning = request.account_id.clone();
        let pruned = tokio::task::spawn_blocking(move || {
            pruner_store.prune_obsolete_generations(
                request.provider,
                &account_for_pruning,
                next_generation,
            )
        })
        .await;
        if !matches!(pruned, Ok(Ok(_))) {
            tracing::warn!("obsolete credential generations could not be pruned after login");
        }

        drop(account_lock);
        Ok(next_generation)
    }
}

async fn release_failed_login_claim(
    state_store: &AsyncSqliteStateStore,
    account_id: &AccountId,
    provider: Provider,
    current_generation: u64,
    successor_generation: u64,
    previous_maintenance: Option<
        &codex_router_state::credential_maintenance::CredentialMaintenanceRecord,
    >,
) -> Result<(), CredentialActivationError> {
    let claim_released = state_store
        .restore_credential_maintenance_after_login_write_failure(
            account_id,
            provider,
            current_generation,
            successor_generation,
            previous_maintenance,
        )
        .await
        .map_err(map_state_error)?;
    if !claim_released {
        return Err(CredentialActivationError::GenerationClaimUnavailable);
    }
    Ok(())
}

fn map_state_error(error: StateStoreError) -> CredentialActivationError {
    match error {
        StateStoreError::AccountProviderImmutable => {
            CredentialActivationError::AccountProviderMismatch
        }
        _ => CredentialActivationError::StateUnavailable,
    }
}
