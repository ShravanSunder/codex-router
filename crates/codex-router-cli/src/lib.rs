//! Command-line entry points for codex-router.
#![cfg_attr(test, allow(clippy::panic_in_result_fn))]

use std::ffi::OsString;
use std::io::IsTerminal;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use codex_router_proxy::server::LoopbackBindAddress;
use codex_router_proxy::server::LoopbackRouterRuntime;
use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
use codex_router_proxy::upstream::UpstreamEndpoint;
use codex_router_secret_store::file_backend::FileSecretStore;

pub mod account;
mod credential_runtime;
pub mod doctor;
mod host_command;
mod live;
mod presentation;
pub mod profile;
pub mod quota;
mod quota_reset;
mod secret_store_factory;
mod telemetry;
pub mod token;

use account::AccountCommand;
use host_command::HostCommand;
use live::LiveCommand;
use profile::CodexRouterProfile;
use profile::CodexRouterProfileWriter;
use quota::QuotaCommand;
use quota::QuotaCommandError;
use token::LocalRouterTokenService;
use token::Shell;
use token::TokenCommandError;
use token::export_token_assignment;

mod profile_preview;
mod token_reload_watcher;
mod websocket_reporting;
use profile_preview::write_profile_preview;
use token_reload_watcher::LocalTokenReloadWatcher;
#[cfg(test)]
use websocket_reporting::websocket_registry_report_value;
use websocket_reporting::{
    validate_websocket_registry_report_file, write_websocket_registry_report_file,
};

mod cli_command_errors;
pub use cli_command_errors::CliError;
mod cli_argument_parsing;
pub(crate) use cli_argument_parsing::ArgumentParser;
use cli_argument_parsing::{CliCommand, ProfileCommand, TokenCommand};

const DEFAULT_PROFILE_PORT: u16 = 8787;
const DEFAULT_MAX_SNAPSHOT_AGE_SECONDS: u64 = 300;
const DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS: u64 = 240;
const LOCAL_TOKEN_ENV_VAR: &str = "CODEX_ROUTER_TOKEN";
const DEFAULT_ROUTER_ROOT_DIR: &str = ".codex-router";
#[cfg(all(debug_assertions, not(test)))]
const DEBUG_ROUTER_ROOT_ENV: &str = "CODEX_ROUTER_DEBUG_ROUTER_ROOT";
const DEBUG_APP_SERVER_SOCKET_ENV: &str = "CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET";
const USE_HOME_DEFAULT_ENV: &str = "CODEX_ROUTER_USE_HOME_DEFAULT";
const DEFAULT_DEBUG_ROUTER_ROOT_DIR: &str = ".codex-router-debug";

/// Runs the process CLI.
pub fn run() -> i32 {
    let _telemetry_guard = telemetry::init_from_env(telemetry::TelemetryMode::EnvironmentOnly);
    let run_span = telemetry::run_span();
    let _run_span_guard = run_span.enter();
    let context = CliContext::from_process();
    let args = std::env::args_os();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    if let Err(error) = run_with_io(args, &context, &mut stdout, &mut stderr) {
        let _ = writeln!(stderr, "{error}");
        return 2;
    }
    0
}

/// Runs the process CLI with native async command support.
pub async fn run_async() -> i32 {
    let args = std::env::args_os().collect::<Vec<_>>();
    let context = CliContext::from_process();
    let parsed_command = CliCommand::parse(args.clone());
    let uses_async_dispatch = matches!(&parsed_command, Ok(CliCommand::Quota(_)))
        || matches!(&parsed_command, Ok(CliCommand::Host(_)));
    if !uses_async_dispatch {
        return std::thread::spawn(move || run_sync_process_args(args))
            .join()
            .unwrap_or(2);
    }
    let telemetry_mode = match &parsed_command {
        Ok(CliCommand::Host(command)) if command.runs_foreground() => {
            telemetry::TelemetryMode::ForegroundHost
        }
        _ => telemetry::TelemetryMode::EnvironmentOnly,
    };
    let telemetry_guard = telemetry::init_from_env(telemetry_mode);
    let telemetry_shutdown = telemetry_guard.shutdown_handle();
    let run_span = telemetry::run_span();
    let _run_span_guard = run_span.enter();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    if let Err(error) = run_with_io_async_with_telemetry(
        args,
        &context,
        &mut stdout,
        &mut stderr,
        Some(telemetry_shutdown),
    )
    .await
    {
        let _ = writeln!(stderr, "{error}");
        return 2;
    }
    0
}

/// Runs the compiled quota-reset PTY harness entry without environment telemetry.
///
/// The installed `codex-router` binary does not reference this feature-gated entry point.
#[cfg(feature = "quota-reset-test-harness")]
#[doc(hidden)]
pub async fn run_quota_reset_test_harness() -> i32 {
    let args = std::env::args_os();
    let context = CliContext::new(Vec::new());
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    if let Err(error) =
        quota_reset::run_quota_reset_test_harness_with_io(args, &context, &mut stdout, &mut stderr)
            .await
    {
        let _ = writeln!(stderr, "{error}");
        return 2;
    }
    0
}

/// Runs the compiled sessions-picker PTY harness without loading Codex session state.
///
/// The installed `codex-router` binary does not reference this feature-gated entry point.
#[cfg(feature = "quota-reset-test-harness")]
#[doc(hidden)]
pub fn run_sessions_picker_test_harness() -> i32 {
    let result = agent_sessions::run_sessions_picker_test_harness();
    if let Err(error) = result {
        let _ = writeln!(std::io::stderr(), "{error}");
        return 2;
    }
    0
}

fn run_sync_process_args(args: Vec<OsString>) -> i32 {
    let _telemetry_guard = telemetry::init_from_env(telemetry::TelemetryMode::EnvironmentOnly);
    let run_span = telemetry::run_span();
    let _run_span_guard = run_span.enter();
    let context = CliContext::from_process();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    if let Err(error) = run_with_io(args, &context, &mut stdout, &mut stderr) {
        let _ = writeln!(stderr, "{error}");
        return 2;
    }
    0
}

/// Executes CLI args while allowing selected commands to remain natively async.
pub async fn run_with_io_async<W, E>(
    args: Vec<OsString>,
    context: &CliContext,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), CliError>
where
    W: std::io::Write,
    E: std::io::Write,
{
    run_with_io_async_with_telemetry(args, context, stdout, stderr, None).await
}

async fn run_with_io_async_with_telemetry<W, E>(
    args: Vec<OsString>,
    context: &CliContext,
    stdout: &mut W,
    stderr: &mut E,
    telemetry_shutdown: Option<telemetry::TelemetryShutdownHandle>,
) -> Result<(), CliError>
where
    W: std::io::Write,
    E: std::io::Write,
{
    match CliCommand::parse(args.clone())? {
        CliCommand::Quota(command) => {
            quota::run_quota_command(
                stdout,
                command,
                context.stdin_is_terminal(),
                context.stdout_is_terminal(),
                context.stdout_terminal_width(),
            )
            .await?;
            stderr.flush().map_err(CliError::Stderr)
        }
        CliCommand::Host(command) => {
            host_command::run_host_command(stdout, command, context, telemetry_shutdown).await?;
            stderr.flush().map_err(CliError::Stderr)
        }
        _ => run_with_io(args, context, stdout, stderr),
    }
}

/// Executes CLI args with process-independent IO.
pub fn run_with_io<I, W, E>(
    args: I,
    context: &CliContext,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), CliError>
where
    I: IntoIterator<Item = OsString>,
    W: std::io::Write,
    E: std::io::Write,
{
    let command = CliCommand::parse(args)?;
    match command {
        CliCommand::Serve(command) => {
            let bind_address = LoopbackBindAddress::new(&command.listen_host, command.port)?;
            let upstream_endpoint = UpstreamEndpoint::new(command.upstream_base_url)?;
            let state_db = command.state_db.clone();
            let secret_root = command.secret_root.clone();
            let mut runtime_config = LoopbackRouterRuntimeConfig::new_tokenless(
                bind_address,
                upstream_endpoint,
                command.state_db,
                command.secret_root,
            );
            if let Some(audit_file) = command.audit_file {
                runtime_config = runtime_config.with_audit_file(audit_file);
            }
            if let Some(report_file) = command.websocket_registry_report_file.clone() {
                validate_websocket_registry_report_file(&report_file)?;
                runtime_config = runtime_config.with_websocket_registry_report_file(report_file);
            }
            let token_reload_watcher = if command.require_local_token {
                let secret_store =
                    FileSecretStore::open(&secret_root).map_err(TokenCommandError::SecretStore)?;
                let token_service = LocalRouterTokenService::new(secret_store.clone());
                let local_token = token_service.load_current()?;
                let initial_token_generation = local_token.generation();
                runtime_config = runtime_config.with_required_local_token(local_token);
                Some((secret_store, initial_token_generation))
            } else {
                None
            };
            if let Some(now_unix_seconds) = command.now_unix_seconds {
                runtime_config = runtime_config
                    .with_quota_clock(now_unix_seconds, command.max_snapshot_age_seconds);
            }
            let runtime = LoopbackRouterRuntime::start(runtime_config)?;
            let _token_reload_watcher =
                token_reload_watcher.map(|(secret_store, initial_token_generation)| {
                    LocalTokenReloadWatcher::start(
                        secret_store,
                        runtime.local_auth_reloader(),
                        initial_token_generation,
                    )
                });

            writeln!(stdout, "listening: {}", runtime.local_addr()).map_err(CliError::Stdout)?;
            let _quota_refresh_worker = if command.background_quota_refresh_enabled {
                Some(quota::start_background_quota_refresh_worker(
                    state_db,
                    secret_root,
                    DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned(),
                    Duration::from_secs(command.quota_refresh_interval_seconds),
                    runtime.websocket_quota_floor_notifier(),
                )?)
            } else {
                None
            };
            let handled_connections =
                runtime.serve_protocol_connections(command.max_connections)?;
            if let Some(report_file) = command.websocket_registry_report_file {
                write_websocket_registry_report_file(&report_file, handled_connections, &runtime)?;
            }
        }
        CliCommand::Token(TokenCommand::Init { router_root }) => {
            let store =
                FileSecretStore::open(router_root).map_err(TokenCommandError::SecretStore)?;
            let service = LocalRouterTokenService::new(store);
            let record = service.initialize()?;
            writeln!(stdout, "generation: {}", record.generation().as_u64())
                .map_err(CliError::Stdout)?;
        }
        CliCommand::Token(TokenCommand::Rotate { router_root }) => {
            let store =
                FileSecretStore::open(router_root).map_err(TokenCommandError::SecretStore)?;
            let service = LocalRouterTokenService::new(store);
            let record = service.rotate()?;
            writeln!(stdout, "generation: {}", record.generation().as_u64())
                .map_err(CliError::Stdout)?;
        }
        CliCommand::Token(TokenCommand::Export { router_root, shell }) => {
            let store =
                FileSecretStore::open(router_root).map_err(TokenCommandError::SecretStore)?;
            let service = LocalRouterTokenService::new(store);
            let record = service.load_current()?;
            let assignment =
                export_token_assignment(LOCAL_TOKEN_ENV_VAR, record.token().expose_secret(), shell);
            stdout
                .write_all(assignment.as_bytes())
                .map_err(CliError::Stdout)?;
        }
        CliCommand::Profile(ProfileCommand::Print { port }) => {
            let rendered = CodexRouterProfile::new(port).render();
            stdout
                .write_all(rendered.as_bytes())
                .map_err(CliError::Stdout)?;
        }
        CliCommand::Profile(ProfileCommand::Doctor) => {
            let _ = context.env_var(LOCAL_TOKEN_ENV_VAR);
            stdout
                .write_all(b"local router token: not required\n")
                .map_err(CliError::Stdout)?;
        }
        CliCommand::Profile(ProfileCommand::Write {
            port,
            codex_home,
            dry_run,
            approve_codex_home_write,
        }) => {
            let profile = CodexRouterProfile::new(port);
            let writer = CodexRouterProfileWriter::new(codex_home);
            if dry_run {
                let preview = writer.dry_run(&profile)?;
                writeln!(stdout, "target: {}", preview.target_path().display())
                    .map_err(CliError::Stdout)?;
                write_profile_preview(stdout, &preview).map_err(CliError::Stdout)?;
            } else {
                let written_path = writer.write(&profile, approve_codex_home_write)?;
                writeln!(stdout, "wrote: {}", written_path.display()).map_err(CliError::Stdout)?;
            }
        }
        CliCommand::Account(command) => account::run_account_command(stdout, command)?,
        CliCommand::Quota(_) => return Err(QuotaCommandError::AsyncDispatchRequired.into()),
        CliCommand::Live(command) => live::run_live_command(stdout, command)?,
        CliCommand::Host(_) => return Err(CliError::HostRequiresAsyncDispatch),
        CliCommand::Version => {
            writeln!(stdout, "codex-router {}", env!("CARGO_PKG_VERSION"))
                .map_err(CliError::Stdout)?;
        }
        CliCommand::Help => {
            stdout
                .write_all(HELP_TEXT.as_bytes())
                .map_err(CliError::Stdout)?;
        }
    }

    stderr.flush().map_err(CliError::Stderr)?;
    Ok(())
}

pub(crate) fn router_root_or_default(router_root: Option<PathBuf>) -> Result<PathBuf, CliError> {
    match router_root {
        Some(router_root) => Ok(router_root),
        None => default_router_root(),
    }
}

pub(crate) fn router_secret_root_or_default(
    router_root: Option<PathBuf>,
) -> Result<PathBuf, CliError> {
    match router_root {
        Some(router_root) => Ok(router_root),
        None => Ok(default_router_root()?.join("secrets")),
    }
}

fn default_router_root() -> Result<PathBuf, CliError> {
    let home = std::env::var_os("HOME");

    #[cfg(all(debug_assertions, not(test)))]
    {
        default_router_root_from_environment(
            home,
            std::env::var_os(DEBUG_ROUTER_ROOT_ENV),
            std::env::var_os(USE_HOME_DEFAULT_ENV),
            true,
        )
    }

    #[cfg(not(all(debug_assertions, not(test))))]
    {
        default_router_root_from_environment(home, None, None, false)
    }
}

fn default_router_root_from_environment(
    home: Option<OsString>,
    debug_router_root: Option<OsString>,
    use_home_default: Option<OsString>,
    use_debug_defaults: bool,
) -> Result<PathBuf, CliError> {
    let should_use_debug_defaults = use_debug_defaults && use_home_default.is_none();
    if should_use_debug_defaults
        && let Some(debug_root) = debug_router_root
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    {
        return Ok(debug_root);
    }

    let home = home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(CliError::HomeDirectoryUnavailable)?;
    if should_use_debug_defaults {
        return Ok(debug_default_router_root_for_home(&home));
    }

    Ok(home.join(DEFAULT_ROUTER_ROOT_DIR))
}

fn debug_default_router_root_for_home(home: &Path) -> PathBuf {
    home.join(DEFAULT_DEBUG_ROUTER_ROOT_DIR)
}

pub(crate) fn app_server_socket_or_default(
    context: &CliContext,
    paths: &codex_native_integration::CodexPaths,
) -> Result<PathBuf, &'static str> {
    codex_native_integration::select_app_server_endpoint(
        codex_native_integration::AppServerEndpointSelection {
            paths,
            debug_defaults: cfg!(all(debug_assertions, not(test)))
                && context.env_var(USE_HOME_DEFAULT_ENV).is_none(),
            requested_socket: context.env_var(DEBUG_APP_SERVER_SOCKET_ENV),
        },
    )
}

#[cfg(test)]
fn validate_debug_app_server_socket(debug_socket: PathBuf) -> Result<PathBuf, &'static str> {
    if !debug_socket.is_absolute() {
        return Err("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET must be an absolute path");
    }
    let Some(parent) = debug_socket.parent() else {
        return Err("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET must have a dedicated parent directory");
    };
    if parent == Path::new("/") || parent == std::env::temp_dir() {
        return Err("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET must be inside a dedicated directory");
    }

    Ok(debug_socket)
}

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-cli"
}

/// Process-independent CLI environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliContext {
    env: Vec<(String, String)>,
    current_dir: PathBuf,
}

impl CliContext {
    /// Creates a context from explicit environment pairs.
    #[must_use]
    pub fn new(env: Vec<(String, String)>) -> Self {
        Self {
            env,
            current_dir: current_dir_or_dot(),
        }
    }

    /// Creates a context from the current process environment.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            env: std::env::vars().collect(),
            current_dir: current_dir_or_dot(),
        }
    }

    /// Sets the current directory for process-independent tests.
    #[must_use]
    pub fn with_current_dir(mut self, current_dir: PathBuf) -> Self {
        self.current_dir = current_dir;
        self
    }

    pub(crate) fn env_var(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(env_name, _)| env_name == name)
            .map(|(_, env_value)| env_value.as_str())
            .filter(|env_value| !env_value.is_empty())
    }

    fn stdout_is_terminal(&self) -> bool {
        if self.env_var("CODEX_ROUTER_FORCE_TTY").is_some() {
            return true;
        }
        if self.env_var("CODEX_ROUTER_FORCE_NON_TTY").is_some() {
            return false;
        }
        std::io::stdout().is_terminal()
    }

    fn stdin_is_terminal(&self) -> bool {
        if self.env_var("CODEX_ROUTER_FORCE_TTY").is_some() {
            return true;
        }
        if self.env_var("CODEX_ROUTER_FORCE_NON_TTY").is_some() {
            return false;
        }
        std::io::stdin().is_terminal()
    }

    fn stdout_terminal_width(&self) -> Option<usize> {
        self.env_var("CODEX_ROUTER_FORCE_TTY_WIDTH")
            .or_else(|| self.env_var("COLUMNS"))
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|width| *width > 0)
    }
}

fn current_dir_or_dot() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_error| PathBuf::from("."))
}

const HELP_TEXT: &str = "\
codex-router

commands:
  serve                         Run the local Codex account router
  account login --label <name>  Add an OAuth account
  account list                  Show configured router accounts
  account set-weekly-floor      Set or disable an account weekly quota floor
  quota                         Show quota, refresh state, and next account
  quota refresh                 Refresh quota data now
  host                           Run the foreground shared Codex host
  host status                    Show shared host status
  host restart                   Restart the managed app-server
  host restart-router            Restart the router when host-owned
  host update                    Update Codex and restart the host if changed
  doctor                        Diagnose local router setup
  profile print                 Print the Codex profile snippet

Sessions and agent communication: run agent-sessions --help.
";

#[cfg(test)]
#[path = "cli_contract_tests.rs"]
mod tests;
