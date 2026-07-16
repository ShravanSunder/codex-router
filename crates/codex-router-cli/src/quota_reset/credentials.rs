//! Read-only account and credential loading for quota reset.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_core::redaction::safe_account_label;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_state::account::AccountStatus;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use sha2::Digest;
use sha2::Sha256;
use sqlx::ConnectOptions;
use sqlx::Row;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use thiserror::Error;
use tokio::sync::Semaphore;

use super::QuotaResetError;
use super::domain::ActiveCredentialGeneration;

const CREDENTIAL_FINGERPRINT_DOMAIN: &[u8] = b"codex-router/quota-reset/provider-authority/v1";
const MAX_CONCURRENT_CREDENTIAL_READS: usize = 4;
static CREDENTIAL_READ_PERMITS: Semaphore = Semaphore::const_new(MAX_CONCURRENT_CREDENTIAL_READS);

/// Fail-closed errors while resolving provider-effective reset authority.
#[derive(Debug, Error)]
pub(in crate::quota_reset) enum CredentialAuthorityError {
    #[error(transparent)]
    Secret(#[from] codex_router_secret_store::model::SecretStoreError),
    #[error("read-only account state query failed")]
    StateReadFailed,
    #[error("selected account is unavailable")]
    AccountUnavailable,
    #[error("selected account credential generation changed")]
    GenerationChanged,
    #[error("selected account credential is expired")]
    Expired,
    #[error("selected account credential is missing provider routing")]
    MissingRoutingId,
    #[error("read-only credential task failed")]
    CredentialTaskFailed,
}

/// Opaque comparison value binding every provider-effective credential field.
#[derive(Eq, PartialEq)]
pub(in crate::quota_reset) struct CredentialFingerprint([u8; 32]);

impl fmt::Debug for CredentialFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialFingerprint([REDACTED])")
    }
}

/// Ephemeral provider authority. It is never serialized or persisted.
pub(in crate::quota_reset) struct PinnedResetAuthority {
    account_id: AccountId,
    active_credential_generation: ActiveCredentialGeneration,
    access_token: SecretString,
    chatgpt_account_id: String,
    expires_unix_seconds: Option<u64>,
    fingerprint: CredentialFingerprint,
}

impl PinnedResetAuthority {
    pub(in crate::quota_reset) const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub(in crate::quota_reset) const fn active_credential_generation(
        &self,
    ) -> ActiveCredentialGeneration {
        self.active_credential_generation
    }

    pub(in crate::quota_reset) const fn access_token(&self) -> &SecretString {
        &self.access_token
    }

    pub(in crate::quota_reset) fn chatgpt_account_id(&self) -> &str {
        &self.chatgpt_account_id
    }

    pub(in crate::quota_reset) const fn expires_unix_seconds(&self) -> Option<u64> {
        self.expires_unix_seconds
    }

    pub(in crate::quota_reset) const fn fingerprint(&self) -> &CredentialFingerprint {
        &self.fingerprint
    }
}

impl fmt::Debug for PinnedResetAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedResetAuthority")
            .field("account_id", &"[REDACTED]")
            .field("active_credential_generation", &"[REDACTED]")
            .field("access_token", &"[REDACTED]")
            .field("chatgpt_account_id", &"[REDACTED]")
            .field("expires_unix_seconds", &"[REDACTED]")
            .field("fingerprint", &"[REDACTED]")
            .finish()
    }
}

/// Resolves one exact account and credential generation without refresh or persistence.
pub(in crate::quota_reset) async fn load_reset_credential_authority(
    state_database_path: &Path,
    secret_root: &Path,
    account_id: &AccountId,
    expected_generation: ActiveCredentialGeneration,
    now_unix_seconds: u64,
) -> Result<PinnedResetAuthority, CredentialAuthorityError> {
    let options = SqliteConnectOptions::new()
        .filename(state_database_path)
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::ZERO)
        .disable_statement_logging()
        .pragma("query_only", "ON");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_error| CredentialAuthorityError::StateReadFailed)?;
    let account_row = sqlx::query(
        "SELECT status, active_credential_generation
           FROM accounts
          WHERE account_id = ?1",
    )
    .bind(account_id.as_str())
    .fetch_optional(&pool)
    .await
    .map_err(|_error| CredentialAuthorityError::StateReadFailed);
    pool.close().await;
    let account_row = account_row?.ok_or(CredentialAuthorityError::AccountUnavailable)?;
    let status = AccountStatus::parse(account_row.get::<String, _>(0).as_str())
        .ok_or(CredentialAuthorityError::StateReadFailed)?;
    if status != AccountStatus::Enabled {
        return Err(CredentialAuthorityError::AccountUnavailable);
    }
    let active_generation = account_row
        .get::<Option<i64>, _>(1)
        .map(u64::try_from)
        .transpose()
        .map_err(|_error| CredentialAuthorityError::StateReadFailed)?;
    if active_generation != Some(expected_generation.get()) {
        return Err(CredentialAuthorityError::GenerationChanged);
    }

    let permit = CREDENTIAL_READ_PERMITS
        .acquire()
        .await
        .map_err(|_error| CredentialAuthorityError::CredentialTaskFailed)?;
    let secret_root = secret_root.to_path_buf();
    let account_id = account_id.clone();
    let authority = tokio::task::spawn_blocking(move || {
        load_exact_credential_authority(
            &secret_root,
            account_id,
            expected_generation,
            now_unix_seconds,
        )
    })
    .await
    .map_err(|_error| CredentialAuthorityError::CredentialTaskFailed)?;
    drop(permit);
    authority
}

fn load_exact_credential_authority(
    secret_root: &Path,
    account_id: AccountId,
    active_credential_generation: ActiveCredentialGeneration,
    now_unix_seconds: u64,
) -> Result<PinnedResetAuthority, CredentialAuthorityError> {
    let store = FileSecretStore::open_read_only(secret_root)?;
    let bundle_key =
        account_credential_bundle_key(&account_id, active_credential_generation.get())?;
    let bundle = AccountCredentialBundle::from_secret_string(store.read_secret(&bundle_key)?)?;
    let expires_unix_seconds = bundle.expires_unix_seconds();
    if expires_unix_seconds.is_some_and(|expires_at| expires_at <= now_unix_seconds) {
        return Err(CredentialAuthorityError::Expired);
    }
    let chatgpt_account_id = bundle
        .chatgpt_account_id()
        .filter(|routing_id| !routing_id.trim().is_empty())
        .map(str::to_owned)
        .ok_or(CredentialAuthorityError::MissingRoutingId)?;
    let fingerprint = credential_fingerprint(
        &account_id,
        active_credential_generation,
        bundle.access_token(),
        &chatgpt_account_id,
        expires_unix_seconds,
    );
    Ok(PinnedResetAuthority {
        account_id,
        active_credential_generation,
        access_token: bundle.access_token().clone(),
        chatgpt_account_id,
        expires_unix_seconds,
        fingerprint,
    })
}

fn credential_fingerprint(
    account_id: &AccountId,
    active_credential_generation: ActiveCredentialGeneration,
    access_token: &SecretString,
    chatgpt_account_id: &str,
    expires_unix_seconds: Option<u64>,
) -> CredentialFingerprint {
    let generation = active_credential_generation.get().to_be_bytes();
    let expiry_marker = [u8::from(expires_unix_seconds.is_some())];
    let expiry = expires_unix_seconds.unwrap_or_default().to_be_bytes();
    credential_fingerprint_from_fields(&[
        account_id.as_str().as_bytes(),
        &generation,
        access_token.expose_secret().as_bytes(),
        chatgpt_account_id.as_bytes(),
        &expiry_marker,
        &expiry,
    ])
}

fn credential_fingerprint_from_fields(fields: &[&[u8]]) -> CredentialFingerprint {
    let mut digest = Sha256::new();
    digest.update(CREDENTIAL_FINGERPRINT_DOMAIN);
    for field in fields {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    CredentialFingerprint(digest.finalize().into())
}

#[cfg(test)]
fn credential_fingerprint_for_test(fields: &[&[u8]]) -> CredentialFingerprint {
    credential_fingerprint_from_fields(fields)
}

/// Non-secret account choice shown by the interactive reset picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResetAccountChoice {
    pub(crate) account_id: AccountId,
    pub(crate) label: String,
    pub(crate) account_tag: String,
    active_credential_generation: u64,
}

#[cfg(test)]
impl ResetAccountChoice {
    pub(crate) fn for_test(
        account_id: AccountId,
        label: impl Into<String>,
        active_credential_generation: u64,
    ) -> Self {
        let account_tag = account_display_tag(&account_id);
        Self {
            account_id,
            label: label.into(),
            account_tag,
            active_credential_generation,
        }
    }
}

/// Selected account credentials loaded without refresh or persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadOnlyResetCredential {
    pub(crate) access_token: SecretString,
    pub(crate) chatgpt_account_id: String,
}

/// Loads eligible account metadata from SQLite's query-only connection mode.
pub(crate) async fn load_reset_account_choices(
    state_database_path: &Path,
) -> Result<Vec<ResetAccountChoice>, QuotaResetError> {
    let state = AsyncSqliteStateStore::open_read_only(state_database_path).await?;
    let accounts = state.list_accounts().await?;
    state.close().await?;

    Ok(accounts
        .into_iter()
        .filter(|account| account.status() == AccountStatus::Enabled)
        .filter_map(|account| {
            account
                .active_credential_generation()
                .map(|active_credential_generation| ResetAccountChoice {
                    account_tag: account_display_tag(account.account_id()),
                    account_id: account.account_id().clone(),
                    label: safe_account_label(account.label(), account.account_id())
                        .as_str()
                        .to_owned(),
                    active_credential_generation,
                })
        })
        .collect())
}

fn account_display_tag(account_id: &AccountId) -> String {
    let digest = Sha256::digest(account_id.as_str().as_bytes());
    digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Loads the selected active bundle without refreshing or writing any state.
pub(crate) async fn load_read_only_reset_credential(
    secret_root: &Path,
    account: &ResetAccountChoice,
    now_unix_seconds: u64,
) -> Result<ReadOnlyResetCredential, QuotaResetError> {
    let secret_root = secret_root.to_path_buf();
    let account_id = account.account_id.clone();
    let active_credential_generation = account.active_credential_generation;
    tokio::task::spawn_blocking(move || {
        let store = FileSecretStore::open_read_only(secret_root)?;
        let bundle_key = account_credential_bundle_key(&account_id, active_credential_generation)?;
        let bundle = AccountCredentialBundle::from_secret_string(store.read_secret(&bundle_key)?)?;
        if bundle
            .expires_unix_seconds()
            .is_some_and(|expires_at| expires_at <= now_unix_seconds)
        {
            return Err(QuotaResetError::ExpiredCredential);
        }
        let chatgpt_account_id = bundle
            .chatgpt_account_id()
            .filter(|account_id| !account_id.trim().is_empty())
            .map(str::to_owned)
            .ok_or(QuotaResetError::MissingChatGptAccountId)?;
        Ok(ReadOnlyResetCredential {
            access_token: bundle.access_token().clone(),
            chatgpt_account_id,
        })
    })
    .await
    .map_err(|_error| QuotaResetError::CredentialTaskFailed)?
}

#[cfg(test)]
mod tests;
