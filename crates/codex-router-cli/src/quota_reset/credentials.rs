//! Read-only account and credential loading for quota reset.

use std::path::Path;

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

use super::QuotaResetError;

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
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use codex_router_secret_store::account_tokens::AccountCredentialBundle;
    use codex_router_secret_store::account_tokens::account_credential_bundle_key;
    use codex_router_secret_store::file_backend::FileSecretStore;
    use codex_router_state::account::AccountRecord;
    use codex_router_state::account::AccountStatus;

    use super::*;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn read_only_credential_loading_refuses_expired_bundle_without_refresh() {
        let root = std::env::temp_dir().join(format!(
            "codex-router-reset-credential-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = FileSecretStore::open(&root)
            .unwrap_or_else(|error| panic!("fixture secret store should open: {error}"));
        let account = ResetAccountChoice::for_test(
            AccountId::new("acct_expired")
                .unwrap_or_else(|error| panic!("fixture account id should parse: {error}")),
            "expired",
            1,
        );
        let key = account_credential_bundle_key(&account.account_id, 1)
            .unwrap_or_else(|error| panic!("fixture key should parse: {error}"));
        let bundle = AccountCredentialBundle::imported_codex_auth("expired-token", None)
            .with_expires_unix_seconds(100);
        store
            .write_secret(
                &key,
                &bundle
                    .to_secret_string()
                    .unwrap_or_else(|error| panic!("fixture bundle should serialize: {error}")),
            )
            .unwrap_or_else(|error| panic!("fixture bundle should write: {error}"));

        let result = load_read_only_reset_credential(&root, &account, 101).await;

        assert!(matches!(result, Err(QuotaResetError::ExpiredCredential)));
        std::fs::remove_dir_all(&root)
            .unwrap_or_else(|error| panic!("fixture root should clean up: {error}"));
    }

    #[tokio::test]
    async fn read_only_credential_loading_requires_exact_chatgpt_account_routing() {
        let root = std::env::temp_dir().join(format!(
            "codex-router-reset-credential-account-id-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = FileSecretStore::open(&root)
            .unwrap_or_else(|error| panic!("fixture secret store should open: {error}"));
        let account = ResetAccountChoice::for_test(
            AccountId::new("acct_missing_header")
                .unwrap_or_else(|error| panic!("fixture account id should parse: {error}")),
            "missing-header",
            1,
        );
        let key = account_credential_bundle_key(&account.account_id, 1)
            .unwrap_or_else(|error| panic!("fixture key should parse: {error}"));
        let bundle = AccountCredentialBundle::imported_codex_auth("current-token", None)
            .with_expires_unix_seconds(1_000);
        store
            .write_secret(
                &key,
                &bundle
                    .to_secret_string()
                    .unwrap_or_else(|error| panic!("fixture bundle should serialize: {error}")),
            )
            .unwrap_or_else(|error| panic!("fixture bundle should write: {error}"));

        let result = load_read_only_reset_credential(&root, &account, 100).await;

        assert!(matches!(
            result,
            Err(QuotaResetError::MissingChatGptAccountId)
        ));
        std::fs::remove_dir_all(&root)
            .unwrap_or_else(|error| panic!("fixture root should clean up: {error}"));
    }

    #[tokio::test]
    async fn reset_account_choices_read_sqlite_without_mutating_database_bytes() {
        let root = std::env::temp_dir().join(format!(
            "codex-router-reset-accounts-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root)
            .unwrap_or_else(|error| panic!("fixture root should create: {error}"));
        let database_path = root.join("state.sqlite");
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture database should open: {error}"));
        let account_id = AccountId::new("acct_read_only")
            .unwrap_or_else(|error| panic!("fixture account id should parse: {error}"));
        state
            .upsert_account(
                &AccountRecord::new(account_id.clone(), "read-only", AccountStatus::Enabled)
                    .with_active_credential_generation(7),
            )
            .await
            .unwrap_or_else(|error| panic!("fixture account should write: {error}"));
        state
            .close()
            .await
            .unwrap_or_else(|error| panic!("fixture database should close: {error}"));
        let before = std::fs::read(&database_path)
            .unwrap_or_else(|error| panic!("fixture database should read: {error}"));

        let choices = load_reset_account_choices(&database_path)
            .await
            .unwrap_or_else(|error| panic!("read-only choices should load: {error}"));
        let after = std::fs::read(&database_path)
            .unwrap_or_else(|error| panic!("fixture database should reread: {error}"));

        assert_eq!(choices.len(), 1);
        assert_eq!(
            choices.first().map(|choice| &choice.account_id),
            Some(&account_id)
        );
        assert_eq!(before, after);
        std::fs::remove_dir_all(&root)
            .unwrap_or_else(|error| panic!("fixture root should clean up: {error}"));
    }
}
