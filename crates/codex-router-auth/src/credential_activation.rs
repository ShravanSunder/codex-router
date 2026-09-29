//! Auth-owned login activation through the durable credential-generation claim.

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_credential_lock::AccountCredentialLock;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::first_unused_account_credential_generation;
use codex_router_secret_store::account_tokens::provider_credential_bundle_key;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::credential_maintenance::ClaimPurpose;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use thiserror::Error;

/// Errors that prevent an owner login from activating its staged credential.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum CredentialActivationError {
    #[error("account provider does not match login credentials")]
    AccountProviderMismatch,
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
    bundle: AccountCredentialBundle,
}

impl CredentialActivationRequest {
    /// Creates a request for one provider-checked login activation.
    #[must_use]
    pub fn new(
        provider: Provider,
        account_id: AccountId,
        label: impl Into<String>,
        bundle: AccountCredentialBundle,
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
    /// credential-maintenance claim row is reused and removed by successful login
    /// activation; no login-only durable state is introduced.
    pub async fn activate_login<S>(
        state_store: &AsyncSqliteStateStore,
        secret_store: &S,
        request: CredentialActivationRequest,
    ) -> Result<u64, CredentialActivationError>
    where
        S: SecretStore + Clone + Send + Sync + 'static,
    {
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

        let claimed = state_store
            .claim_credential_refresh(
                &request.account_id,
                request.provider,
                ClaimPurpose::Login,
                current_generation,
                next_generation,
            )
            .await
            .map_err(map_state_error)?;
        if !claimed {
            return Err(CredentialActivationError::GenerationClaimUnavailable);
        }

        let now_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| CredentialActivationError::ClockUnavailable)?
            .as_secs();
        let secret_key =
            provider_credential_bundle_key(request.provider, &request.account_id, next_generation)
                .map_err(|_| CredentialActivationError::CredentialStoreUnavailable)?;
        let serialized_bundle = request
            .bundle
            .to_secret_string()
            .map_err(|_| CredentialActivationError::CredentialStoreUnavailable)?;
        let secret_store_for_write = secret_store.clone();
        let (returned_lock, write_result) = tokio::task::spawn_blocking(move || {
            let result = secret_store_for_write.write_staged(&secret_key, &serialized_bundle);
            (account_lock, result)
        })
        .await
        .map_err(|_| CredentialActivationError::CredentialStoreUnavailable)?;
        account_lock = returned_lock;
        write_result.map_err(|_| CredentialActivationError::CredentialStoreUnavailable)?;

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

        drop(account_lock);
        Ok(next_generation)
    }
}

fn map_state_error(error: StateStoreError) -> CredentialActivationError {
    match error {
        StateStoreError::AccountProviderImmutable => {
            CredentialActivationError::AccountProviderMismatch
        }
        _ => CredentialActivationError::StateUnavailable,
    }
}
