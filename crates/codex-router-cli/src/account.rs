//! Account onboarding commands.

use std::io::Write;
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_router_auth::router_credentials::RouterCredentialImportError;
use codex_router_auth::router_credentials::router_credentials_from_auth_text;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::account_tokens::upstream_access_token_key;
use codex_router_secret_store::account_tokens::upstream_refresh_token_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::file_backend::SecretStore;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::account::AccountCredentialMetadata;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::repositories::AccountCredentialRepository;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;

use crate::ArgumentParser;
use crate::CliError;
use crate::RouterRootPaths;

/// Account command namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AccountCommand {
    /// Import existing Codex auth.json into router-owned storage.
    ImportCodexAuth(AccountImportCodexAuthCommand),
    /// List router accounts.
    List(AccountRootCommand),
    /// Enable a router account.
    Enable(AccountSelectCommand),
    /// Disable a router account.
    Disable(AccountSelectCommand),
}

impl AccountCommand {
    pub(crate) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let Some(command) = parser.next_string()? else {
            return Err(CliError::MissingCommand {
                command: "account".to_owned(),
            });
        };

        match command.as_str() {
            "import-codex-auth" => Ok(Self::ImportCodexAuth(AccountImportCodexAuthCommand::parse(
                parser,
            )?)),
            "list" => Ok(Self::List(AccountRootCommand::parse(parser)?)),
            "enable" => Ok(Self::Enable(AccountSelectCommand::parse(parser)?)),
            "disable" => Ok(Self::Disable(AccountSelectCommand::parse(parser)?)),
            unknown => Err(CliError::UnknownCommand {
                command: format!("account {unknown}"),
            }),
        }
    }
}

/// Command with only router-root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountRootCommand {
    router_root: PathBuf,
}

impl AccountRootCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut router_root = None;
        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    router_root = Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(Self {
            router_root: router_root.ok_or(CliError::MissingOption {
                option: "--router-root",
            })?,
        })
    }
}

/// Command selecting one account by id or label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountSelectCommand {
    router_root: PathBuf,
    account: String,
}

impl AccountSelectCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut router_root = None;
        let mut account = None;
        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    router_root = Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--account" => {
                    account = Some(parser.next_required_value("--account")?);
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(Self {
            router_root: router_root.ok_or(CliError::MissingOption {
                option: "--router-root",
            })?,
            account: account.ok_or(CliError::MissingOption {
                option: "--account",
            })?,
        })
    }
}

/// `account import-codex-auth` command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountImportCodexAuthCommand {
    router_root: PathBuf,
    auth_json: PathBuf,
    label: String,
    allow_plaintext_file_secrets: bool,
}

impl AccountImportCodexAuthCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut router_root = None;
        let mut auth_json = None;
        let mut label = None;
        let mut allow_plaintext_file_secrets = false;

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    router_root = Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--auth-json" => {
                    auth_json = Some(PathBuf::from(parser.next_required_value("--auth-json")?));
                }
                "--label" => {
                    label = Some(parser.next_required_value("--label")?);
                }
                "--allow-plaintext-file-secrets" => {
                    allow_plaintext_file_secrets = true;
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(Self {
            router_root: router_root.ok_or(CliError::MissingOption {
                option: "--router-root",
            })?,
            auth_json: auth_json.ok_or(CliError::MissingOption {
                option: "--auth-json",
            })?,
            label: label.ok_or(CliError::MissingOption { option: "--label" })?,
            allow_plaintext_file_secrets,
        })
    }
}

/// Account command failure.
#[derive(Debug, Error)]
pub enum AccountCommandError {
    /// Plaintext file secret storage acknowledgement is required.
    #[error(
        "importing OAuth material into the file backend stores plaintext-at-rest secrets; rerun with --allow-plaintext-file-secrets to acknowledge"
    )]
    PlaintextAcknowledgementRequired,
    /// Labels must not contain email-like PII.
    #[error("account label must be a non-email local label")]
    EmailLikeLabel,
    /// Account label already exists.
    #[error("account label already exists")]
    AccountAlreadyExists,
    /// Auth JSON could not be read.
    #[error("failed to read auth json: {message}")]
    ReadAuth {
        /// Redacted message.
        message: String,
    },
    /// Auth JSON was not router-importable.
    #[error(transparent)]
    ImportCredentials(#[from] RouterCredentialImportError),
    /// State store failed.
    #[error(transparent)]
    State(#[from] StateStoreError),
    /// Secret store failed.
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    /// Account selector matched no account.
    #[error("account not found")]
    AccountNotFound,
    /// Account selector matched more than one account.
    #[error("account selector is ambiguous")]
    AmbiguousAccount,
    /// Stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),
}

/// Runs an account command.
pub fn run_account_command(
    stdout: &mut impl Write,
    command: AccountCommand,
) -> Result<(), AccountCommandError> {
    match command {
        AccountCommand::ImportCodexAuth(command) => import_codex_auth(stdout, command),
        AccountCommand::List(command) => list_accounts(stdout, command),
        AccountCommand::Enable(command) => {
            set_account_status(stdout, command, AccountStatus::Enabled)
        }
        AccountCommand::Disable(command) => {
            set_account_status(stdout, command, AccountStatus::Disabled)
        }
    }
}

fn import_codex_auth(
    stdout: &mut impl Write,
    command: AccountImportCodexAuthCommand,
) -> Result<(), AccountCommandError> {
    if !command.allow_plaintext_file_secrets {
        return Err(AccountCommandError::PlaintextAcknowledgementRequired);
    }
    if !is_safe_local_label(&command.label) {
        return Err(AccountCommandError::EmailLikeLabel);
    }

    let auth_text = std::fs::read_to_string(&command.auth_json).map_err(|error| {
        AccountCommandError::ReadAuth {
            message: error.to_string(),
        }
    })?;
    let credentials = router_credentials_from_auth_text(&auth_text)?;
    let account_id = account_id_for_label(&command.label);
    let paths = RouterRootPaths::new(command.router_root);
    let state_store = SqliteStateStore::open(&paths.state_db)?;
    if AccountStateRepository::load_account(&state_store, &account_id)?.is_some() {
        return Err(AccountCommandError::AccountAlreadyExists);
    }
    let secret_store = FileSecretStore::open(&paths.secret_root)?;

    let disabled_account = AccountRecord::new(
        account_id.clone(),
        command.label.clone(),
        AccountStatus::Disabled,
    );
    state_store.upsert_account(&disabled_account)?;

    let access_key = upstream_access_token_key(&account_id)?;
    secret_store.write_secret(
        &access_key,
        &SecretString::new(credentials.access_token().to_owned()),
    )?;
    let access_roundtrip = secret_store.read_secret(&access_key)?;
    if access_roundtrip.expose_secret() != credentials.access_token() {
        return Err(AccountCommandError::SecretStore(
            SecretStoreError::InvalidSecretKey {
                value: "openai_access_token_roundtrip".to_owned(),
            },
        ));
    }

    if let Some(refresh_token) = credentials.refresh_token() {
        let refresh_key = upstream_refresh_token_key(&account_id)?;
        secret_store.write_secret(&refresh_key, &SecretString::new(refresh_token.to_owned()))?;
        let refresh_roundtrip = secret_store.read_secret(&refresh_key)?;
        if refresh_roundtrip.expose_secret() != refresh_token {
            return Err(AccountCommandError::SecretStore(
                SecretStoreError::InvalidSecretKey {
                    value: "openai_refresh_token_roundtrip".to_owned(),
                },
            ));
        }
    }

    let credential_metadata = AccountCredentialMetadata::new(
        account_id.clone(),
        credentials.refresh_token().is_some(),
        credentials.expires_at_unix_seconds(),
        0,
    );
    AccountCredentialRepository::upsert_credential_metadata(&state_store, &credential_metadata)?;

    let enabled_account =
        AccountRecord::new(account_id.clone(), command.label, AccountStatus::Enabled);
    state_store.upsert_account(&enabled_account)?;

    writeln!(stdout, "account: {}", account_id.as_str()).map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "label: {}", display_label(enabled_account.label()))
        .map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "status: enabled").map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "import: codex-auth").map_err(AccountCommandError::Stdout)?;

    Ok(())
}

fn list_accounts(
    stdout: &mut impl Write,
    command: AccountRootCommand,
) -> Result<(), AccountCommandError> {
    let paths = RouterRootPaths::new(command.router_root);
    let state_store = SqliteStateStore::open_existing_read_only(&paths.state_db)?;
    for account in AccountStateRepository::list_accounts(&state_store)? {
        let credential_metadata = AccountCredentialRepository::load_credential_metadata(
            &state_store,
            account.account_id(),
        )?;
        let refresh = credential_metadata
            .as_ref()
            .map(|metadata| {
                if metadata.has_refresh_token() {
                    "present"
                } else {
                    "missing"
                }
            })
            .unwrap_or("unknown");
        let expires = credential_metadata
            .and_then(|metadata| metadata.expires_at_unix_seconds())
            .map(|expires_at| expires_at.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        writeln!(
            stdout,
            "{}\t{}\t{}\trefresh={}\texpires_at={}",
            account.account_id().as_str(),
            display_label(account.label()),
            account.status().as_str(),
            refresh,
            expires,
        )
        .map_err(AccountCommandError::Stdout)?;
    }

    Ok(())
}

fn set_account_status(
    stdout: &mut impl Write,
    command: AccountSelectCommand,
    status: AccountStatus,
) -> Result<(), AccountCommandError> {
    let paths = RouterRootPaths::new(command.router_root);
    let state_store = SqliteStateStore::open(&paths.state_db)?;
    let account = resolve_account(&state_store, &command.account)?;
    let updated = AccountRecord::new(account.account_id().clone(), account.label(), status);
    AccountStateRepository::upsert_account(&state_store, &updated)?;

    writeln!(stdout, "account: {}", updated.account_id().as_str())
        .map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "label: {}", display_label(updated.label()))
        .map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "status: {}", updated.status().as_str())
        .map_err(AccountCommandError::Stdout)?;

    Ok(())
}

fn resolve_account(
    state_store: &SqliteStateStore,
    selector: &str,
) -> Result<AccountRecord, AccountCommandError> {
    let accounts = AccountStateRepository::list_accounts(state_store)?;
    let matches: Vec<AccountRecord> = accounts
        .into_iter()
        .filter(|account| account.account_id().as_str() == selector || account.label() == selector)
        .collect();

    match matches.as_slice() {
        [] => Err(AccountCommandError::AccountNotFound),
        [account] => Ok(account.clone()),
        _ => Err(AccountCommandError::AmbiguousAccount),
    }
}

fn account_id_for_label(label: &str) -> AccountId {
    let digest = Sha256::digest(label.as_bytes());
    let encoded = URL_SAFE_NO_PAD.encode(&digest[..12]);
    match AccountId::new(format!("acct_{encoded}")) {
        Ok(account_id) => account_id,
        Err(error) => panic!("derived account id must be non-empty: {error}"),
    }
}

fn is_safe_local_label(label: &str) -> bool {
    !label.is_empty()
        && label.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn display_label(label: &str) -> String {
    label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}
