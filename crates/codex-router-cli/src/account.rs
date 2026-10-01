//! Account command glue for router-owned account state.

use std::io::BufRead;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use codex_router_auth::claude_oauth::AccountLoginFlow;
use codex_router_auth::claude_oauth::ClaudeOAuthLoginFlow;
use codex_router_auth::claude_oauth::LoginFlowError;
use codex_router_auth::claude_oauth::PendingClaudeOAuthLogin;
use codex_router_auth::credential_activation::CredentialActivation;
use codex_router_auth::credential_activation::CredentialActivationError;
use codex_router_auth::credential_activation::CredentialActivationRequest;
use codex_router_auth::openai_oauth::OpenAiOAuthDeviceLoginClient;
use codex_router_auth::openai_oauth::OpenAiOAuthDeviceLoginError;
use codex_router_auth::openai_oauth::OpenAiOAuthLoginTokens;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus;
use codex_router_secret_store::model::SecretStoreError;
#[cfg(test)]
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::account_routing_policy::WeeklyQuotaFloorBasisPoints;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::AsyncWeeklyQuotaFloorMutationStore;
use codex_router_state::sqlite::StateStoreError;
use comfy_table::Table;
use comfy_table::presets::UTF8_FULL;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::ArgumentParser;
use crate::CliError;
use crate::router_root_or_default;

/// Account CLI command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccountCommand {
    /// Prints account command help.
    Help(&'static str),
    /// Runs provider OAuth login and activates the resulting credentials.
    Login {
        /// Router-owned root.
        router_root: PathBuf,
        /// Display label.
        label: String,
        /// Typed provider flow selected by the shared login dispatcher.
        provider_login_flow: ProviderLoginFlow,
    },
    /// Lists router-owned accounts.
    List {
        /// Router-owned root.
        router_root: PathBuf,
    },
    /// Changes one account's lifecycle status while retaining its credentials.
    SetStatus {
        /// Router-owned root.
        router_root: PathBuf,
        /// Exact display label used to resolve one account.
        account_label: String,
        /// Lifecycle status to persist.
        status: AccountStatus,
    },
    /// Sets or disables one account's weekly quota floor.
    SetWeeklyFloor {
        /// Router-owned root.
        router_root: PathBuf,
        /// Exact display label used to resolve one account.
        account_label: String,
        /// Integer percentage from zero through fifteen.
        percent: u16,
    },
}

/// Provider-specific OAuth flow selected by `account login --provider`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderLoginFlow {
    /// Router-native OpenAI OAuth device-code flow.
    OpenAiDevice,
    /// Claude's hosted callback and pasted code#state flow.
    ClaudeOAuth,
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
                let provider = options.provider.unwrap_or(Provider::Openai);
                let provider_login_flow = match provider {
                    Provider::Openai => ProviderLoginFlow::OpenAiDevice,
                    Provider::Claude => ProviderLoginFlow::ClaudeOAuth,
                };
                Ok(Self::Login {
                    router_root: options.router_root()?,
                    label: options.label()?,
                    provider_login_flow,
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
            "enable" | "disable" => {
                if parser.next_if_help()? {
                    parser.reject_remaining()?;
                    return Ok(Self::Help(if command == "enable" {
                        ACCOUNT_ENABLE_HELP_TEXT
                    } else {
                        ACCOUNT_DISABLE_HELP_TEXT
                    }));
                }
                let options = AccountStatusOptions::parse(parser)?;
                Ok(Self::SetStatus {
                    router_root: options.router_root()?,
                    account_label: options.account_label()?,
                    status: if command == "enable" {
                        AccountStatus::Enabled
                    } else {
                        AccountStatus::Disabled
                    },
                })
            }
            "set-weekly-floor" => {
                if parser.next_if_help()? {
                    parser.reject_remaining()?;
                    return Ok(Self::Help(ACCOUNT_SET_WEEKLY_FLOOR_HELP_TEXT));
                }
                let options = AccountSetWeeklyFloorOptions::parse(parser)?;
                Ok(Self::SetWeeklyFloor {
                    router_root: options.router_root()?,
                    account_label: options.account_label()?,
                    percent: options.percent()?,
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
    /// Router root creation failed.
    #[error("failed to create router root {path}: {source}")]
    CreateRouterRoot {
        /// Router root path.
        path: PathBuf,
        /// IO source.
        #[source]
        source: std::io::Error,
    },
    /// An account id already belongs to another provider.
    #[error("account provider does not match OpenAI credential import")]
    AccountProviderMismatch,
    /// Display label was empty.
    #[error("account label must not be empty")]
    EmptyLabel,
    /// Another provider or account already owns this globally unique label.
    #[error("account label already exists: {label}")]
    DuplicateAccountLabel {
        /// Existing display label.
        label: String,
    },
    /// A setter option was supplied more than once.
    #[error("weekly floor option supplied more than once: {option}")]
    DuplicateWeeklyFloorOption {
        /// Duplicated option name.
        option: &'static str,
    },
    /// The configured percentage was not an integer in the supported range.
    #[error("weekly floor percent must be an integer from 0 through 15")]
    InvalidWeeklyFloorPercent,
    /// A lifecycle status option was supplied more than once.
    #[error("account status option supplied more than once: {option}")]
    DuplicateAccountStatusOption {
        /// Duplicated option name.
        option: &'static str,
    },
    /// Provider login option was supplied more than once.
    #[error("account login option supplied more than once: {option}")]
    DuplicateAccountLoginOption {
        /// Duplicated option name.
        option: &'static str,
    },
    /// OpenAI OAuth device-code login failed.
    #[error(transparent)]
    OpenAiOAuth(#[from] OpenAiOAuthDeviceLoginError),
    /// Claude OAuth flow could not safely produce account credentials.
    #[error(transparent)]
    ClaudeOAuth(#[from] LoginFlowError),
    /// No configured account has the supplied exact label.
    #[error("weekly floor account label did not match a configured account")]
    WeeklyFloorAccountNotFound,
    /// More than one configured account has the supplied exact label.
    #[error("weekly floor account label matched more than one configured account")]
    WeeklyFloorAccountAmbiguous,
    /// SQLite writer contention exceeded the state layer's bounded retry window.
    #[error("failed to update weekly floor: database is busy; retry the command")]
    WeeklyFloorDatabaseBusy,
    /// The router must migrate the database before the setter can write policy.
    #[error("weekly quota floor requires a compatible upgraded router database")]
    WeeklyFloorSchemaUpgradeRequired,
    /// A weekly-floor state operation failed without exposing storage details.
    #[error("weekly floor state operation failed")]
    WeeklyFloorStateOperationFailed,
    /// Account status could not acquire the SQLite writer lock in time.
    #[error("failed to update account status: database is busy; retry the command")]
    AccountStatusDatabaseBusy,
    /// The router must migrate the database before the status command can write state.
    #[error("account status requires a compatible upgraded router database")]
    AccountStatusSchemaUpgradeRequired,
    /// No configured account has the supplied exact label.
    #[error("account status target did not match a configured account")]
    AccountStatusAccountNotFound,
    /// More than one configured account has the supplied exact label.
    #[error("account status target label matched more than one configured account")]
    AccountStatusAccountAmbiguous,
    /// An account status operation failed without exposing storage details.
    #[error("account status update failed")]
    AccountStatusStateOperationFailed,
    /// Secret-store operation failed.
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    /// Cross-process credential authority could not be established.
    #[error("account credential lock unavailable")]
    CredentialLockUnavailable,
    /// Process-scoped encrypted credential storage could not be initialized.
    #[error("encrypted credential store could not be initialized")]
    CredentialStoreInitialization,
    /// Auth-owned credential activation failed.
    #[error(transparent)]
    CredentialActivation(CredentialActivationError),
    /// State-store operation failed.
    #[error(transparent)]
    StateStore(#[from] StateStoreError),
    /// Tokio runtime failed to initialize.
    #[error(transparent)]
    Runtime(#[from] std::io::Error),
    /// Stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),
}

/// Runs an account command.
pub fn run_account_command(
    stdout: &mut impl Write,
    command: AccountCommand,
) -> Result<(), AccountCommandError> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    run_account_command_with_input(stdout, &mut reader, command)
}

pub(crate) fn run_account_command_with_input(
    stdout: &mut impl Write,
    reader: &mut impl BufRead,
    command: AccountCommand,
) -> Result<(), AccountCommandError> {
    let openai_client = OpenAiOAuthDeviceLoginClient::new();
    run_account_command_with_input_and_openai_client(stdout, reader, command, &openai_client)
}

pub(crate) fn run_account_command_with_input_and_openai_client(
    stdout: &mut impl Write,
    reader: &mut impl BufRead,
    command: AccountCommand,
    openai_client: &OpenAiOAuthDeviceLoginClient,
) -> Result<(), AccountCommandError> {
    run_account_command_with_input_and_provider_flows_and_store_overrides(
        stdout,
        reader,
        command,
        openai_client,
        None,
        None,
        None,
    )
}

#[cfg(test)]
pub(crate) fn run_account_command_with_input_and_openai_client_and_secret_store(
    stdout: &mut impl Write,
    reader: &mut impl BufRead,
    command: AccountCommand,
    openai_client: &OpenAiOAuthDeviceLoginClient,
    secret_store: EncryptedCredentialStore,
) -> Result<(), AccountCommandError> {
    run_account_command_with_input_and_provider_flows_and_store_overrides(
        stdout,
        reader,
        command,
        openai_client,
        Some(secret_store),
        None,
        None,
    )
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn run_account_command_with_input_and_claude_flow_and_secret_store(
    stdout: &mut impl Write,
    reader: &mut impl BufRead,
    command: AccountCommand,
    claude_flow: &impl AccountLoginFlow<PendingLogin = PendingClaudeOAuthLogin>,
    secret_store: EncryptedCredentialStore,
) -> Result<(), AccountCommandError> {
    assert!(
        matches!(
            &command,
            AccountCommand::Login {
                provider_login_flow: ProviderLoginFlow::ClaudeOAuth,
                ..
            }
        ),
        "test Claude login runner requires a Claude login command"
    );
    let openai_client = OpenAiOAuthDeviceLoginClient::new();
    let claude_flow_override: &dyn AccountLoginFlow<PendingLogin = PendingClaudeOAuthLogin> =
        claude_flow;
    run_account_command_with_input_and_provider_flows_and_store_overrides(
        stdout,
        reader,
        command,
        &openai_client,
        None,
        Some(claude_flow_override),
        Some(secret_store),
    )
}

fn run_account_command_with_input_and_provider_flows_and_store_overrides(
    stdout: &mut impl Write,
    reader: &mut impl BufRead,
    command: AccountCommand,
    openai_client: &OpenAiOAuthDeviceLoginClient,
    openai_secret_store_override: Option<EncryptedCredentialStore>,
    claude_flow_override: Option<&dyn AccountLoginFlow<PendingLogin = PendingClaudeOAuthLogin>>,
    claude_secret_store_override: Option<EncryptedCredentialStore>,
) -> Result<(), AccountCommandError> {
    match command {
        AccountCommand::Help(text) => stdout
            .write_all(text.as_bytes())
            .map_err(AccountCommandError::Stdout),
        AccountCommand::Login {
            router_root,
            label,
            provider_login_flow,
        } => match provider_login_flow {
            ProviderLoginFlow::OpenAiDevice => login_with_openai_device_auth(
                stdout,
                router_root,
                label,
                openai_client,
                openai_secret_store_override,
            ),
            ProviderLoginFlow::ClaudeOAuth => match claude_flow_override {
                Some(claude_flow) => login_with_claude_oauth(
                    stdout,
                    reader,
                    router_root,
                    label,
                    claude_flow,
                    claude_secret_store_override,
                ),
                None => login_with_claude_oauth(
                    stdout,
                    reader,
                    router_root,
                    label,
                    &ClaudeOAuthLoginFlow::new(),
                    claude_secret_store_override,
                ),
            },
        },
        AccountCommand::List { router_root } => list_accounts(stdout, router_root),
        AccountCommand::SetStatus {
            router_root,
            account_label,
            status,
        } => set_account_status(stdout, router_root, account_label, status),
        AccountCommand::SetWeeklyFloor {
            router_root,
            account_label,
            percent,
        } => set_weekly_floor(stdout, router_root, account_label, percent),
    }
}

const ACCOUNT_HELP_TEXT: &str = "\
codex-router account

commands:
  disable --account <name>  Stop routing to an account while retaining its credentials
  enable --account <name>   Resume routing to an account
  login --provider <openai|claude> --label <name>  Add a provider OAuth account
  list                  Show configured router accounts
  set-weekly-floor      Set or disable one account's weekly quota floor
";

const ACCOUNT_LOGIN_HELP_TEXT: &str = "\
codex-router account login --provider <openai|claude> --label <name>

Adds an OAuth account to router-owned encrypted storage.

options:
  --label <name>         Friendly account name shown in quota and account list
  --provider <name>      OAuth account provider [default: openai]
  OpenAI login displays a device URL and code, then waits for approval.
  Claude login opens the hosted authorization URL and asks you to paste code#state.
";

const ACCOUNT_LIST_HELP_TEXT: &str = "\
codex-router account list

Shows configured router accounts.
";

const ACCOUNT_ENABLE_HELP_TEXT: &str = "\
codex-router account enable --account <label>

Resumes routing to one account without changing its credentials, quota, or history.
";

const ACCOUNT_DISABLE_HELP_TEXT: &str = "\
codex-router account disable --account <label>

Changes one account's routing status without removing its credentials, quota, or history.
";

const ACCOUNT_SET_WEEKLY_FLOOR_HELP_TEXT: &str = "\
codex-router account set-weekly-floor --account <label> --percent <0-15>

Sets an integer weekly quota floor for exactly one account label. Zero disables it.
";

fn login_with_openai_device_auth(
    stdout: &mut impl Write,
    router_root: PathBuf,
    label: String,
    client: &OpenAiOAuthDeviceLoginClient,
    secret_store_override: Option<EncryptedCredentialStore>,
) -> Result<(), AccountCommandError> {
    let label = normalize_label(&label)?;
    ensure_account_label_available_at_router_root(&router_root, &label, Provider::Openai)?;
    let account_id = account_id_from_label(&label)?;
    let runtime = account_command_runtime()?;
    let tokens = collect_openai_oauth_tokens(stdout, client, &runtime)?;

    create_router_root(&router_root)?;
    let state = runtime.block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    ))?;
    ensure_account_label_available(&state, &label, Provider::Openai, &runtime)?;
    let secret_store = match secret_store_override {
        Some(secret_store) => secret_store,
        None => runtime
            .block_on(crate::secret_store_factory::open_cli_secret_store_async(
                router_root.join("secrets"),
            ))
            .map_err(|_| AccountCommandError::CredentialStoreInitialization)?,
    };

    let mut bundle = AccountCredentialBundle::imported_codex_auth(
        tokens.access_token().expose_secret().to_owned(),
        Some(tokens.refresh_token().expose_secret().to_owned()),
    );
    if let Some(chatgpt_account_id) = tokens.chatgpt_account_id() {
        bundle = bundle.with_chatgpt_account_id(chatgpt_account_id.as_str());
    }
    let activation_request = CredentialActivationRequest::new(
        Provider::Openai,
        account_id.clone(),
        label.clone(),
        bundle.into(),
    );
    runtime
        .block_on(CredentialActivation::activate_login(
            &state,
            &secret_store,
            activation_request,
        ))
        .map_err(map_credential_activation_error)?;

    writeln!(stdout, "logged in account: {label}").map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "account_id: {}", account_id.as_str()).map_err(AccountCommandError::Stdout)?;
    writeln!(
        stdout,
        "next: codex-router quota refresh --router-root {}",
        router_root.display()
    )
    .map_err(AccountCommandError::Stdout)?;
    Ok(())
}

fn collect_openai_oauth_tokens(
    stdout: &mut impl Write,
    client: &OpenAiOAuthDeviceLoginClient,
    runtime: &tokio::runtime::Runtime,
) -> Result<OpenAiOAuthLoginTokens, AccountCommandError> {
    let cancellation = CancellationToken::new();
    let device_code = runtime.block_on(client.request_user_code(&cancellation))?;
    writeln!(
        stdout,
        "Open this URL and enter the code to sign in to OpenAI:"
    )
    .map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "{}", device_code.verification_url()).map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "Code: {}", device_code.user_code()).map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "Waiting for approval...").map_err(AccountCommandError::Stdout)?;
    stdout.flush().map_err(AccountCommandError::Stdout)?;

    runtime
        .block_on(client.complete_device_code_login(&device_code, &cancellation))
        .map_err(Into::into)
}

fn collect_claude_oauth_bundle(
    stdout: &mut impl Write,
    reader: &mut impl BufRead,
    flow: &(impl AccountLoginFlow<PendingLogin = PendingClaudeOAuthLogin> + ?Sized),
) -> Result<codex_router_secret_store::credential_bundle::CredentialBundle, AccountCommandError> {
    let pending = flow.begin_login()?;
    let authorization_url = flow.authorization_url(&pending)?;
    writeln!(stdout, "Open this URL and finish the Claude authorization:")
        .map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "{authorization_url}").map_err(AccountCommandError::Stdout)?;
    write!(stdout, "Paste the returned code#state: ").map_err(AccountCommandError::Stdout)?;
    stdout.flush().map_err(AccountCommandError::Stdout)?;
    let mut pasted_callback = String::new();
    reader.read_line(&mut pasted_callback)?;
    flow.finish_login(pending, &pasted_callback)
        .map_err(Into::into)
}

fn map_credential_activation_error(error: CredentialActivationError) -> AccountCommandError {
    match error {
        CredentialActivationError::AccountProviderMismatch => {
            AccountCommandError::AccountProviderMismatch
        }
        other => AccountCommandError::CredentialActivation(other),
    }
}

fn login_with_claude_oauth(
    stdout: &mut impl Write,
    reader: &mut impl BufRead,
    router_root: PathBuf,
    label: String,
    flow: &(impl AccountLoginFlow<PendingLogin = PendingClaudeOAuthLogin> + ?Sized),
    secret_store_override: Option<EncryptedCredentialStore>,
) -> Result<(), AccountCommandError> {
    let label = normalize_label(&label)?;
    ensure_account_label_available_at_router_root(&router_root, &label, Provider::Claude)?;
    let account_id = account_id_from_label(&label)?;
    let bundle = collect_claude_oauth_bundle(stdout, reader, flow)?;

    create_router_root(&router_root)?;
    let runtime = account_command_runtime()?;
    let state = runtime.block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    ))?;
    ensure_account_label_available(&state, &label, Provider::Claude, &runtime)?;
    let secret_store = match secret_store_override {
        Some(secret_store) => secret_store,
        None => runtime
            .block_on(crate::secret_store_factory::open_cli_secret_store_async(
                router_root.join("secrets"),
            ))
            .map_err(|_| AccountCommandError::CredentialStoreInitialization)?,
    };
    let activation_request = CredentialActivationRequest::new(
        Provider::Claude,
        account_id.clone(),
        label.clone(),
        bundle,
    );
    runtime
        .block_on(CredentialActivation::activate_login(
            &state,
            &secret_store,
            activation_request,
        ))
        .map_err(map_credential_activation_error)?;

    writeln!(stdout, "logged in Claude account: {label}").map_err(AccountCommandError::Stdout)?;
    writeln!(stdout, "account_id: {}", account_id.as_str()).map_err(AccountCommandError::Stdout)?;
    writeln!(
        stdout,
        "next: codex-router quota refresh --router-root {}",
        router_root.display()
    )
    .map_err(AccountCommandError::Stdout)?;
    Ok(())
}

/// OpenAI credential activation request used by account storage tests.
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

/// Imports an already-parsed Codex OAuth auth record into router-owned SQLx state.
pub async fn import_codex_auth_from_request_async<S>(
    state: &AsyncSqliteStateStore,
    secrets: &S,
    request: AccountImportRequest,
) -> Result<(), AccountCommandError>
where
    S: SecretStore + Clone + Send + Sync + 'static,
{
    let mut bundle =
        AccountCredentialBundle::imported_codex_auth(request.access_token, request.refresh_token);
    if let Some(chatgpt_account_id) = request.chatgpt_account_id {
        bundle = bundle.with_chatgpt_account_id(chatgpt_account_id);
    }
    let activation_request = CredentialActivationRequest::new(
        Provider::Openai,
        request.account_id,
        request.label,
        bundle.into(),
    );
    CredentialActivation::activate_login(state, secrets, activation_request)
        .await
        .map_err(|error| match error {
            CredentialActivationError::AccountProviderMismatch => {
                AccountCommandError::AccountProviderMismatch
            }
            other => AccountCommandError::CredentialActivation(other),
        })?;

    Ok(())
}

fn list_accounts(stdout: &mut impl Write, router_root: PathBuf) -> Result<(), AccountCommandError> {
    let runtime = account_command_runtime()?;
    let credential_store = runtime
        .block_on(crate::secret_store_factory::open_cli_secret_store_async(
            router_root.join("secrets"),
        ))
        .map_err(|_| AccountCommandError::CredentialStoreInitialization)?;
    let state = runtime.block_on(AsyncSqliteStateStore::open_read_only(
        &router_root.join("state.sqlite"),
    ))?;
    let accounts = runtime.block_on(state.list_accounts())?;
    let policies = runtime.block_on(state.list_account_routing_policies())?;
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(["provider", "account", "status", "weekly floor", "OAuth"]);
    for account in accounts {
        let weekly_floor = policies
            .iter()
            .find(|policy| policy.account_id() == account.account_id())
            .map_or_else(
                || "disabled".to_owned(),
                |policy| format!("{}%", policy.weekly_quota_floor_basis_points().percent()),
            );
        let maintenance =
            runtime.block_on(state.load_credential_maintenance(account.account_id()))?;
        let oauth_status = match credential_store.status() {
            EncryptedCredentialStoreStatus::KeyUnavailable => "keychain_locked".to_owned(),
            EncryptedCredentialStoreStatus::MigrationIncomplete { accounts, failure } => {
                if accounts.is_empty() {
                    format!("migration incomplete ({failure})")
                } else {
                    format!("migration incomplete ({failure}): {}", accounts.join(", "))
                }
            }
            EncryptedCredentialStoreStatus::Ready => match maintenance
                .as_ref()
                .filter(|record| {
                    Some(record.credential_generation) == account.active_credential_generation()
                })
                .map(|record| record.state.as_str())
            {
                Some("healthy") => "healthy".to_owned(),
                Some("retrying") => "retrying".to_owned(),
                Some("reauth_required" | "unrefreshable") => "re-login required".to_owned(),
                _ => "unknown".to_owned(),
            },
        };
        table.add_row([
            account.provider().as_str(),
            account.label(),
            account.status().as_str(),
            &weekly_floor,
            &oauth_status,
        ]);
    }
    writeln!(stdout, "{table}").map_err(AccountCommandError::Stdout)?;

    Ok(())
}

fn ensure_account_label_available_at_router_root(
    router_root: &Path,
    label: &str,
    provider: Provider,
) -> Result<(), AccountCommandError> {
    let state_database_path = router_root.join("state.sqlite");
    if !state_database_path.exists() {
        return Ok(());
    }

    let runtime = account_command_runtime()?;
    let state = runtime.block_on(AsyncSqliteStateStore::open(&state_database_path))?;
    ensure_account_label_available(&state, label, provider, &runtime)?;
    runtime.block_on(state.close())?;
    Ok(())
}

fn ensure_account_label_available(
    state: &AsyncSqliteStateStore,
    label: &str,
    provider: Provider,
    runtime: &tokio::runtime::Runtime,
) -> Result<(), AccountCommandError> {
    let accounts = runtime.block_on(state.list_accounts())?;
    let account_id = account_id_from_label(label)?;
    for account in accounts {
        if account.label() != label && account.account_id() != &account_id {
            continue;
        }
        let is_same_account = account.label() == label
            && account.account_id() == &account_id
            && account.provider() == provider;
        if !is_same_account {
            return Err(AccountCommandError::DuplicateAccountLabel {
                label: label.to_owned(),
            });
        }
    }
    Ok(())
}

fn set_weekly_floor(
    stdout: &mut impl Write,
    router_root: PathBuf,
    account_label: String,
    percent: u16,
) -> Result<(), AccountCommandError> {
    let runtime = account_command_runtime()?;
    let database_path = router_root.join("state.sqlite");
    let floor = if percent == 0 {
        None
    } else {
        let basis_points = percent
            .checked_mul(100)
            .ok_or(AccountCommandError::InvalidWeeklyFloorPercent)?;
        Some(
            WeeklyQuotaFloorBasisPoints::new(basis_points)
                .map_err(|_| AccountCommandError::InvalidWeeklyFloorPercent)?,
        )
    };
    let mutation = runtime
        .block_on(AsyncWeeklyQuotaFloorMutationStore::open(&database_path))
        .map_err(redacted_weekly_floor_state_error)?;
    let mutation_result =
        runtime.block_on(mutation.set_weekly_quota_floor_by_label(&account_label, floor));
    runtime.block_on(mutation.close());
    mutation_result.map_err(redacted_weekly_floor_state_error)?;

    if percent == 0 {
        writeln!(
            stdout,
            "updated weekly floor: {account_label} = disabled (0%)"
        )
        .map_err(AccountCommandError::Stdout)
    } else {
        writeln!(stdout, "updated weekly floor: {account_label} = {percent}%")
            .map_err(AccountCommandError::Stdout)
    }
}

fn set_account_status(
    stdout: &mut impl Write,
    router_root: PathBuf,
    account_label: String,
    status: AccountStatus,
) -> Result<(), AccountCommandError> {
    let runtime = account_command_runtime()?;
    let mutation = runtime
        .block_on(AsyncWeeklyQuotaFloorMutationStore::open(
            &router_root.join("state.sqlite"),
        ))
        .map_err(redacted_account_status_state_error)?;
    let mutation_result =
        runtime.block_on(mutation.set_account_status_by_label(&account_label, status));
    runtime.block_on(mutation.close());
    mutation_result.map_err(redacted_account_status_state_error)?;
    writeln!(
        stdout,
        "updated account status: {account_label} = {}",
        status.as_str()
    )
    .map_err(AccountCommandError::Stdout)
}

fn redacted_weekly_floor_state_error(error: StateStoreError) -> AccountCommandError {
    match error {
        StateStoreError::WeeklyQuotaFloorDatabaseBusy => {
            AccountCommandError::WeeklyFloorDatabaseBusy
        }
        StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired => {
            AccountCommandError::WeeklyFloorSchemaUpgradeRequired
        }
        StateStoreError::WeeklyQuotaFloorAccountNotFound => {
            AccountCommandError::WeeklyFloorAccountNotFound
        }
        StateStoreError::WeeklyQuotaFloorAccountLabelAmbiguous => {
            AccountCommandError::WeeklyFloorAccountAmbiguous
        }
        _ => AccountCommandError::WeeklyFloorStateOperationFailed,
    }
}

fn redacted_account_status_state_error(error: StateStoreError) -> AccountCommandError {
    match error {
        StateStoreError::AccountStatusDatabaseBusy => {
            AccountCommandError::AccountStatusDatabaseBusy
        }
        StateStoreError::AccountStatusSchemaUpgradeRequired
        | StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired => {
            AccountCommandError::AccountStatusSchemaUpgradeRequired
        }
        StateStoreError::AccountStatusAccountNotFound => {
            AccountCommandError::AccountStatusAccountNotFound
        }
        StateStoreError::AccountStatusAccountLabelAmbiguous => {
            AccountCommandError::AccountStatusAccountAmbiguous
        }
        _ => AccountCommandError::AccountStatusStateOperationFailed,
    }
}

#[cfg(test)]
mod account_status_error_tests {
    use super::*;

    #[test]
    fn schema_upgrade_error_is_actionable_and_redacted() {
        let rendered = redacted_account_status_state_error(
            StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired,
        )
        .to_string();
        assert_eq!(
            rendered,
            "account status requires a compatible upgraded router database"
        );
        for canary in ["sensitive-label", "acct_internal", "/private/state.sqlite"] {
            assert!(!rendered.contains(canary));
        }
    }
}

#[cfg(test)]
mod account_provider_cli_tests {
    use super::*;
    use codex_router_core::provider::Provider;

    #[test]
    fn login_refuses_a_label_owned_by_another_provider_before_device_request() {
        let temporary_root = tempfile::tempdir().expect("temporary router root should exist");
        let runtime = account_command_runtime().expect("test runtime should initialize");
        let state = runtime
            .block_on(AsyncSqliteStateStore::open(
                &temporary_root.path().join("state.sqlite"),
            ))
            .expect("test state should open");
        let existing_account = AccountRecord::new(
            Provider::Claude,
            account_id_from_label("shared-label").expect("test account id should parse"),
            "shared-label",
            AccountStatus::Enabled,
        );
        runtime
            .block_on(state.upsert_account(&existing_account))
            .expect("Claude account should persist");
        runtime
            .block_on(state.close())
            .expect("test state should close");

        let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
            "http://127.0.0.1:1",
            std::time::Duration::from_secs(1),
        );
        let error = login_with_openai_device_auth(
            &mut Vec::new(),
            temporary_root.path().to_path_buf(),
            " shared-label ".to_owned(),
            &client,
            None,
        )
        .expect_err("duplicate labels must be refused before device auth");
        assert!(matches!(
            error,
            AccountCommandError::DuplicateAccountLabel { label } if label == "shared-label"
        ));
    }

    #[test]
    fn account_list_displays_each_provider() {
        let temporary_root = tempfile::tempdir().expect("temporary router root should exist");
        let runtime = account_command_runtime().expect("test runtime should initialize");
        let state = runtime
            .block_on(AsyncSqliteStateStore::open(
                &temporary_root.path().join("state.sqlite"),
            ))
            .expect("test state should open");
        for (label, provider) in [
            ("openai-label", Provider::Openai),
            ("claude-label", Provider::Claude),
        ] {
            let account = AccountRecord::new(
                provider,
                account_id_from_label(label).expect("test account id should parse"),
                label,
                AccountStatus::Enabled,
            );
            runtime
                .block_on(state.upsert_account(&account))
                .expect("test account should persist");
        }
        runtime
            .block_on(state.close())
            .expect("test state should close");

        let mut output = Vec::new();
        list_accounts(&mut output, temporary_root.path().to_path_buf())
            .expect("account list should render");
        let output = String::from_utf8(output).expect("account list output should be UTF-8");
        assert!(output.contains("provider"));
        assert!(output.contains("openai"));
        assert!(output.contains("claude"));
    }
}

fn account_command_runtime() -> Result<tokio::runtime::Runtime, AccountCommandError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(AccountCommandError::Runtime)
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct AccountLoginOptions {
    router_root: Option<PathBuf>,
    label: Option<String>,
    provider: Option<Provider>,
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
                "--provider" if options.provider.is_none() => {
                    let value = parser.next_required_value("--provider")?;
                    options.provider =
                        Some(
                            Provider::parse(&value).ok_or_else(|| CliError::UnknownOption {
                                option: format!("--provider {value}"),
                            })?,
                        );
                }
                "--provider" => {
                    return Err(AccountCommandError::DuplicateAccountLoginOption {
                        option: "--provider",
                    }
                    .into());
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
}

#[cfg(test)]
mod claude_login_glue_tests {
    use super::*;
    use codex_router_core::redaction::SecretString;
    use codex_router_secret_store::credential_bundle::CredentialBundle;
    use std::io::Cursor;
    use std::sync::Mutex;

    struct RecordingClaudeLoginFlow {
        flow: ClaudeOAuthLoginFlow,
        pasted_callbacks: Mutex<Vec<String>>,
    }

    impl RecordingClaudeLoginFlow {
        fn new() -> Self {
            Self {
                flow: ClaudeOAuthLoginFlow::new(),
                pasted_callbacks: Mutex::new(Vec::new()),
            }
        }
    }

    impl AccountLoginFlow for RecordingClaudeLoginFlow {
        type PendingLogin = PendingClaudeOAuthLogin;

        fn begin_login(&self) -> Result<Self::PendingLogin, LoginFlowError> {
            self.flow.begin_login()
        }

        fn authorization_url(
            &self,
            pending: &Self::PendingLogin,
        ) -> Result<String, LoginFlowError> {
            self.flow.authorization_url(pending)
        }

        fn finish_login(
            &self,
            _pending: Self::PendingLogin,
            pasted_callback: &str,
        ) -> Result<CredentialBundle, LoginFlowError> {
            self.pasted_callbacks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(pasted_callback.to_owned());
            if pasted_callback.trim().is_empty() {
                return Err(LoginFlowError::InvalidCallback);
            }
            CredentialBundle::new_claude(
                SecretString::new("glue-access-canary"),
                SecretString::new("glue-refresh-canary"),
                2_000,
            )
            .map_err(|_| LoginFlowError::InvalidTokenResponse)
        }
    }

    #[test]
    fn claude_login_glue_prints_prompt_and_passes_the_pasted_callback() {
        let flow = RecordingClaudeLoginFlow::new();
        let mut stdout = Vec::new();
        let mut reader = Cursor::new(b"code-value#state-value\n".to_vec());

        let bundle = collect_claude_oauth_bundle(&mut stdout, &mut reader, &flow)
            .unwrap_or_else(|error| panic!("test login should finish: {error}"));

        let prompt = String::from_utf8(stdout)
            .unwrap_or_else(|error| panic!("test prompt should be UTF-8: {error}"));
        assert!(prompt.contains("Open this URL and finish the Claude authorization:"));
        assert!(prompt.contains("Paste the returned code#state: "));
        assert_eq!(
            *flow
                .pasted_callbacks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["code-value#state-value\n"]
        );
        assert_eq!(bundle.access_token().expose_secret(), "glue-access-canary");
    }

    #[test]
    fn claude_login_glue_reports_eof_as_an_invalid_callback() {
        let flow = RecordingClaudeLoginFlow::new();
        let mut stdout = Vec::new();
        let mut reader = Cursor::new(Vec::<u8>::new());

        let error = collect_claude_oauth_bundle(&mut stdout, &mut reader, &flow)
            .expect_err("EOF must not activate an empty callback");

        assert!(matches!(
            error,
            AccountCommandError::ClaudeOAuth(LoginFlowError::InvalidCallback)
        ));
    }

    #[test]
    fn claude_login_glue_maps_provider_mismatch_from_activation() {
        assert!(matches!(
            map_credential_activation_error(CredentialActivationError::AccountProviderMismatch),
            AccountCommandError::AccountProviderMismatch
        ));
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct AccountRootOptions {
    router_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct AccountSetWeeklyFloorOptions {
    router_root: Option<PathBuf>,
    account_label: Option<String>,
    percent: Option<u16>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct AccountStatusOptions {
    router_root: Option<PathBuf>,
    account_label: Option<String>,
}

impl AccountStatusOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();
        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" if options.router_root.is_none() => {
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--account" if options.account_label.is_none() => {
                    options.account_label = Some(parser.next_required_value("--account")?);
                }
                "--router-root" => {
                    return Err(AccountCommandError::DuplicateAccountStatusOption {
                        option: "--router-root",
                    }
                    .into());
                }
                "--account" => {
                    return Err(AccountCommandError::DuplicateAccountStatusOption {
                        option: "--account",
                    }
                    .into());
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

    fn account_label(&self) -> Result<String, CliError> {
        self.account_label.clone().ok_or(CliError::MissingOption {
            option: "--account",
        })
    }
}

impl AccountSetWeeklyFloorOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();
        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    if options.router_root.is_some() {
                        return Err(AccountCommandError::DuplicateWeeklyFloorOption {
                            option: "--router-root",
                        }
                        .into());
                    }
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--account" => {
                    if options.account_label.is_some() {
                        return Err(AccountCommandError::DuplicateWeeklyFloorOption {
                            option: "--account",
                        }
                        .into());
                    }
                    options.account_label = Some(parser.next_required_value("--account")?);
                }
                "--percent" => {
                    if options.percent.is_some() {
                        return Err(AccountCommandError::DuplicateWeeklyFloorOption {
                            option: "--percent",
                        }
                        .into());
                    }
                    let raw_percent = parser.next_required_value("--percent")?;
                    let percent = raw_percent
                        .parse::<u16>()
                        .ok()
                        .filter(|percent| *percent <= 15)
                        .ok_or(AccountCommandError::InvalidWeeklyFloorPercent)?;
                    options.percent = Some(percent);
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

    fn account_label(&self) -> Result<String, CliError> {
        self.account_label.clone().ok_or(CliError::MissingOption {
            option: "--account",
        })
    }

    fn percent(&self) -> Result<u16, CliError> {
        self.percent.ok_or(CliError::MissingOption {
            option: "--percent",
        })
    }
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
