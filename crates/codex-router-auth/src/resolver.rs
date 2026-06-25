//! Router-owned provider credential resolution.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::account::AccountStatus;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use crate::oauth::OAuthRefreshClassification;
use crate::oauth::classify_refresh_response;

const DEFAULT_OPENAI_OAUTH_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const DEFAULT_OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

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
    chatgpt_account_id: Option<String>,
    credential_generation: u64,
}

impl ResolvedProviderCredential {
    /// Creates a resolved provider credential.
    #[must_use]
    pub const fn new(
        account_id: AccountId,
        access_token: SecretString,
        credential_generation: u64,
    ) -> Self {
        Self {
            account_id,
            access_token,
            chatgpt_account_id: None,
            credential_generation,
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

    /// Returns the ChatGPT account id for ChatGPT backend requests.
    #[must_use]
    pub fn chatgpt_account_id(&self) -> Option<&str> {
        self.chatgpt_account_id.as_deref()
    }

    /// Returns the credential generation that produced this access token.
    #[must_use]
    pub const fn credential_generation(&self) -> u64 {
        self.credential_generation
    }

    /// Sets the ChatGPT account id for ChatGPT backend requests.
    #[must_use]
    pub fn with_chatgpt_account_id(mut self, chatgpt_account_id: Option<&str>) -> Self {
        self.chatgpt_account_id = chatgpt_account_id.map(str::to_owned);
        self
    }
}

impl fmt::Debug for ResolvedProviderCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedProviderCredential")
            .field("account_id", &self.account_id)
            .field("access_token", &"<redacted>")
            .field("chatgpt_account_id", &self.chatgpt_account_id)
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

/// Test-only refresh commit failpoints for cancellation-safety coverage.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshCommitFailpoint {
    /// Fail after writing the next-generation secret and before state commit.
    AfterSecretWrite,
    /// Fail after state commit and before returning to the caller.
    AfterStateCommit,
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

/// OpenAI OAuth refresh client compatible with Codex auth.json credentials.
#[derive(Clone, Debug)]
pub struct OpenAiOAuthRefreshClient {
    token_endpoint: String,
    client_id: String,
}

impl OpenAiOAuthRefreshClient {
    /// Creates a refresh client using Codex-compatible production defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token_endpoint: DEFAULT_OPENAI_OAUTH_TOKEN_ENDPOINT.to_owned(),
            client_id: DEFAULT_OPENAI_OAUTH_CLIENT_ID.to_owned(),
        }
    }

    /// Creates a refresh client with explicit endpoint/client id for tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn new_with_endpoint(
        token_endpoint: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            token_endpoint: token_endpoint.into(),
            client_id: client_id.into(),
        }
    }
}

impl Default for OpenAiOAuthRefreshClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialRefreshClient for OpenAiOAuthRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialResolverError> {
        let request = RefreshTokenRequest {
            client_id: &self.client_id,
            grant_type: "refresh_token",
            refresh_token: refresh_token.expose_secret(),
        };
        let body = serde_json::to_string(&request)
            .map_err(|_error| CredentialResolverError::RefreshUnavailable)?;
        let client = reqwest::blocking::Client::builder()
            .build()
            .map_err(|_error| CredentialResolverError::RefreshUnavailable)?;
        let response = client
            .post(&self.token_endpoint)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .map_err(|_error| CredentialResolverError::RefreshUnavailable)?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|_error| CredentialResolverError::RefreshUnavailable)?;

        if !status.is_success() {
            let oauth_error = serde_json::from_str::<RefreshTokenErrorResponse>(&body)
                .ok()
                .and_then(|error| error.error);
            return match classify_refresh_response(status.as_u16(), oauth_error.as_deref()) {
                OAuthRefreshClassification::Succeeded
                | OAuthRefreshClassification::RefreshTokenRejected
                | OAuthRefreshClassification::RateLimited
                | OAuthRefreshClassification::TransientProviderFailure
                | OAuthRefreshClassification::UnexpectedProviderResponse { .. } => {
                    Err(CredentialResolverError::RefreshUnavailable)
                }
            };
        }

        let refresh_response = serde_json::from_str::<RefreshTokenResponse>(&body)
            .map_err(|_error| CredentialResolverError::RefreshUnavailable)?;
        let mut refreshed = AccountCredentialBundle::imported_codex_auth(
            refresh_response.access_token,
            Some(
                refresh_response
                    .refresh_token
                    .unwrap_or_else(|| refresh_token.expose_secret().to_owned()),
            ),
        );
        if let Some(expires_in_seconds) = refresh_response.expires_in {
            let expires_unix_seconds = current_unix_seconds()
                .unwrap_or(0)
                .saturating_add(expires_in_seconds);
            refreshed = refreshed.with_expires_unix_seconds(expires_unix_seconds);
        }

        Ok(refreshed)
    }
}

#[derive(Serialize)]
struct RefreshTokenRequest<'a> {
    client_id: &'a str,
    grant_type: &'a str,
    refresh_token: &'a str,
}

#[derive(Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct RefreshTokenErrorResponse {
    error: Option<String>,
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

/// Shared per-account async refresh leases for resolver single-flight behavior.
#[derive(Clone, Debug, Default)]
pub struct AsyncRefreshLeaseRegistry {
    leases: Arc<Mutex<HashMap<AccountId, Arc<tokio::sync::Mutex<()>>>>>,
}

impl AsyncRefreshLeaseRegistry {
    /// Creates an empty async refresh lease registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lease_for(&self, account_id: &AccountId) -> Arc<tokio::sync::Mutex<()>> {
        let mut leases = self
            .leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            leases
                .entry(account_id.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
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
    #[cfg(test)]
    refresh_commit_failpoint: Option<RefreshCommitFailpoint>,
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
            #[cfg(test)]
            refresh_commit_failpoint: None,
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
            #[cfg(test)]
            refresh_commit_failpoint: None,
        }
    }

    /// Adds a test-only failpoint for credential refresh commit windows.
    #[cfg(test)]
    #[must_use]
    pub fn with_refresh_commit_failpoint(mut self, failpoint: RefreshCommitFailpoint) -> Self {
        self.refresh_commit_failpoint = Some(failpoint);
        self
    }

    fn read_active_bundle(
        &self,
        account_id: &AccountId,
    ) -> Result<(u64, AccountCredentialBundle), CredentialResolverError> {
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

        let bundle = AccountCredentialBundle::from_secret_string(
            self.secret_store
                .read_secret(&bundle_key)
                .map_err(map_secret_error)?,
        )
        .map_err(map_secret_error)?;

        Ok((active_generation, bundle))
    }

    fn bundle_is_expired(&self, bundle: &AccountCredentialBundle) -> bool {
        bundle
            .expires_unix_seconds()
            .is_some_and(|expires| expires <= self.now_unix_seconds)
    }

    fn refresh_expired_bundle(
        &self,
        account_id: &AccountId,
        current_generation: u64,
        bundle: &AccountCredentialBundle,
    ) -> Result<(u64, AccountCredentialBundle), CredentialResolverError> {
        let refresh_token = bundle
            .refresh_token()
            .ok_or(CredentialResolverError::RefreshUnavailable)?;
        let mut refreshed = self
            .refresh_client
            .refresh_credentials(account_id, refresh_token)?;
        if refreshed.chatgpt_account_id().is_none()
            && let Some(chatgpt_account_id) = bundle.chatgpt_account_id()
        {
            refreshed = refreshed.with_chatgpt_account_id(chatgpt_account_id);
        }
        let refreshed_generation = current_generation
            .checked_add(1)
            .ok_or(CredentialResolverError::RefreshUnavailable)?;
        let refreshed_key = account_credential_bundle_key(account_id, refreshed_generation)
            .map_err(map_secret_error)?;
        self.secret_store
            .write_secret(
                &refreshed_key,
                &refreshed.to_secret_string().map_err(map_secret_error)?,
            )
            .map_err(map_secret_error)?;
        #[cfg(test)]
        if self.refresh_commit_failpoint == Some(RefreshCommitFailpoint::AfterSecretWrite) {
            return Err(CredentialResolverError::RefreshUnavailable);
        }
        self.state_repository
            .activate_account_credential_generation_if_current_and_invalidate_quota(
                account_id,
                current_generation,
                refreshed_generation,
                AccountStatus::Enabled,
            )
            .map_err(map_state_error)?;
        #[cfg(test)]
        if self.refresh_commit_failpoint == Some(RefreshCommitFailpoint::AfterStateCommit) {
            return Err(CredentialResolverError::RefreshUnavailable);
        }

        Ok((refreshed_generation, refreshed))
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
        let (active_generation, bundle) = self.read_active_bundle(account_id)?;
        if self.bundle_is_expired(&bundle) {
            let lease = self.refresh_leases.lease_for(account_id);
            let _guard = lease
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (current_generation, current_bundle) = self.read_active_bundle(account_id)?;
            let (resolved_generation, refreshed) = if self.bundle_is_expired(&current_bundle) {
                self.refresh_expired_bundle(account_id, current_generation, &current_bundle)?
            } else {
                (current_generation, current_bundle)
            };
            return Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                refreshed.access_token().clone(),
                resolved_generation,
            )
            .with_chatgpt_account_id(refreshed.chatgpt_account_id()));
        }

        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            bundle.access_token().clone(),
            active_generation,
        )
        .with_chatgpt_account_id(bundle.chatgpt_account_id()))
    }
}

/// Async resolver for provider credentials through router-owned state and secret stores.
#[derive(Clone, Debug)]
pub struct AsyncRouterCredentialResolver<S, C>
where
    S: SecretStore + Clone,
    C: CredentialRefreshClient + Clone,
{
    state_store: AsyncSqliteStateStore,
    secret_store: S,
    refresh_client: C,
    fixed_now_unix_seconds: Option<u64>,
    refresh_leases: AsyncRefreshLeaseRegistry,
}

/// Default async router credential resolver for OpenAI OAuth account tokens.
pub type DefaultAsyncRouterCredentialResolver<S> =
    AsyncRouterCredentialResolver<S, OpenAiOAuthRefreshClient>;

impl<S> AsyncRouterCredentialResolver<S, OpenAiOAuthRefreshClient>
where
    S: SecretStore + Clone + Send + 'static,
{
    /// Creates a default OpenAI OAuth async resolver with shared refresh leases.
    #[must_use]
    pub fn new_default_oauth_with_refresh_leases(
        state_store: AsyncSqliteStateStore,
        secret_store: S,
        fixed_now_unix_seconds: Option<u64>,
        refresh_leases: AsyncRefreshLeaseRegistry,
    ) -> Self {
        Self::new_with_refresh_leases(
            state_store,
            secret_store,
            OpenAiOAuthRefreshClient::new(),
            fixed_now_unix_seconds,
            refresh_leases,
        )
    }
}

impl<S, C> AsyncRouterCredentialResolver<S, C>
where
    S: SecretStore + Clone + Send + 'static,
    C: CredentialRefreshClient + Clone + Send + 'static,
{
    /// Creates an async credential resolver.
    #[must_use]
    pub fn new(
        state_store: AsyncSqliteStateStore,
        secret_store: S,
        refresh_client: C,
        fixed_now_unix_seconds: Option<u64>,
    ) -> Self {
        Self {
            state_store,
            secret_store,
            refresh_client,
            fixed_now_unix_seconds,
            refresh_leases: AsyncRefreshLeaseRegistry::new(),
        }
    }

    /// Creates an async credential resolver with shared refresh leases.
    #[must_use]
    pub fn new_with_refresh_leases(
        state_store: AsyncSqliteStateStore,
        secret_store: S,
        refresh_client: C,
        fixed_now_unix_seconds: Option<u64>,
        refresh_leases: AsyncRefreshLeaseRegistry,
    ) -> Self {
        Self {
            state_store,
            secret_store,
            refresh_client,
            fixed_now_unix_seconds,
            refresh_leases,
        }
    }

    /// Resolves credentials immediately before provider egress.
    pub async fn resolve_provider_credentials(
        &self,
        account_id: &AccountId,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        let now_unix_seconds = match self.fixed_now_unix_seconds {
            Some(now_unix_seconds) => now_unix_seconds,
            None => current_unix_seconds()
                .map_err(|_error| CredentialResolverError::RefreshUnavailable)?,
        };
        let (active_generation, bundle) = self.read_active_bundle(account_id).await?;
        if self.bundle_is_expired(&bundle, now_unix_seconds) {
            let lease = self.refresh_leases.lease_for(account_id);
            let _guard = lease.lock().await;
            let (current_generation, current_bundle) = self.read_active_bundle(account_id).await?;
            let (resolved_generation, refreshed) =
                if self.bundle_is_expired(&current_bundle, now_unix_seconds) {
                    self.refresh_expired_bundle(account_id, current_generation, &current_bundle)
                        .await?
                } else {
                    (current_generation, current_bundle)
                };
            return Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                refreshed.access_token().clone(),
                resolved_generation,
            )
            .with_chatgpt_account_id(refreshed.chatgpt_account_id()));
        }

        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            bundle.access_token().clone(),
            active_generation,
        )
        .with_chatgpt_account_id(bundle.chatgpt_account_id()))
    }

    async fn read_active_bundle(
        &self,
        account_id: &AccountId,
    ) -> Result<(u64, AccountCredentialBundle), CredentialResolverError> {
        let account = self
            .state_store
            .load_account(account_id)
            .await
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
        let secret_store = self.secret_store.clone();
        let bundle = tokio::task::spawn_blocking(move || {
            let secret = secret_store
                .read_secret(&bundle_key)
                .map_err(map_secret_error)?;
            AccountCredentialBundle::from_secret_string(secret).map_err(map_secret_error)
        })
        .await
        .map_err(|_error| CredentialResolverError::SecretUnavailable)??;

        Ok((active_generation, bundle))
    }

    fn bundle_is_expired(&self, bundle: &AccountCredentialBundle, now_unix_seconds: u64) -> bool {
        bundle
            .expires_unix_seconds()
            .is_some_and(|expires| expires <= now_unix_seconds)
    }

    async fn refresh_expired_bundle(
        &self,
        account_id: &AccountId,
        current_generation: u64,
        bundle: &AccountCredentialBundle,
    ) -> Result<(u64, AccountCredentialBundle), CredentialResolverError> {
        let refresh_token = bundle
            .refresh_token()
            .ok_or(CredentialResolverError::RefreshUnavailable)?
            .clone();
        let refresh_client = self.refresh_client.clone();
        let account_id_for_refresh = account_id.clone();
        let mut refreshed = tokio::task::spawn_blocking(move || {
            refresh_client.refresh_credentials(&account_id_for_refresh, &refresh_token)
        })
        .await
        .map_err(|_error| CredentialResolverError::RefreshUnavailable)??;
        if refreshed.chatgpt_account_id().is_none()
            && let Some(chatgpt_account_id) = bundle.chatgpt_account_id()
        {
            refreshed = refreshed.with_chatgpt_account_id(chatgpt_account_id);
        }
        let refreshed_generation = current_generation
            .checked_add(1)
            .ok_or(CredentialResolverError::RefreshUnavailable)?;
        let refreshed_key = account_credential_bundle_key(account_id, refreshed_generation)
            .map_err(map_secret_error)?;
        let refreshed_secret = refreshed.to_secret_string().map_err(map_secret_error)?;
        let secret_store = self.secret_store.clone();
        tokio::task::spawn_blocking(move || {
            secret_store
                .write_secret(&refreshed_key, &refreshed_secret)
                .map_err(map_secret_error)
        })
        .await
        .map_err(|_error| CredentialResolverError::SecretUnavailable)??;
        self.state_store
            .activate_account_credential_generation_if_current_and_invalidate_quota(
                account_id,
                current_generation,
                refreshed_generation,
                AccountStatus::Enabled,
            )
            .await
            .map_err(map_state_error)?;

        Ok((refreshed_generation, refreshed))
    }
}

fn map_state_error(_error: StateStoreError) -> CredentialResolverError {
    CredentialResolverError::AccountUnavailable
}

fn map_secret_error(_error: SecretStoreError) -> CredentialResolverError {
    CredentialResolverError::SecretUnavailable
}

/// Returns the current Unix second for runtime credential freshness checks.
pub fn current_unix_seconds() -> Result<u64, std::time::SystemTimeError> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
