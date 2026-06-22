//! Account command glue for router-owned account state.

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::account_tokens::upstream_access_token_key;
use codex_router_secret_store::account_tokens::upstream_refresh_token_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::file_backend::SecretStore;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use thiserror::Error;

use crate::ArgumentParser;
use crate::CliError;

/// Account CLI command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccountCommand {
    /// Imports an existing Codex OAuth auth.json into router-owned storage.
    ImportCodexAuth {
        /// Router-owned root.
        router_root: PathBuf,
        /// Display label.
        label: String,
        /// Source auth.json path.
        auth_json: PathBuf,
        /// Explicit plaintext file-backend acknowledgement.
        allow_plaintext_file_secrets: bool,
    },
    /// Lists router-owned accounts.
    List {
        /// Router-owned root.
        router_root: PathBuf,
    },
}

impl AccountCommand {
    pub(crate) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let Some(command) = parser.next_string()? else {
            return Err(CliError::MissingCommand {
                command: "account".to_owned(),
            });
        };

        match command.as_str() {
            "import-codex-auth" => {
                let options = AccountImportOptions::parse(parser)?;
                Ok(Self::ImportCodexAuth {
                    router_root: options.router_root()?,
                    label: options.label()?,
                    auth_json: options.auth_json()?,
                    allow_plaintext_file_secrets: options.allow_plaintext_file_secrets,
                })
            }
            "list" => {
                let options = AccountRootOptions::parse(parser)?;
                Ok(Self::List {
                    router_root: options.router_root()?,
                })
            }
            unknown => Err(CliError::UnknownCommand {
                command: format!("account {unknown}"),
            }),
        }
    }
}

/// Account command failure.
#[derive(Debug, Error)]
pub enum AccountCommandError {
    /// Plaintext file-backed import needs explicit acknowledgement.
    #[error("account import-codex-auth requires --allow-plaintext-file-secrets")]
    PlaintextFileSecretsNotAllowed,
    /// Router root creation failed.
    #[error("failed to create router root {path}: {source}")]
    CreateRouterRoot {
        /// Router root path.
        path: PathBuf,
        /// IO source.
        #[source]
        source: std::io::Error,
    },
    /// Auth JSON read failed.
    #[error("failed to read auth json: {message}")]
    ReadAuthJson {
        /// Redacted message.
        message: String,
    },
    /// Auth JSON parse failed.
    #[error("failed to parse auth json: {message}")]
    ParseAuthJson {
        /// Redacted message.
        message: String,
    },
    /// API-key auth cannot be imported as quota-compatible OAuth state.
    #[error("account import-codex-auth requires Codex OAuth auth.json, not API-key auth")]
    ApiKeyAuth,
    /// Access token was missing.
    #[error("access token not found in auth json")]
    MissingAccessToken,
    /// Display label was empty.
    #[error("account label must not be empty")]
    EmptyLabel,
    /// Secret-store operation failed.
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    /// State-store operation failed.
    #[error(transparent)]
    StateStore(#[from] StateStoreError),
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
        AccountCommand::ImportCodexAuth {
            router_root,
            label,
            auth_json,
            allow_plaintext_file_secrets,
        } => import_codex_auth(
            stdout,
            router_root,
            label,
            auth_json,
            allow_plaintext_file_secrets,
        ),
        AccountCommand::List { router_root } => list_accounts(stdout, router_root),
    }
}

fn import_codex_auth(
    stdout: &mut impl Write,
    router_root: PathBuf,
    label: String,
    auth_json: PathBuf,
    allow_plaintext_file_secrets: bool,
) -> Result<(), AccountCommandError> {
    if !allow_plaintext_file_secrets {
        return Err(AccountCommandError::PlaintextFileSecretsNotAllowed);
    }

    let trimmed_label = normalize_label(&label)?;
    let account_id = account_id_from_label(&trimmed_label)?;
    let auth_text =
        std::fs::read_to_string(&auth_json).map_err(|error| AccountCommandError::ReadAuthJson {
            message: error.to_string(),
        })?;
    let imported_auth = ImportedCodexAuth::parse(&auth_text)?;

    create_router_root(&router_root)?;
    let state = SqliteStateStore::open(&router_root.join("state.sqlite"))?;
    let secrets = FileSecretStore::open(router_root.join("secrets"))?;

    let account = AccountRecord::new(
        account_id.clone(),
        trimmed_label.clone(),
        AccountStatus::Enabled,
    );
    AccountStateRepository::upsert_account(&state, &account)?;
    let access_key = upstream_access_token_key(&account_id)?;
    secrets.write_secret(
        &access_key,
        &SecretString::new(imported_auth.access_token.clone()),
    )?;
    if let Some(refresh_token) = imported_auth.refresh_token {
        let refresh_key = upstream_refresh_token_key(&account_id)?;
        secrets.write_secret(&refresh_key, &SecretString::new(refresh_token))?;
    }

    writeln!(stdout, "imported account: {trimmed_label}").map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "account_id: {}", account_id.as_str()).map_err(AccountCommandError::Stdout)
}

fn list_accounts(stdout: &mut impl Write, router_root: PathBuf) -> Result<(), AccountCommandError> {
    let state = SqliteStateStore::open(&router_root.join("state.sqlite"))?;
    let accounts = AccountStateRepository::list_accounts(&state)?;
    for account in accounts {
        writeln!(
            stdout,
            "{}\t{}\t{}",
            account.account_id().as_str(),
            account.label(),
            account.status().as_str()
        )
        .map_err(AccountCommandError::Stdout)?;
    }

    Ok(())
}

fn create_router_root(router_root: &Path) -> Result<(), AccountCommandError> {
    std::fs::create_dir_all(router_root).map_err(|source| AccountCommandError::CreateRouterRoot {
        path: router_root.to_path_buf(),
        source,
    })
}

fn normalize_label(label: &str) -> Result<String, AccountCommandError> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Err(AccountCommandError::EmptyLabel);
    }

    Ok(trimmed.to_owned())
}

fn account_id_from_label(label: &str) -> Result<AccountId, AccountCommandError> {
    let mut normalized = String::new();
    let mut previous_was_separator = false;
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            normalized.extend(character.to_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator {
            normalized.push('_');
            previous_was_separator = true;
        }
    }
    let normalized = normalized.trim_matches('_');
    let stem = if normalized.is_empty() {
        "imported"
    } else {
        normalized
    };

    AccountId::new(format!("acct_{stem}")).map_err(|_| AccountCommandError::EmptyLabel)
}

struct ImportedCodexAuth {
    access_token: String,
    refresh_token: Option<String>,
}

impl ImportedCodexAuth {
    fn parse(auth_text: &str) -> Result<Self, AccountCommandError> {
        let value: serde_json::Value = serde_json::from_str(auth_text).map_err(|error| {
            AccountCommandError::ParseAuthJson {
                message: error.to_string(),
            }
        })?;
        let auth_mode = value
            .get("auth_mode")
            .and_then(serde_json::Value::as_str)
            .map(normalize_auth_mode);
        let has_api_key = value
            .get("OPENAI_API_KEY")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|api_key| !api_key.trim().is_empty());
        if auth_mode.as_deref() == Some("apikey") || has_api_key {
            return Err(AccountCommandError::ApiKeyAuth);
        }

        let tokens = value
            .get("tokens")
            .and_then(serde_json::Value::as_object)
            .ok_or(AccountCommandError::MissingAccessToken)?;
        let access_token = tokens
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or(AccountCommandError::MissingAccessToken)?
            .to_owned();
        let refresh_token = tokens
            .get("refresh_token")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_owned);

        Ok(Self {
            access_token,
            refresh_token,
        })
    }
}

fn normalize_auth_mode(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !matches!(character, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct AccountImportOptions {
    router_root: Option<PathBuf>,
    label: Option<String>,
    auth_json: Option<PathBuf>,
    allow_plaintext_file_secrets: bool,
}

impl AccountImportOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--label" => {
                    options.label = Some(parser.next_required_value("--label")?);
                }
                "--auth-json" => {
                    options.auth_json =
                        Some(PathBuf::from(parser.next_required_value("--auth-json")?));
                }
                "--allow-plaintext-file-secrets" => {
                    options.allow_plaintext_file_secrets = true;
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(options)
    }

    fn router_root(&self) -> Result<PathBuf, CliError> {
        self.router_root.clone().ok_or(CliError::MissingOption {
            option: "--router-root",
        })
    }

    fn label(&self) -> Result<String, CliError> {
        self.label
            .clone()
            .ok_or(CliError::MissingOption { option: "--label" })
    }

    fn auth_json(&self) -> Result<PathBuf, CliError> {
        self.auth_json.clone().ok_or(CliError::MissingOption {
            option: "--auth-json",
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct AccountRootOptions {
    router_root: Option<PathBuf>,
}

impl AccountRootOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(options)
    }

    fn router_root(self) -> Result<PathBuf, CliError> {
        self.router_root.ok_or(CliError::MissingOption {
            option: "--router-root",
        })
    }
}
