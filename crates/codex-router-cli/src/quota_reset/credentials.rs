//! Read-only account and credential loading for quota reset.

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_state::account::AccountStatus;
use sha2::Digest;
use sha2::Sha256;
use sqlx::ConnectOptions;
use sqlx::Row;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use thiserror::Error;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use super::domain::ActiveCredentialGeneration;

const CREDENTIAL_FINGERPRINT_DOMAIN: &[u8] = b"codex-router/quota-reset/provider-authority/v1";
const MAX_CONCURRENT_CREDENTIAL_READS: usize = 4;
static CREDENTIAL_READ_PERMITS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_CREDENTIAL_READS)));

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

/// A validated credential read that has not started blocking secret-store work.
///
/// Dropping this value is cancellation-safe: it only releases reserved capacity.
#[must_use = "prepared credential reads must be started or explicitly dropped"]
pub(in crate::quota_reset) struct PreparedCredentialAuthorityRead {
    secret_root: std::path::PathBuf,
    account_id: AccountId,
    expected_generation: ActiveCredentialGeneration,
    now_unix_seconds: u64,
    permit: OwnedSemaphorePermit,
}

impl PreparedCredentialAuthorityRead {
    /// Starts the blocking secret read and transfers its capacity permit into that operation.
    pub(in crate::quota_reset) fn start(self) -> StartedCredentialAuthorityRead {
        let Self {
            secret_root,
            account_id,
            expected_generation,
            now_unix_seconds,
            permit,
        } = self;
        StartedCredentialAuthorityRead {
            operation: start_bounded_blocking_read(permit, move || {
                load_exact_credential_authority(
                    &secret_root,
                    account_id,
                    expected_generation,
                    now_unix_seconds,
                )
            }),
        }
    }
}

/// A started blocking credential read retained for command-supervisor drainage.
#[must_use = "started credential reads must be drained before session cleanup"]
pub(in crate::quota_reset) struct StartedCredentialAuthorityRead {
    operation: BlockingReadOperation<Result<PinnedResetAuthority, CredentialAuthorityError>>,
}

impl StartedCredentialAuthorityRead {
    /// Waits for the real blocking read without consuming this owner while pending.
    ///
    /// The mutable-borrowed join is cancellation-safe in `select!`: if another branch wins, the
    /// supervisor still owns this operation and can call `drain` again during cleanup.
    pub(in crate::quota_reset) async fn drain(
        &mut self,
    ) -> Result<PinnedResetAuthority, CredentialAuthorityError> {
        self.operation
            .drain()
            .await
            .map_err(|_error| CredentialAuthorityError::CredentialTaskFailed)?
    }
}

struct BlockingReadOperation<TResult> {
    task: JoinHandle<TResult>,
}

impl<TResult> BlockingReadOperation<TResult>
where
    TResult: Send + 'static,
{
    async fn drain(&mut self) -> Result<TResult, tokio::task::JoinError> {
        (&mut self.task).await
    }
}

fn start_bounded_blocking_read<TRead, TResult>(
    permit: OwnedSemaphorePermit,
    read: TRead,
) -> BlockingReadOperation<TResult>
where
    TRead: FnOnce() -> TResult + Send + 'static,
    TResult: Send + 'static,
{
    BlockingReadOperation {
        task: tokio::task::spawn_blocking(move || {
            let _permit = permit;
            read()
        }),
    }
}

/// Resolves one exact account and credential generation without refresh or persistence.
pub(in crate::quota_reset) async fn prepare_reset_credential_authority_read(
    state_database_path: &Path,
    secret_root: &Path,
    account_id: &AccountId,
    expected_generation: ActiveCredentialGeneration,
    now_unix_seconds: u64,
) -> Result<PreparedCredentialAuthorityRead, CredentialAuthorityError> {
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

    let permit = Arc::clone(&CREDENTIAL_READ_PERMITS)
        .acquire_owned()
        .await
        .map_err(|_error| CredentialAuthorityError::CredentialTaskFailed)?;
    Ok(PreparedCredentialAuthorityRead {
        secret_root: secret_root.to_path_buf(),
        account_id: account_id.clone(),
        expected_generation,
        now_unix_seconds,
        permit,
    })
}

/// Convenience adapter for callers that do not yet supervise the two-phase read directly.
#[cfg(test)]
pub(in crate::quota_reset) async fn load_reset_credential_authority(
    state_database_path: &Path,
    secret_root: &Path,
    account_id: &AccountId,
    expected_generation: ActiveCredentialGeneration,
    now_unix_seconds: u64,
) -> Result<PinnedResetAuthority, CredentialAuthorityError> {
    let mut read = prepare_reset_credential_authority_read(
        state_database_path,
        secret_root,
        account_id,
        expected_generation,
        now_unix_seconds,
    )
    .await?
    .start();
    read.drain().await
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

#[cfg(test)]
mod tests;
