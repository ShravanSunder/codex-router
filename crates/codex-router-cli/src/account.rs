//! Account command glue for router-owned account state.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_router_core::ids::AccountId;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use comfy_table::Table;
use comfy_table::presets::UTF8_FULL;
use thiserror::Error;

use crate::ArgumentParser;
use crate::CliError;
use crate::router_root_or_default;

/// Account CLI command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccountCommand {
    /// Prints account command help.
    Help(&'static str),
    /// Logs in from an existing Codex OAuth auth.json into router-owned storage.
    LoginAuthJson {
        /// Router-owned root.
        router_root: PathBuf,
        /// Display label.
        label: String,
        /// Source auth.json path.
        auth_json: PathBuf,
        /// Explicit plaintext file-backend acknowledgement.
        allow_plaintext_file_secrets: bool,
    },
    /// Delegates device-code login to Codex, then imports the resulting auth.json.
    LoginDeviceAuth {
        /// Router-owned root.
        router_root: PathBuf,
        /// Display label.
        label: String,
        /// Codex executable to run.
        codex_bin: PathBuf,
        /// Explicit plaintext file-backend acknowledgement.
        allow_plaintext_file_secrets: bool,
    },
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
            "--help" | "-h" | "help" => {
                parser.reject_remaining()?;
                Ok(Self::Help(ACCOUNT_HELP_TEXT))
            }
            "login" => {
                if parser.next_if_help()? {
                    parser.reject_remaining()?;
                    return Ok(Self::Help(ACCOUNT_LOGIN_HELP_TEXT));
                }
                let options = AccountLoginOptions::parse(parser)?;
                match options.method()? {
                    AccountLoginMethod::AuthJson(auth_json) => Ok(Self::LoginAuthJson {
                        router_root: options.router_root()?,
                        label: options.label()?,
                        auth_json,
                        allow_plaintext_file_secrets: options.allow_plaintext_file_secrets,
                    }),
                    AccountLoginMethod::DeviceAuth { codex_bin } => Ok(Self::LoginDeviceAuth {
                        router_root: options.router_root()?,
                        label: options.label()?,
                        codex_bin,
                        allow_plaintext_file_secrets: options.allow_plaintext_file_secrets,
                    }),
                }
            }
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
                if parser.next_if_help()? {
                    parser.reject_remaining()?;
                    return Ok(Self::Help(ACCOUNT_LIST_HELP_TEXT));
                }
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
    #[error("account login/import requires --allow-plaintext-file-secrets")]
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
    /// Login source was missing or ambiguous.
    #[error("account login requires exactly one of --auth-json or --device-auth")]
    LoginMethodRequired,
    /// Device-auth process failed to start.
    #[error("failed to start codex device-auth login {path}: {source}")]
    DeviceAuthLaunch {
        /// Codex executable path.
        path: PathBuf,
        /// IO source.
        #[source]
        source: std::io::Error,
    },
    /// Device-auth process failed.
    #[error("codex device-auth login failed with status {status}")]
    DeviceAuthFailed {
        /// Process status.
        status: String,
    },
    /// Temporary Codex home creation failed.
    #[error("failed to create temporary Codex home {path}: {source}")]
    CreateTemporaryCodexHome {
        /// Temporary Codex home path.
        path: PathBuf,
        /// IO source.
        #[source]
        source: std::io::Error,
    },
    /// Temporary Codex home cleanup failed.
    #[error("failed to remove temporary Codex home {path}: {source}")]
    RemoveTemporaryCodexHome {
        /// Temporary Codex home path.
        path: PathBuf,
        /// IO source.
        #[source]
        source: std::io::Error,
    },
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
        AccountCommand::Help(text) => stdout
            .write_all(text.as_bytes())
            .map_err(AccountCommandError::Stdout),
        AccountCommand::LoginAuthJson {
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
            AccountImportOutputMode::Login,
        ),
        AccountCommand::LoginDeviceAuth {
            router_root,
            label,
            codex_bin,
            allow_plaintext_file_secrets,
        } => login_with_codex_device_auth(
            stdout,
            router_root,
            label,
            codex_bin,
            allow_plaintext_file_secrets,
        ),
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
            AccountImportOutputMode::Import,
        ),
        AccountCommand::List { router_root } => list_accounts(stdout, router_root),
    }
}

const ACCOUNT_HELP_TEXT: &str = "\
codex-router account

commands:
  login --label <name>  Add an OAuth account with device-code login
  list                  Show configured router accounts
";

const ACCOUNT_LOGIN_HELP_TEXT: &str = "\
codex-router account login --label <name>

Adds an OAuth account to router-owned storage.

options:
  --label <name>         Friendly account name shown in quota and account list
  --codex-bin <path>     Codex binary to use for device-code login [default: codex]
";

const ACCOUNT_LIST_HELP_TEXT: &str = "\
codex-router account list

Shows configured router accounts.
";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccountImportOutputMode {
    Import,
    Login,
}

fn import_codex_auth(
    stdout: &mut impl Write,
    router_root: PathBuf,
    label: String,
    auth_json: PathBuf,
    allow_plaintext_file_secrets: bool,
    output_mode: AccountImportOutputMode,
) -> Result<(), AccountCommandError> {
    if !allow_plaintext_file_secrets {
        return Err(AccountCommandError::PlaintextFileSecretsNotAllowed);
    }

    let auth_text =
        std::fs::read_to_string(&auth_json).map_err(|error| AccountCommandError::ReadAuthJson {
            message: error.to_string(),
        })?;
    import_codex_auth_text(stdout, router_root, label, &auth_text, output_mode)
}

fn import_codex_auth_text(
    stdout: &mut impl Write,
    router_root: PathBuf,
    label: String,
    auth_text: &str,
    output_mode: AccountImportOutputMode,
) -> Result<(), AccountCommandError> {
    let trimmed_label = normalize_label(&label)?;
    let account_id = account_id_from_label(&trimmed_label)?;
    let imported_auth = ImportedCodexAuth::parse(auth_text)?;

    create_router_root(&router_root)?;
    let state = SqliteStateStore::open(&router_root.join("state.sqlite"))?;
    let secrets = FileSecretStore::open(router_root.join("secrets"))?;

    let mut request = AccountImportRequest::new(
        account_id.clone(),
        trimmed_label.clone(),
        imported_auth.access_token,
    )
    .with_optional_refresh_token(imported_auth.refresh_token);
    if let Some(chatgpt_account_id) = imported_auth.chatgpt_account_id {
        request = request.with_chatgpt_account_id(chatgpt_account_id);
    }
    import_codex_auth_from_request(&state, &secrets, request)?;

    match output_mode {
        AccountImportOutputMode::Import => {
            writeln!(stdout, "imported account: {trimmed_label}")
                .map_err(AccountCommandError::Stdout)?;
        }
        AccountImportOutputMode::Login => {
            writeln!(stdout, "logged in account: {trimmed_label}")
                .map_err(AccountCommandError::Stdout)?;
        }
    }
    writeln!(stdout, "account_id: {}", account_id.as_str()).map_err(AccountCommandError::Stdout)?;
    if output_mode == AccountImportOutputMode::Login {
        writeln!(
            stdout,
            "next: codex-router quota refresh --router-root {}",
            router_root.display()
        )
        .map_err(AccountCommandError::Stdout)?;
    }

    Ok(())
}

fn login_with_codex_device_auth(
    stdout: &mut impl Write,
    router_root: PathBuf,
    label: String,
    codex_bin: PathBuf,
    allow_plaintext_file_secrets: bool,
) -> Result<(), AccountCommandError> {
    if !allow_plaintext_file_secrets {
        return Err(AccountCommandError::PlaintextFileSecretsNotAllowed);
    }

    let temporary_codex_home = temporary_codex_home_path();
    std::fs::create_dir_all(&temporary_codex_home).map_err(|source| {
        AccountCommandError::CreateTemporaryCodexHome {
            path: temporary_codex_home.clone(),
            source,
        }
    })?;
    let permissions = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(&temporary_codex_home, permissions).map_err(|source| {
        AccountCommandError::CreateTemporaryCodexHome {
            path: temporary_codex_home.clone(),
            source,
        }
    })?;
    let status = match Command::new(&codex_bin)
        .arg("login")
        .arg("--device-auth")
        .env("CODEX_HOME", &temporary_codex_home)
        .status()
    {
        Ok(status) => status,
        Err(source) => {
            remove_temporary_codex_home(&temporary_codex_home)?;
            return Err(AccountCommandError::DeviceAuthLaunch {
                path: codex_bin,
                source,
            });
        }
    };
    if !status.success() {
        remove_temporary_codex_home(&temporary_codex_home)?;
        return Err(AccountCommandError::DeviceAuthFailed {
            status: status.to_string(),
        });
    }

    let auth_json = temporary_codex_home.join("auth.json");
    let auth_text = match std::fs::read_to_string(&auth_json) {
        Ok(auth_text) => auth_text,
        Err(error) => {
            remove_temporary_codex_home(&temporary_codex_home)?;
            return Err(AccountCommandError::ReadAuthJson {
                message: error.to_string(),
            });
        }
    };
    remove_temporary_codex_home(&temporary_codex_home)?;
    import_codex_auth_text(
        stdout,
        router_root,
        label,
        &auth_text,
        AccountImportOutputMode::Login,
    )
}

fn temporary_codex_home_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "codex-router-device-auth-{}-{nanos}",
        std::process::id()
    ))
}

fn remove_temporary_codex_home(temporary_codex_home: &Path) -> Result<(), AccountCommandError> {
    std::fs::remove_dir_all(temporary_codex_home).map_err(|source| {
        AccountCommandError::RemoveTemporaryCodexHome {
            path: temporary_codex_home.to_path_buf(),
            source,
        }
    })
}

/// Parsed import request used by CLI and failure-injection tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountImportRequest {
    account_id: AccountId,
    label: String,
    access_token: String,
    refresh_token: Option<String>,
    chatgpt_account_id: Option<String>,
}

impl AccountImportRequest {
    /// Creates an account import request.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        label: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        Self {
            account_id,
            label: label.into(),
            access_token: access_token.into(),
            refresh_token: None,
            chatgpt_account_id: None,
        }
    }

    /// Sets a required refresh token.
    #[must_use]
    pub fn with_refresh_token(mut self, refresh_token: impl Into<String>) -> Self {
        self.refresh_token = Some(refresh_token.into());
        self
    }

    /// Sets an optional refresh token.
    #[must_use]
    pub fn with_optional_refresh_token(mut self, refresh_token: Option<String>) -> Self {
        self.refresh_token = refresh_token;
        self
    }

    /// Sets the ChatGPT account id used by ChatGPT backend requests.
    #[must_use]
    pub fn with_chatgpt_account_id(mut self, chatgpt_account_id: impl Into<String>) -> Self {
        let chatgpt_account_id = chatgpt_account_id.into();
        if !chatgpt_account_id.trim().is_empty() {
            self.chatgpt_account_id = Some(chatgpt_account_id);
        }
        self
    }
}

/// Imports an already-parsed Codex OAuth auth record into router-owned state.
pub fn import_codex_auth_from_request(
    state: &SqliteStateStore,
    secrets: &impl SecretStore,
    request: AccountImportRequest,
) -> Result<(), AccountCommandError> {
    let active_credential_generation = state.next_credential_generation(&request.account_id)?;
    let disabled_account = AccountRecord::new(
        request.account_id.clone(),
        request.label.clone(),
        AccountStatus::Disabled,
    );
    AccountStateRepository::upsert_account(state, &disabled_account)?;
    let bundle_key =
        account_credential_bundle_key(&request.account_id, active_credential_generation)?;
    let mut bundle =
        AccountCredentialBundle::imported_codex_auth(request.access_token, request.refresh_token);
    if let Some(chatgpt_account_id) = request.chatgpt_account_id {
        bundle = bundle.with_chatgpt_account_id(chatgpt_account_id);
    }
    secrets.write_secret(&bundle_key, &bundle.to_secret_string()?)?;
    state.activate_account_credential_generation_and_invalidate_quota(
        &request.account_id,
        active_credential_generation,
        AccountStatus::Enabled,
    )?;

    Ok(())
}

fn list_accounts(stdout: &mut impl Write, router_root: PathBuf) -> Result<(), AccountCommandError> {
    let state = SqliteStateStore::open(&router_root.join("state.sqlite"))?;
    let accounts = AccountStateRepository::list_accounts(&state)?;
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(["account", "status"]);
    for account in accounts {
        table.add_row([account.label(), account.status().as_str()]);
    }
    writeln!(stdout, "{table}").map_err(AccountCommandError::Stdout)?;

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
    chatgpt_account_id: Option<String>,
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
        let chatgpt_account_id = tokens
            .get("id_token")
            .and_then(serde_json::Value::as_str)
            .and_then(chatgpt_account_id_from_id_token);

        Ok(Self {
            access_token,
            refresh_token,
            chatgpt_account_id,
        })
    }
}

fn chatgpt_account_id_from_id_token(id_token: &str) -> Option<String> {
    let payload_segment = id_token.split('.').nth(1)?;
    let payload = URL_SAFE_NO_PAD.decode(payload_segment).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    value
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty())
        .map(str::to_owned)
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
struct AccountLoginOptions {
    router_root: Option<PathBuf>,
    label: Option<String>,
    auth_json: Option<PathBuf>,
    device_auth: bool,
    codex_bin: Option<PathBuf>,
    allow_plaintext_file_secrets: bool,
}

impl AccountLoginOptions {
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
                "--device-auth" => {
                    options.device_auth = true;
                }
                "--codex-bin" => {
                    options.codex_bin =
                        Some(PathBuf::from(parser.next_required_value("--codex-bin")?));
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
        router_root_or_default(self.router_root.clone())
    }

    fn label(&self) -> Result<String, CliError> {
        self.label
            .clone()
            .ok_or(CliError::MissingOption { option: "--label" })
    }

    fn method(&self) -> Result<AccountLoginMethod, CliError> {
        match (&self.auth_json, self.device_auth) {
            (Some(_), true) => Err(AccountCommandError::LoginMethodRequired.into()),
            (Some(auth_json), false) => Ok(AccountLoginMethod::AuthJson(auth_json.clone())),
            (None, false) | (None, true) => Ok(AccountLoginMethod::DeviceAuth {
                codex_bin: self
                    .codex_bin
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("codex")),
            }),
        }
    }
}

enum AccountLoginMethod {
    AuthJson(PathBuf),
    DeviceAuth { codex_bin: PathBuf },
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
        router_root_or_default(self.router_root.clone())
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
        router_root_or_default(self.router_root)
    }
}
