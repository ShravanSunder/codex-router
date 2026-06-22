//! Router-owned provider credential resolution.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::file_backend::SecretStore;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::account::AccountStatus;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use thiserror::Error;

/// Credential resolver failure.
#[derive(Debug, Error)]
pub enum CredentialResolverError {
    /// Account metadata was unavailable or not usable.
    #[error("provider credential account is unavailable")]
    AccountUnavailable,
    /// Account is disabled or has no active credential generation.
    #[error("provider credential account is ineligible")]
    AccountIneligible,
    /// Secret material was unavailable or malformed.
    #[error("provider credential secret is unavailable")]
    SecretUnavailable,
    /// Refresh is required but cannot be performed.
    #[error("provider credential refresh is unavailable")]
    RefreshUnavailable,
}

impl Clone for CredentialResolverError {
    fn clone(&self) -> Self {
        match self {
            Self::AccountUnavailable => Self::AccountUnavailable,
            Self::AccountIneligible => Self::AccountIneligible,
            Self::SecretUnavailable => Self::SecretUnavailable,
            Self::RefreshUnavailable => Self::RefreshUnavailable,
        }
    }
}

impl PartialEq for CredentialResolverError {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::AccountUnavailable, Self::AccountUnavailable)
                | (Self::AccountIneligible, Self::AccountIneligible)
                | (Self::SecretUnavailable, Self::SecretUnavailable)
                | (Self::RefreshUnavailable, Self::RefreshUnavailable)
        )
    }
}

impl Eq for CredentialResolverError {}

/// Resolved provider credential emitted immediately before upstream egress.
#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedProviderCredential {
    account_id: AccountId,
    access_token: SecretString,
}

impl ResolvedProviderCredential {
    /// Creates a resolved provider credential.
    #[must_use]
    pub const fn new(account_id: AccountId, access_token: SecretString) -> Self {
        Self {
            account_id,
            access_token,
        }
    }

    /// Returns the selected account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the bearer access token.
    #[must_use]
    pub const fn access_token(&self) -> &SecretString {
        &self.access_token
    }
}

impl fmt::Debug for ResolvedProviderCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedProviderCredential")
            .field("account_id", &self.account_id)
            .field("access_token", &"<redacted>")
            .finish()
    }
}

/// Resolves provider credentials for selected accounts.
pub trait ProviderCredentialResolver {
    /// Resolves credentials immediately before provider egress.
    fn resolve_provider_credentials(
        &self,
        account_id: &AccountId,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError>;
}

/// Refresh client used when an active access token is expired.
pub trait CredentialRefreshClient {
    /// Refreshes one account credential bundle.
    fn refresh_credentials(
        &self,
        account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialResolverError>;
}

/// Refresh client used when runtime refresh is intentionally unavailable.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoopCredentialRefreshClient;

impl CredentialRefreshClient for NoopCredentialRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialResolverError> {
        Err(CredentialResolverError::RefreshUnavailable)
    }
}

/// Shared per-account refresh leases for resolver single-flight behavior.
#[derive(Clone, Debug, Default)]
pub struct RefreshLeaseRegistry {
    leases: Arc<Mutex<HashMap<AccountId, Arc<Mutex<()>>>>>,
}

impl RefreshLeaseRegistry {
    /// Creates an empty refresh lease registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lease_for(&self, account_id: &AccountId) -> Arc<Mutex<()>> {
        let mut leases = self
            .leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            leases
                .entry(account_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }
}

/// Resolves provider credentials through router-owned state and secret stores.
#[derive(Debug)]
pub struct RouterCredentialResolver<'a, S, C>
where
    S: SecretStore,
    C: CredentialRefreshClient,
{
    state_repository: &'a SqliteStateStore,
    secret_store: &'a S,
    refresh_client: C,
    now_unix_seconds: u64,
    refresh_leases: RefreshLeaseRegistry,
}

impl<'a, S, C> RouterCredentialResolver<'a, S, C>
where
    S: SecretStore,
    C: CredentialRefreshClient,
{
    /// Creates a credential resolver.
    #[must_use]
    pub fn new(
        state_repository: &'a SqliteStateStore,
        secret_store: &'a S,
        refresh_client: C,
        now_unix_seconds: u64,
    ) -> Self {
        Self {
            state_repository,
            secret_store,
            refresh_client,
            now_unix_seconds,
            refresh_leases: RefreshLeaseRegistry::new(),
        }
    }

    /// Creates a credential resolver with shared refresh leases.
    #[must_use]
    pub fn new_with_refresh_leases(
        state_repository: &'a SqliteStateStore,
        secret_store: &'a S,
        refresh_client: C,
        now_unix_seconds: u64,
        refresh_leases: RefreshLeaseRegistry,
    ) -> Self {
        Self {
            state_repository,
            secret_store,
            refresh_client,
            now_unix_seconds,
            refresh_leases,
        }
    }

    fn read_active_bundle(
        &self,
        account_id: &AccountId,
    ) -> Result<AccountCredentialBundle, CredentialResolverError> {
        let account = self
            .state_repository
            .load_account(account_id)
            .map_err(map_state_error)?
            .ok_or(CredentialResolverError::AccountUnavailable)?;
        if account.status() != AccountStatus::Enabled {
            return Err(CredentialResolverError::AccountIneligible);
        }
        let active_generation = account
            .active_credential_generation()
            .ok_or(CredentialResolverError::AccountIneligible)?;
        let bundle_key = account_credential_bundle_key(account_id, active_generation)
            .map_err(map_secret_error)?;

        AccountCredentialBundle::from_secret_string(
            self.secret_store
                .read_secret(&bundle_key)
                .map_err(map_secret_error)?,
        )
        .map_err(map_secret_error)
    }

    fn bundle_is_expired(&self, bundle: &AccountCredentialBundle) -> bool {
        bundle
            .expires_unix_seconds()
            .is_some_and(|expires| expires <= self.now_unix_seconds)
    }

    fn refresh_expired_bundle(
        &self,
        account_id: &AccountId,
        bundle: &AccountCredentialBundle,
    ) -> Result<AccountCredentialBundle, CredentialResolverError> {
        let refresh_token = bundle
            .refresh_token()
            .ok_or(CredentialResolverError::RefreshUnavailable)?;
        let refreshed = self
            .refresh_client
            .refresh_credentials(account_id, refresh_token)?;
        let refreshed_generation = self
            .state_repository
            .next_credential_generation(account_id)
            .map_err(map_state_error)?;
        let refreshed_key = account_credential_bundle_key(account_id, refreshed_generation)
            .map_err(map_secret_error)?;
        self.secret_store
            .write_secret(
                &refreshed_key,
                &refreshed.to_secret_string().map_err(map_secret_error)?,
            )
            .map_err(map_secret_error)?;
        self.state_repository
            .activate_account_credential_generation_and_invalidate_quota(
                account_id,
                refreshed_generation,
                AccountStatus::Enabled,
            )
            .map_err(map_state_error)?;

        Ok(refreshed)
    }
}

impl<S, C> ProviderCredentialResolver for RouterCredentialResolver<'_, S, C>
where
    S: SecretStore,
    C: CredentialRefreshClient,
{
    fn resolve_provider_credentials(
        &self,
        account_id: &AccountId,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        let bundle = self.read_active_bundle(account_id)?;
        if self.bundle_is_expired(&bundle) {
            let lease = self.refresh_leases.lease_for(account_id);
            let _guard = lease
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let current_bundle = self.read_active_bundle(account_id)?;
            let refreshed = if self.bundle_is_expired(&current_bundle) {
                self.refresh_expired_bundle(account_id, &current_bundle)?
            } else {
                current_bundle
            };
            return Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                refreshed.access_token().clone(),
            ));
        }

        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            bundle.access_token().clone(),
        ))
    }
}

fn map_state_error(_error: StateStoreError) -> CredentialResolverError {
    CredentialResolverError::AccountUnavailable
}

fn map_secret_error(_error: SecretStoreError) -> CredentialResolverError {
    CredentialResolverError::SecretUnavailable
}
