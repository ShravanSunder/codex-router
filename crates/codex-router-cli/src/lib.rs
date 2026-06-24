//! Command-line entry points for codex-router.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use codex_router_proxy::server::LocalAuthReloader;
use codex_router_proxy::server::LoopbackBindAddress;
use codex_router_proxy::server::LoopbackRouterRuntime;
use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
use codex_router_proxy::server::LoopbackRouterRuntimeError;
use codex_router_proxy::server::ServerBindError;
use codex_router_proxy::upstream::UpstreamEndpoint;
use codex_router_proxy::upstream::UpstreamEndpointError;
use codex_router_secret_store::file_backend::FileSecretStore;

pub mod account;
mod credential_runtime;
pub mod doctor;
mod live;
pub mod profile;
pub mod quota;
mod secret_store_factory;
pub mod token;

use account::AccountCommand;
use account::AccountCommandError;
use live::LiveCommand;
use profile::CodexRouterProfile;
use profile::CodexRouterProfileWriter;
use profile::ProfileWriteError;
use quota::QuotaCommand;
use quota::QuotaCommandError;
use thiserror::Error;
use token::LocalRouterTokenService;
use token::Shell;
use token::TokenCommandError;
use token::export_token_assignment;

const DEFAULT_PROFILE_PORT: u16 = 8787;
const LOCAL_TOKEN_ENV_VAR: &str = "CODEX_ROUTER_TOKEN";
const DEFAULT_ROUTER_ROOT_DIR: &str = ".codex-router";

/// Runs the process CLI.
pub fn run() {
    let context = CliContext::from_process();
    let args = std::env::args_os();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    if let Err(error) = run_with_io(args, &context, &mut stdout, &mut stderr) {
        let _ = writeln!(stderr, "{error}");
        std::process::exit(2);
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
            let secret_store = FileSecretStore::open(&command.secret_root)
                .map_err(TokenCommandError::SecretStore)?;
            let token_service = LocalRouterTokenService::new(secret_store.clone());
            let local_token = token_service.load_current()?;
            let initial_token_generation = local_token.generation();
            let bind_address = LoopbackBindAddress::new(&command.listen_host, command.port)?;
            let upstream_endpoint = UpstreamEndpoint::new(command.upstream_base_url)?;
            let state_db = command.state_db.clone();
            let secret_root = command.secret_root.clone();
            let runtime_config = LoopbackRouterRuntimeConfig::new(
                bind_address,
                upstream_endpoint,
                command.state_db,
                command.secret_root,
                local_token,
            )
            .with_quota_clock(command.now_unix_seconds, command.max_snapshot_age_seconds)
            .with_max_websocket_upstream_messages(command.max_websocket_upstream_messages);
            let runtime = LoopbackRouterRuntime::start(runtime_config)?;
            let _token_reload_watcher = LocalTokenReloadWatcher::start(
                secret_store,
                runtime.local_auth_reloader(),
                initial_token_generation,
            );

            writeln!(stdout, "listening: {}", runtime.local_addr()).map_err(CliError::Stdout)?;
            let _quota_refresh_worker = if command.background_quota_refresh_enabled {
                Some(quota::start_background_quota_refresh_worker(
                    state_db,
                    secret_root,
                    DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned(),
                    Duration::from_secs(command.quota_refresh_interval_seconds),
                )?)
            } else {
                None
            };
            runtime.serve_protocol_connections(command.max_connections)?;
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
            if context.env_var(LOCAL_TOKEN_ENV_VAR).is_some() {
                stdout
                    .write_all(b"CODEX_ROUTER_TOKEN: present\n")
                    .map_err(CliError::Stdout)?;
            } else {
                stdout
                    .write_all(b"CODEX_ROUTER_TOKEN: missing\n")
                    .map_err(CliError::Stdout)?;
            }
        }
        CliCommand::Profile(ProfileCommand::Write {
            port,
            codex_home,
            dry_run,
            approve_codex_home_write,
            preview_token,
        }) => {
            let profile = CodexRouterProfile::new(port);
            let writer = CodexRouterProfileWriter::new(codex_home);
            if dry_run {
                let preview = writer.dry_run(&profile)?;
                writeln!(stdout, "target: {}", preview.target_path().display())
                    .map_err(CliError::Stdout)?;
                writeln!(stdout, "preview-token: {}", preview.preview_token())
                    .map_err(CliError::Stdout)?;
                write_profile_preview(stdout, &preview).map_err(CliError::Stdout)?;
            } else {
                let written_path =
                    writer.write(&profile, approve_codex_home_write, preview_token.as_deref())?;
                writeln!(stdout, "wrote: {}", written_path.display()).map_err(CliError::Stdout)?;
            }
        }
        CliCommand::Account(command) => account::run_account_command(stdout, command)?,
        CliCommand::Quota(command) => quota::run_quota_command(stdout, command)?,
        CliCommand::Live(command) => live::run_live_command(stdout, command)?,
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
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(CliError::HomeDirectoryUnavailable)?;
    Ok(home.join(DEFAULT_ROUTER_ROOT_DIR))
}

fn write_profile_preview(
    stdout: &mut impl Write,
    preview: &profile::ProfileDryRun,
) -> Result<(), std::io::Error> {
    if let Some(existing_content) = preview.existing_content() {
        stdout.write_all(b"current:\n")?;
        write_prefixed_redacted_lines(stdout, "< ", existing_content)?;
    } else {
        stdout.write_all(b"current: <missing>\n")?;
    }
    stdout.write_all(b"proposed:\n")?;
    write_prefixed_lines(stdout, "> ", preview.content())
}

fn write_prefixed_redacted_lines(
    stdout: &mut impl Write,
    prefix: &str,
    content: &str,
) -> Result<(), std::io::Error> {
    for line in content.lines() {
        writeln!(stdout, "{prefix}{}", redact_profile_preview_line(line))?;
    }
    Ok(())
}

fn redact_profile_preview_line(line: &str) -> String {
    const SECRET_KEYS: &[&str] = &[
        "authorization",
        "bearer",
        "key",
        "oauth",
        "password",
        "secret",
        "token",
    ];

    let lower = line.to_ascii_lowercase();
    if !SECRET_KEYS.iter().any(|key| lower.contains(key)) {
        return line.to_owned();
    }
    if let Some((key, _value)) = line.split_once('=') {
        return format!("{key}= \"<redacted>\"");
    }
    "<redacted>".to_owned()
}

fn write_prefixed_lines(
    stdout: &mut impl Write,
    prefix: &str,
    content: &str,
) -> Result<(), std::io::Error> {
    for line in content.lines() {
        writeln!(stdout, "{prefix}{line}")?;
    }
    Ok(())
}

struct LocalTokenReloadWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LocalTokenReloadWatcher {
    fn start(
        secret_store: FileSecretStore,
        reloader: LocalAuthReloader,
        initial_generation: codex_router_core::ids::TokenGeneration,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let token_service = LocalRouterTokenService::new(secret_store);
            let mut last_generation = initial_generation;
            while !stop_for_thread.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
                let auth = match token_service.load_auth() {
                    Ok(auth) => auth,
                    Err(_error) => continue,
                };
                let current_generation = auth.current_generation();
                if current_generation != last_generation {
                    reloader.reload_auth(auth);
                    last_generation = current_generation;
                }
            }
        });

        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for LocalTokenReloadWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _result = thread.join();
        }
    }
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
}

impl CliContext {
    /// Creates a context from explicit environment pairs.
    #[must_use]
    pub fn new(env: Vec<(String, String)>) -> Self {
        Self { env }
    }

    /// Creates a context from the current process environment.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            env: std::env::vars().collect(),
        }
    }

    fn env_var(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(env_name, _)| env_name == name)
            .map(|(_, env_value)| env_value.as_str())
            .filter(|env_value| !env_value.is_empty())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CliCommand {
    Serve(ServeCommand),
    Token(TokenCommand),
    Profile(ProfileCommand),
    Account(AccountCommand),
    Quota(QuotaCommand),
    Live(LiveCommand),
    Help,
}

impl CliCommand {
    fn parse<I>(args: I) -> Result<Self, CliError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut parser = ArgumentParser::new(args.into_iter().collect());
        let Some(command) = parser.next_string()? else {
            return Ok(Self::Help);
        };
        if is_binary_name(&command) {
            let Some(command_after_binary_name) = parser.next_string()? else {
                return Ok(Self::Help);
            };
            return Self::parse_after_binary(command_after_binary_name, &mut parser);
        }

        Self::parse_after_binary(command, &mut parser)
    }

    fn parse_after_binary(command: String, parser: &mut ArgumentParser) -> Result<Self, CliError> {
        match command.as_str() {
            "serve" => Ok(Self::Serve(ServeCommand::parse(parser)?)),
            "profile" => Ok(Self::Profile(ProfileCommand::parse(parser)?)),
            "token" => Ok(Self::Token(TokenCommand::parse(parser)?)),
            "account" => Ok(Self::Account(AccountCommand::parse(parser)?)),
            "quota" => Ok(Self::Quota(QuotaCommand::parse(parser)?)),
            "live" => Ok(Self::Live(LiveCommand::parse(parser)?)),
            "--help" | "-h" | "help" => Ok(Self::Help),
            unknown => Err(CliError::UnknownCommand {
                command: unknown.to_owned(),
            }),
        }
    }
}

fn is_binary_name(command: &str) -> bool {
    std::path::Path::new(command)
        .file_name()
        .and_then(|file_name| file_name.to_str())
        == Some("codex-router")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServeCommand {
    listen_host: String,
    port: u16,
    state_db: PathBuf,
    secret_root: PathBuf,
    upstream_base_url: String,
    now_unix_seconds: u64,
    max_snapshot_age_seconds: u64,
    quota_refresh_interval_seconds: u64,
    background_quota_refresh_enabled: bool,
    max_websocket_upstream_messages: usize,
    max_connections: usize,
}

impl ServeCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let options = ServeCommandOptions::parse(parser)?;
        let listen_host = options
            .listen_host
            .unwrap_or_else(|| "127.0.0.1".to_owned());
        let port = options.port.unwrap_or(DEFAULT_PROFILE_PORT);
        let router_root = default_router_root()?;
        let state_db = options
            .state_db
            .unwrap_or_else(|| router_root.join("state.sqlite"));
        let secret_root = options
            .secret_root
            .unwrap_or_else(|| router_root.join("secrets"));
        let upstream_base_url = options
            .upstream_base_url
            .unwrap_or_else(|| DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned());

        Ok(Self {
            listen_host,
            port,
            state_db,
            secret_root,
            upstream_base_url,
            now_unix_seconds: options
                .now_unix_seconds
                .map_or_else(current_unix_seconds, Ok)?,
            max_snapshot_age_seconds: options.max_snapshot_age_seconds.unwrap_or(300),
            quota_refresh_interval_seconds: options.quota_refresh_interval_seconds.unwrap_or(300),
            background_quota_refresh_enabled: !options.disable_background_quota_refresh,
            max_websocket_upstream_messages: options
                .max_websocket_upstream_messages
                .unwrap_or(usize::MAX),
            max_connections: options.max_connections.unwrap_or(usize::MAX),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServeCommandOptions {
    listen_host: Option<String>,
    port: Option<u16>,
    state_db: Option<PathBuf>,
    secret_root: Option<PathBuf>,
    upstream_base_url: Option<String>,
    now_unix_seconds: Option<u64>,
    max_snapshot_age_seconds: Option<u64>,
    quota_refresh_interval_seconds: Option<u64>,
    disable_background_quota_refresh: bool,
    max_websocket_upstream_messages: Option<usize>,
    max_connections: Option<usize>,
}

impl ServeCommandOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self {
            listen_host: None,
            port: None,
            state_db: None,
            secret_root: None,
            upstream_base_url: None,
            now_unix_seconds: None,
            max_snapshot_age_seconds: None,
            quota_refresh_interval_seconds: None,
            disable_background_quota_refresh: false,
            max_websocket_upstream_messages: None,
            max_connections: None,
        };

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--listen-host" => {
                    options.listen_host = Some(parser.next_required_value("--listen-host")?);
                }
                "--port" => {
                    let value = parser.next_required_value("--port")?;
                    options.port = Some(parse_port(&value)?);
                }
                "--state-db" => {
                    let value = parser.next_required_value("--state-db")?;
                    options.state_db = Some(PathBuf::from(value));
                }
                "--secret-root" => {
                    let value = parser.next_required_value("--secret-root")?;
                    options.secret_root = Some(PathBuf::from(value));
                }
                "--upstream-base-url" => {
                    options.upstream_base_url =
                        Some(parser.next_required_value("--upstream-base-url")?);
                }
                "--now-unix-seconds" => {
                    let value = parser.next_required_value("--now-unix-seconds")?;
                    options.now_unix_seconds =
                        Some(parse_u64_option("--now-unix-seconds", &value)?);
                }
                "--max-snapshot-age-seconds" => {
                    let value = parser.next_required_value("--max-snapshot-age-seconds")?;
                    options.max_snapshot_age_seconds =
                        Some(parse_u64_option("--max-snapshot-age-seconds", &value)?);
                }
                "--quota-refresh-interval-seconds" => {
                    let value = parser.next_required_value("--quota-refresh-interval-seconds")?;
                    options.quota_refresh_interval_seconds = Some(parse_u64_option(
                        "--quota-refresh-interval-seconds",
                        &value,
                    )?);
                }
                "--disable-background-quota-refresh" => {
                    options.disable_background_quota_refresh = true;
                }
                "--max-connections" => {
                    let value = parser.next_required_value("--max-connections")?;
                    options.max_connections =
                        Some(parse_usize_option("--max-connections", &value)?);
                }
                "--max-websocket-upstream-messages" => {
                    let value = parser.next_required_value("--max-websocket-upstream-messages")?;
                    options.max_websocket_upstream_messages = Some(parse_usize_option(
                        "--max-websocket-upstream-messages",
                        &value,
                    )?);
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TokenCommand {
    Init { router_root: PathBuf },
    Rotate { router_root: PathBuf },
    Export { router_root: PathBuf, shell: Shell },
}

impl TokenCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let Some(command) = parser.next_string()? else {
            return Err(CliError::MissingCommand {
                command: "token".to_owned(),
            });
        };

        match command.as_str() {
            "init" => {
                let options = TokenRootOptions::parse(parser)?;
                let router_root = options.router_root()?;
                Ok(Self::Init { router_root })
            }
            "rotate" => {
                let options = TokenRootOptions::parse(parser)?;
                let router_root = options.router_root()?;
                Ok(Self::Rotate { router_root })
            }
            "export" => {
                let options = TokenExportOptions::parse(parser)?;
                let shell = options.shell;
                let router_root = options.router_root()?;
                Ok(Self::Export { router_root, shell })
            }
            unknown => Err(CliError::UnknownCommand {
                command: format!("token {unknown}"),
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TokenRootOptions {
    router_root: Option<PathBuf>,
}

impl TokenRootOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self { router_root: None };

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    let value = parser.next_required_value("--router-root")?;
                    options.router_root = Some(PathBuf::from(value));
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
        router_secret_root_or_default(self.router_root)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TokenExportOptions {
    router_root: Option<PathBuf>,
    shell: Shell,
}

impl TokenExportOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self {
            router_root: None,
            shell: Shell::Posix,
        };

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    let value = parser.next_required_value("--router-root")?;
                    options.router_root = Some(PathBuf::from(value));
                }
                "--shell" => {
                    let value = parser.next_required_value("--shell")?;
                    options.shell = parse_shell(&value)?;
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
        router_secret_root_or_default(self.router_root)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProfileCommand {
    Print {
        port: u16,
    },
    Doctor,
    Write {
        port: u16,
        codex_home: PathBuf,
        dry_run: bool,
        approve_codex_home_write: bool,
        preview_token: Option<String>,
    },
}

impl ProfileCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let Some(command) = parser.next_string()? else {
            return Err(CliError::MissingCommand {
                command: "profile".to_owned(),
            });
        };

        match command.as_str() {
            "print" => {
                let options = ProfileOptions::parse(parser)?;
                Ok(Self::Print { port: options.port })
            }
            "doctor" => {
                parser.reject_remaining()?;
                Ok(Self::Doctor)
            }
            "write" => {
                let options = ProfileOptions::parse(parser)?;
                let ProfileOptions {
                    port,
                    codex_home,
                    dry_run,
                    approve_codex_home_write,
                    preview_token,
                } = options;
                let codex_home = codex_home.ok_or(CliError::MissingOption {
                    option: "--codex-home",
                })?;
                Ok(Self::Write {
                    port,
                    codex_home,
                    dry_run,
                    approve_codex_home_write,
                    preview_token,
                })
            }
            unknown => Err(CliError::UnknownCommand {
                command: format!("profile {unknown}"),
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProfileOptions {
    port: u16,
    codex_home: Option<PathBuf>,
    dry_run: bool,
    approve_codex_home_write: bool,
    preview_token: Option<String>,
}

impl ProfileOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self {
            port: DEFAULT_PROFILE_PORT,
            codex_home: None,
            dry_run: false,
            approve_codex_home_write: false,
            preview_token: None,
        };

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--port" => {
                    let value = parser.next_required_value("--port")?;
                    options.port = parse_port(&value)?;
                }
                "--codex-home" => {
                    let value = parser.next_required_value("--codex-home")?;
                    options.codex_home = Some(PathBuf::from(value));
                }
                "--dry-run" => {
                    options.dry_run = true;
                }
                "--approve-codex-home-write" => {
                    options.approve_codex_home_write = true;
                }
                "--preview-token" => {
                    let value = parser.next_required_value("--preview-token")?;
                    options.preview_token = Some(value);
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArgumentParser {
    arguments: Vec<OsString>,
    index: usize,
}

impl ArgumentParser {
    fn new(arguments: Vec<OsString>) -> Self {
        Self {
            arguments,
            index: 0,
        }
    }

    pub(crate) fn next_string(&mut self) -> Result<Option<String>, CliError> {
        let Some(argument) = self.arguments.get(self.index) else {
            return Ok(None);
        };
        self.index += 1;
        argument
            .clone()
            .into_string()
            .map(Some)
            .map_err(|value| CliError::NonUtf8Argument { value })
    }

    pub(crate) fn next_required_value(&mut self, option: &'static str) -> Result<String, CliError> {
        self.next_string()?
            .ok_or(CliError::MissingOptionValue { option })
    }

    pub(crate) fn reject_remaining(&mut self) -> Result<(), CliError> {
        if let Some(argument) = self.next_string()? {
            return Err(CliError::UnknownOption { option: argument });
        }
        Ok(())
    }
}

fn parse_port(value: &str) -> Result<u16, CliError> {
    let port = value.parse::<u16>().map_err(|_| CliError::InvalidPort {
        value: value.to_owned(),
    })?;
    if port == 0 {
        return Err(CliError::InvalidPort {
            value: value.to_owned(),
        });
    }

    Ok(port)
}

fn parse_shell(value: &str) -> Result<Shell, CliError> {
    match value {
        "posix" => Ok(Shell::Posix),
        other => Err(CliError::InvalidShell {
            value: other.to_owned(),
        }),
    }
}

fn parse_u64_option(option: &'static str, value: &str) -> Result<u64, CliError> {
    value
        .parse::<u64>()
        .map_err(|_| CliError::InvalidNumericOption {
            option,
            value: value.to_owned(),
        })
}

fn parse_usize_option(option: &'static str, value: &str) -> Result<usize, CliError> {
    value
        .parse::<usize>()
        .map_err(|_| CliError::InvalidNumericOption {
            option,
            value: value.to_owned(),
        })
}

fn current_unix_seconds() -> Result<u64, CliError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(CliError::SystemClock)
}

/// CLI execution failure.
#[derive(Debug, Error)]
pub enum CliError {
    /// Command name is unknown.
    #[error("unknown command: {command}")]
    UnknownCommand {
        /// Unknown command.
        command: String,
    },

    /// A nested command is missing.
    #[error("missing command after {command}")]
    MissingCommand {
        /// Parent command.
        command: String,
    },

    /// An option is unknown.
    #[error("unknown option: {option}")]
    UnknownOption {
        /// Unknown option.
        option: String,
    },

    /// A required option is missing.
    #[error("missing required option: {option}")]
    MissingOption {
        /// Missing option.
        option: &'static str,
    },

    /// HOME is unavailable for the default router root.
    #[error("HOME is not set; pass --router-root <path>")]
    HomeDirectoryUnavailable,

    /// A required option value is missing.
    #[error("missing value for option: {option}")]
    MissingOptionValue {
        /// Option missing its value.
        option: &'static str,
    },

    /// Port is invalid.
    #[error("invalid profile port: {value}")]
    InvalidPort {
        /// Raw port value.
        value: String,
    },

    /// Shell is invalid.
    #[error("invalid shell: {value}")]
    InvalidShell {
        /// Raw shell value.
        value: String,
    },

    /// Numeric option is invalid.
    #[error("invalid numeric value for {option}: {value}")]
    InvalidNumericOption {
        /// Option name.
        option: &'static str,
        /// Raw option value.
        value: String,
    },

    /// CLI argument is not UTF-8.
    #[error("non-UTF-8 CLI argument: {value:?}")]
    NonUtf8Argument {
        /// Raw argument.
        value: OsString,
    },

    /// System clock is before Unix epoch.
    #[error("system clock is before Unix epoch: {0}")]
    SystemClock(std::time::SystemTimeError),

    /// Profile write failed.
    #[error(transparent)]
    ProfileWrite(#[from] ProfileWriteError),

    /// Token command failed.
    #[error(transparent)]
    Token(#[from] TokenCommandError),
    /// Account command failed.
    #[error(transparent)]
    Account(#[from] AccountCommandError),
    /// Quota command failed.
    #[error(transparent)]
    Quota(#[from] QuotaCommandError),
    /// Live quota command needs exactly one source.
    #[error("live quota requires exactly one of --auth-json or --profiles-root")]
    LiveQuotaSourceRequired,
    /// Live quota profile discovery failed.
    #[error("failed to read live quota profiles: {message}")]
    LiveQuotaProfileRead {
        /// Redacted message.
        message: String,
    },
    /// Live quota command found no profiles.
    #[error("no live quota profiles found")]
    NoLiveQuotaProfiles,
    /// Live quota request failed.
    #[error(transparent)]
    LiveQuota(#[from] codex_router_auth::live_quota::LiveQuotaError),

    /// Loopback bind failed.
    #[error(transparent)]
    Bind(#[from] ServerBindError),

    /// Upstream endpoint was invalid.
    #[error(transparent)]
    UpstreamEndpoint(#[from] UpstreamEndpointError),

    /// Router runtime failed.
    #[error(transparent)]
    Runtime(#[from] LoopbackRouterRuntimeError),

    /// Stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),

    /// Stderr write failed.
    #[error("failed to write stderr: {0}")]
    Stderr(std::io::Error),
}

const HELP_TEXT: &str = "\
codex-router

commands:
  serve [--state-db <path>] [--secret-root <path>] [--upstream-base-url <url>] [--quota-refresh-interval-seconds <seconds>] [--disable-background-quota-refresh]
  token init [--router-root <path>]
  token rotate [--router-root <path>]
  token export [--router-root <path>] [--shell posix]
  account login [--router-root <path>] --label <label> --auth-json <path> --allow-plaintext-file-secrets
  account login [--router-root <path>] --label <label> --device-auth [--codex-bin <path>] --allow-plaintext-file-secrets
  account import-codex-auth [--router-root <path>] --label <label> --auth-json <path> --allow-plaintext-file-secrets
  account list [--router-root <path>]
  quota refresh [--router-root <path>] [--base-url <url>]
  quota status [--router-root <path>] [--format table|plain|json] [--all-limits] [--now-unix-seconds <seconds>]
  profile print [--port <port>]
  profile doctor
  profile write --codex-home <path> [--port <port>] [--dry-run]
  profile write --codex-home <path> --approve-codex-home-write --preview-token <token>
  live quota --auth-json <path> [--profile-label <label>] [--base-url <url>]
  live quota --profiles-root <path> [--base-url <url>]
";

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::fs;
    use std::io::Read;
    use std::io::Write;
    use std::net::Shutdown;
    use std::net::TcpListener;
    use std::net::TcpStream;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use std::time::Instant;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;
    use tungstenite::Message;
    use tungstenite::accept_hdr;
    use tungstenite::client::IntoClientRequest;
    use tungstenite::connect;
    use tungstenite::handshake::server::Request;
    use tungstenite::handshake::server::Response;
    use tungstenite::http::HeaderValue;

    use codex_router_auth::resolver::CredentialRefreshClient;
    use codex_router_auth::resolver::CredentialResolverError;
    use codex_router_auth::resolver::NoopCredentialRefreshClient;
    use codex_router_auth::resolver::ProviderCredentialResolver;
    use codex_router_auth::resolver::RouterCredentialResolver;
    use codex_router_core::ids::AccountId;
    use codex_router_core::redaction::SecretString;
    use codex_router_proxy::server::LoopbackBindAddress;
    use codex_router_proxy::server::LoopbackRouterRuntime;
    use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
    use codex_router_proxy::upstream::UpstreamEndpoint;
    use codex_router_secret_store::SecretStore;
    use codex_router_secret_store::account_tokens::AccountCredentialBundle;
    use codex_router_secret_store::account_tokens::account_credential_bundle_key;
    use codex_router_secret_store::account_tokens::upstream_access_token_key;
    use codex_router_secret_store::file_backend::FileSecretStore;
    use codex_router_secret_store::model::SecretKey;
    use codex_router_secret_store::model::SecretStoreError;
    use codex_router_state::account::AccountRecord;
    use codex_router_state::account::AccountStatus;
    use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
    use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
    use codex_router_state::quota_snapshot::QuotaSnapshotSource;
    use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
    use codex_router_state::repositories::AccountStateRepository;
    use codex_router_state::repositories::QuotaSnapshotRepository;
    use codex_router_state::repositories::SelectorQuotaRepository;
    use codex_router_state::sqlite::SqliteStateStore;

    use super::CliCommand;
    use super::CliContext;
    use super::TokenCommand;
    use super::package_name;
    use super::run_with_io;
    use crate::account::AccountCommand;
    use crate::account::AccountImportRequest;
    use crate::account::import_codex_auth_from_request;
    use crate::credential_runtime::CliCredentialResolver;
    use crate::doctor::DoctorAccountState;
    use crate::doctor::DoctorReport;
    use crate::doctor::QuotaDoctorState;
    use crate::profile::CodexRouterProfile;
    use crate::profile::CodexRouterProfileWriter;
    use crate::profile::ProfileWriteError;
    use crate::quota::BackgroundQuotaRefreshRuntime;
    use crate::quota::HttpQuotaRefreshProvider;
    use crate::quota::QuotaCommand;
    use crate::quota::QuotaRefreshProvider;
    use crate::quota::QuotaRefreshProviderRequest;
    use crate::quota::QuotaRefreshProviderResponse;
    use crate::quota::QuotaRefreshProviderWindow;
    use crate::quota::refresh_quota_store_paths_with_dependencies;
    use crate::quota::refresh_quota_with_dependencies;
    use crate::quota::start_background_quota_refresh_worker_with_clock;
    use crate::quota::start_background_quota_refresh_worker_with_dependencies;
    use crate::quota::start_background_quota_refresh_worker_with_reporter;
    use crate::token::LocalRouterTokenService;
    use crate::token::Shell;
    use crate::token::export_token_assignment;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(name: &str) -> Self {
            let counter = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "codex-router-cli-token-{name}-{}-{counter}",
                std::process::id()
            ));
            if path.exists() {
                remove_dir_all(&path);
            }

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    fn default_router_root_for_test() -> PathBuf {
        let Some(home) = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        else {
            panic!("HOME must be set for default router root tests");
        };

        home.join(".codex-router")
    }

    fn default_router_secret_root_for_test() -> PathBuf {
        default_router_root_for_test().join("secrets")
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            if self.path.exists() {
                remove_dir_all(&self.path);
            }
        }
    }

    struct FailingSecretStore {
        write_attempts: AtomicUsize,
    }

    impl FailingSecretStore {
        fn new() -> Self {
            Self {
                write_attempts: AtomicUsize::new(0),
            }
        }

        fn write_attempts(&self) -> usize {
            self.write_attempts.load(Ordering::SeqCst)
        }
    }

    impl SecretStore for FailingSecretStore {
        fn write_secret(
            &self,
            _key: &SecretKey,
            _secret: &SecretString,
        ) -> Result<(), SecretStoreError> {
            self.write_attempts.fetch_add(1, Ordering::SeqCst);

            Err(SecretStoreError::Filesystem {
                path: PathBuf::from("injected-secret-store-failure"),
                source: std::io::Error::other("injected secret-store failure"),
            })
        }

        fn read_secret(&self, _key: &SecretKey) -> Result<SecretString, SecretStoreError> {
            Err(SecretStoreError::Filesystem {
                path: PathBuf::from("injected-secret-store-failure"),
                source: std::io::Error::other("injected secret-store failure"),
            })
        }
    }

    fn preview_token_from_stdout(stdout: &str) -> &str {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("preview-token: "))
            .unwrap_or_else(|| panic!("preview-token line missing from output:\n{stdout}"))
    }

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-cli");
    }

    #[test]
    fn process_binary_path_is_skipped_before_command_parse() {
        let output = run_cli(
            ["/tmp/build/target/debug/codex-router", "--help"],
            CliContext::new(Vec::new()),
        );

        for expected_line in [
            "serve [--state-db <path>] [--secret-root <path>] [--upstream-base-url <url>] [--quota-refresh-interval-seconds <seconds>] [--disable-background-quota-refresh]",
            "account login [--router-root <path>] --label <label> --auth-json <path> --allow-plaintext-file-secrets",
            "account login [--router-root <path>] --label <label> --device-auth [--codex-bin <path>] --allow-plaintext-file-secrets",
            "quota refresh [--router-root <path>] [--base-url <url>]",
            "quota status [--router-root <path>] [--format table|plain|json] [--all-limits] [--now-unix-seconds <seconds>]",
            "live quota --auth-json <path> [--profile-label <label>] [--base-url <url>]",
        ] {
            assert!(
                output.stdout.contains(expected_line),
                "help output missing expected line: {expected_line}\n{}",
                output.stdout
            );
        }
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn token_export_is_single_assignment_without_prose() {
        let assignment =
            export_token_assignment("CODEX_ROUTER_TOKEN", "quote'and\nnewline", Shell::Posix);

        assert!(assignment.starts_with("export CODEX_ROUTER_TOKEN='"));
        assert!(assignment.ends_with("'\n"));
        assert_eq!(assignment.matches("CODEX_ROUTER_TOKEN=").count(), 1);
        assert!(assignment.contains("'\\''"));
    }

    #[test]
    fn token_service_rotates_through_real_secret_store() {
        let test_root = TestRoot::new("service");
        let store = must_ok(FileSecretStore::open(test_root.path()));
        let service = LocalRouterTokenService::new(store);

        let first = must_ok(service.rotate_with_token("first-token"));
        let second = must_ok(service.rotate_with_token("second-token"));

        assert_eq!(first.generation().as_u64(), 1);
        assert_eq!(second.generation().as_u64(), 2);

        let loaded = must_ok(service.load_current());
        assert_eq!(loaded.token().expose_secret(), "second-token");
        assert_eq!(loaded.generation().as_u64(), 2);
    }

    #[test]
    fn doctor_reports_stale_and_missing_state_without_secrets() {
        let report = DoctorReport::new(vec![
            DoctorAccountState::new(
                "primary",
                QuotaDoctorState::Stale {
                    age_seconds: 600,
                    secret_canary: "refresh-token-canary".to_owned(),
                },
            ),
            DoctorAccountState::new("secondary", QuotaDoctorState::Missing),
        ]);

        let rendered = report.render();

        assert!(rendered.contains("primary"));
        assert!(rendered.contains("quota: stale age=600s"));
        assert!(rendered.contains("secondary"));
        assert!(rendered.contains("quota: missing"));
        assert!(!rendered.contains("refresh-token-canary"));
        assert!(!rendered.contains("token"));
    }

    #[test]
    fn profile_render_includes_codex_custom_provider_contract() {
        let profile = CodexRouterProfile::new(8787);
        let rendered = profile.render();

        assert!(!rendered.contains("[profiles.codex-router]\n"));
        assert!(rendered.contains("model_provider = \"codex-router\"\n"));
        assert!(rendered.contains("[model_providers.codex-router]\n"));
        assert!(rendered.contains("name = \"codex-router\"\n"));
        assert!(rendered.contains("base_url = \"http://127.0.0.1:8787/v1\"\n"));
        assert!(rendered.contains("wire_api = \"responses\"\n"));
        assert!(rendered.contains("requires_openai_auth = false\n"));
        assert!(rendered.contains("supports_websockets = true\n"));
        assert!(rendered.contains("env_key = \"CODEX_ROUTER_TOKEN\"\n"));
        assert!(!rendered.contains("env_http_headers"));
        assert!(!rendered.contains("sk-"));
        assert!(!rendered.contains("oauth"));
    }

    #[test]
    fn profile_writer_dry_run_and_approval_gate_do_not_touch_real_codex_home() {
        let test_root = TestRoot::new("profile");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");
        let profile = CodexRouterProfile::new(8787);
        let writer = CodexRouterProfileWriter::new(&codex_home);

        let dry_run = must_ok(writer.dry_run(&profile));

        assert_eq!(
            dry_run.target_path(),
            codex_home.join("codex-router.config.toml")
        );
        assert!(!dry_run.content().contains("[profiles.codex-router]"));
        assert!(!dry_run.target_path().exists());
        assert_eq!(
            writer.write(&profile, false, Some(dry_run.preview_token())),
            Err(ProfileWriteError::ApprovalRequired)
        );
        assert!(!dry_run.target_path().exists());

        let written_path = must_ok(writer.write(&profile, true, Some(dry_run.preview_token())));

        assert_eq!(written_path, dry_run.target_path());
        assert_eq!(must_ok(fs::read_to_string(&written_path)), profile.render());
    }

    #[test]
    fn profile_print_command_renders_profile_without_writing() {
        let test_root = TestRoot::new("profile-print");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");

        let output = run_cli(
            [
                "codex-router",
                "profile",
                "print",
                "--port",
                "9876",
                "--codex-home",
                path_to_str(&codex_home),
            ],
            CliContext::new(Vec::new()),
        );

        assert!(!output.stdout.contains("[profiles.codex-router]\n"));
        assert!(
            output
                .stdout
                .contains("model_provider = \"codex-router\"\n")
        );
        assert!(output.stdout.contains("name = \"codex-router\"\n"));
        assert!(
            output
                .stdout
                .contains("base_url = \"http://127.0.0.1:9876/v1\"\n")
        );
        assert!(!codex_home.join("codex-router.config.toml").exists());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn profile_print_emits_router_custom_provider_without_home_mutation() {
        let test_root = TestRoot::new("profile-print-plan-row");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");

        let output = run_cli(
            [
                "codex-router",
                "profile",
                "print",
                "--port",
                "9876",
                "--codex-home",
                path_to_str(&codex_home),
            ],
            CliContext::new(Vec::new()),
        );

        assert_router_profile_contract(&output.stdout, 9876);
        assert!(!codex_home.join("codex-router.config.toml").exists());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn profile_doctor_reports_token_presence_without_token_value() {
        let output = run_cli(
            ["codex-router", "profile", "doctor"],
            CliContext::new(vec![(
                "CODEX_ROUTER_TOKEN".to_owned(),
                "local-secret-token-canary".to_owned(),
            )]),
        );

        assert!(output.stdout.contains("CODEX_ROUTER_TOKEN: present\n"));
        assert!(!output.stdout.contains("local-secret-token-canary"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn token_export_and_profile_doctor_redact_router_token_value() {
        let test_root = TestRoot::new("token-export-profile-doctor");
        let store = must_ok(FileSecretStore::open(test_root.path()));
        let service = LocalRouterTokenService::new(store);
        let record = must_ok(service.rotate_with_token("router-token-canary"));

        let export_output = run_cli(
            [
                "codex-router",
                "token",
                "export",
                "--router-root",
                path_to_str(test_root.path()),
            ],
            CliContext::new(Vec::new()),
        );
        let doctor_output = run_cli(
            ["codex-router", "profile", "doctor"],
            CliContext::new(vec![(
                "CODEX_ROUTER_TOKEN".to_owned(),
                record.token().expose_secret().to_owned(),
            )]),
        );

        assert!(
            export_output
                .stdout
                .starts_with("export CODEX_ROUTER_TOKEN='")
        );
        assert!(
            doctor_output
                .stdout
                .contains("CODEX_ROUTER_TOKEN: present\n")
        );
        assert!(
            !doctor_output
                .stdout
                .contains(record.token().expose_secret())
        );
        assert!(export_output.stderr.is_empty());
        assert!(doctor_output.stderr.is_empty());
    }

    #[test]
    fn profile_write_dry_run_previews_target_without_writing() {
        let test_root = TestRoot::new("profile-dry-run");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");

        let output = run_cli(
            [
                "codex-router",
                "profile",
                "write",
                "--codex-home",
                path_to_str(&codex_home),
                "--port",
                "9876",
                "--dry-run",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(
            output.stdout.contains(
                format!(
                    "target: {}",
                    codex_home.join("codex-router.config.toml").display()
                )
                .as_str()
            )
        );
        assert!(output.stdout.contains("preview-token: "));
        assert!(output.stdout.contains("current: <missing>\n"));
        assert!(output.stdout.contains("proposed:\n"));
        assert!(
            output
                .stdout
                .contains("base_url = \"http://127.0.0.1:9876/v1\"\n")
        );
        assert!(!codex_home.join("codex-router.config.toml").exists());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn profile_write_dry_run_previews_named_profile_without_mutation() {
        let test_root = TestRoot::new("profile-dry-run-plan-row");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");

        let output = run_cli(
            [
                "codex-router",
                "profile",
                "write",
                "--codex-home",
                path_to_str(&codex_home),
                "--port",
                "9876",
                "--dry-run",
            ],
            CliContext::new(Vec::new()),
        );

        let target_path = codex_home.join("codex-router.config.toml");
        assert!(
            output
                .stdout
                .contains(format!("target: {}", target_path.display()).as_str())
        );
        assert!(output.stdout.contains("preview-token: "));
        assert!(output.stdout.contains("current: <missing>\n"));
        assert!(output.stdout.contains("proposed:\n"));
        assert_router_profile_contract(&output.stdout, 9876);
        assert!(!target_path.exists());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn profile_write_dry_run_previews_existing_file_delta_without_writing() {
        let test_root = TestRoot::new("profile-existing-dry-run");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");
        must_ok(fs::create_dir(&codex_home));
        let target_path = codex_home.join("codex-router.config.toml");
        must_ok(fs::write(
            &target_path,
            "model_provider = \"old-router\"\nlegacy = true\n",
        ));

        let output = run_cli(
            [
                "codex-router",
                "profile",
                "write",
                "--codex-home",
                path_to_str(&codex_home),
                "--port",
                "9876",
                "--dry-run",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(
            output
                .stdout
                .contains(format!("target: {}", target_path.display()).as_str())
        );
        assert!(output.stdout.contains("preview-token: "));
        assert!(output.stdout.contains("current:\n"));
        assert!(
            output
                .stdout
                .contains("< model_provider = \"old-router\"\n")
        );
        assert!(output.stdout.contains("< legacy = true\n"));
        assert!(output.stdout.contains("proposed:\n"));
        assert!(
            output
                .stdout
                .contains("> base_url = \"http://127.0.0.1:9876/v1\"\n")
        );
        assert_eq!(
            must_ok(fs::read_to_string(&target_path)),
            "model_provider = \"old-router\"\nlegacy = true\n"
        );
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn profile_write_dry_run_redacts_existing_secret_values() {
        let test_root = TestRoot::new("profile-redacted-dry-run");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");
        must_ok(fs::create_dir(&codex_home));
        let target_path = codex_home.join("codex-router.config.toml");
        must_ok(fs::write(
            &target_path,
            "api_key = \"local-secret-canary\"\nmodel_provider = \"old-router\"\n",
        ));

        let output = run_cli(
            [
                "codex-router",
                "profile",
                "write",
                "--codex-home",
                path_to_str(&codex_home),
                "--dry-run",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(!output.stdout.contains("local-secret-canary"));
        assert!(output.stdout.contains("< api_key = \"<redacted>\"\n"));
        assert!(
            output
                .stdout
                .contains("< model_provider = \"old-router\"\n")
        );
        assert_eq!(
            must_ok(fs::read_to_string(&target_path)),
            "api_key = \"local-secret-canary\"\nmodel_provider = \"old-router\"\n"
        );
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn profile_write_rejects_stale_preview_after_target_appears() {
        let test_root = TestRoot::new("profile-stale-missing");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");
        let profile = CodexRouterProfile::new(8787);
        let writer = CodexRouterProfileWriter::new(&codex_home);

        let dry_run = must_ok(writer.dry_run(&profile));
        let target_path = dry_run.target_path();
        must_ok(fs::create_dir(&codex_home));
        must_ok(fs::write(&target_path, ""));

        assert_eq!(
            writer.write(&profile, true, Some(dry_run.preview_token())),
            Err(ProfileWriteError::PreviewTokenMismatch)
        );
        assert_eq!(must_ok(fs::read_to_string(&target_path)), "");
    }

    #[test]
    fn profile_write_rejects_stale_preview_after_target_changes() {
        let test_root = TestRoot::new("profile-stale-changed");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");
        must_ok(fs::create_dir(&codex_home));
        let target_path = codex_home.join("codex-router.config.toml");
        must_ok(fs::write(&target_path, "model_provider = \"old-router\"\n"));
        let profile = CodexRouterProfile::new(8787);
        let writer = CodexRouterProfileWriter::new(&codex_home);

        let dry_run = must_ok(writer.dry_run(&profile));
        must_ok(fs::write(
            &target_path,
            "model_provider = \"changed-router\"\n",
        ));

        assert_eq!(
            writer.write(&profile, true, Some(dry_run.preview_token())),
            Err(ProfileWriteError::PreviewTokenMismatch)
        );
        assert_eq!(
            must_ok(fs::read_to_string(&target_path)),
            "model_provider = \"changed-router\"\n"
        );
    }

    #[test]
    fn profile_write_rejects_unreadable_existing_profile() {
        let test_root = TestRoot::new("profile-invalid-utf8");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");
        must_ok(fs::create_dir(&codex_home));
        let target_path = codex_home.join("codex-router.config.toml");
        must_ok(fs::write(&target_path, [0xff, 0xfe, 0xfd]));
        let profile = CodexRouterProfile::new(8787);
        let writer = CodexRouterProfileWriter::new(&codex_home);

        let error = match writer.dry_run(&profile) {
            Ok(_) => panic!("invalid UTF-8 profile must not be previewed as missing"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("profile filesystem error"));
        assert_eq!(must_ok(fs::read(&target_path)), vec![0xff, 0xfe, 0xfd]);
    }

    #[test]
    fn profile_write_command_requires_approval_flag() {
        let test_root = TestRoot::new("profile-write-approval");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = match run_with_io(
            vec![
                "codex-router".into(),
                "profile".into(),
                "write".into(),
                "--codex-home".into(),
                codex_home.as_os_str().to_owned(),
            ],
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("profile write without approval must fail"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "explicit approval is required before writing Codex profile files"
        );
        assert!(!codex_home.join("codex-router.config.toml").exists());
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn profile_write_command_requires_preview_token_with_approval_flag() {
        let test_root = TestRoot::new("profile-write-preview-required");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = match run_with_io(
            vec![
                "codex-router".into(),
                "profile".into(),
                "write".into(),
                "--codex-home".into(),
                codex_home.as_os_str().to_owned(),
                "--approve-codex-home-write".into(),
            ],
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("profile write without preview token must fail"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "profile preview token is required before writing Codex profile files"
        );
        assert!(!codex_home.join("codex-router.config.toml").exists());
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn profile_write_command_with_preview_token_writes_only_temp_codex_home() {
        let test_root = TestRoot::new("profile-write-preview-approved");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");
        let preview_output = run_cli(
            [
                "codex-router",
                "profile",
                "write",
                "--codex-home",
                path_to_str(&codex_home),
                "--dry-run",
            ],
            CliContext::new(Vec::new()),
        );
        let preview_token = preview_token_from_stdout(&preview_output.stdout);

        let output = run_cli(
            [
                "codex-router",
                "profile",
                "write",
                "--codex-home",
                path_to_str(&codex_home),
                "--approve-codex-home-write",
                "--preview-token",
                preview_token,
            ],
            CliContext::new(Vec::new()),
        );

        let target_path = codex_home.join("codex-router.config.toml");
        assert!(
            output
                .stdout
                .contains(format!("wrote: {}", target_path.display()).as_str())
        );
        assert_eq!(
            must_ok(fs::read_to_string(&target_path)),
            CodexRouterProfile::new(8787).render()
        );
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn profile_write_approved_writes_only_named_temp_profile_file() {
        let test_root = TestRoot::new("profile-write-plan-row");
        must_ok(fs::create_dir(test_root.path()));
        let codex_home = test_root.path().join("codex-home");
        let preview_output = run_cli(
            [
                "codex-router",
                "profile",
                "write",
                "--codex-home",
                path_to_str(&codex_home),
                "--port",
                "9876",
                "--dry-run",
            ],
            CliContext::new(Vec::new()),
        );
        let preview_token = preview_token_from_stdout(&preview_output.stdout);

        let output = run_cli(
            [
                "codex-router",
                "profile",
                "write",
                "--codex-home",
                path_to_str(&codex_home),
                "--port",
                "9876",
                "--approve-codex-home-write",
                "--preview-token",
                preview_token,
            ],
            CliContext::new(Vec::new()),
        );

        let target_path = codex_home.join("codex-router.config.toml");
        assert!(
            output
                .stdout
                .contains(format!("wrote: {}", target_path.display()).as_str())
        );
        assert_eq!(
            must_ok(fs::read_to_string(&target_path)),
            CodexRouterProfile::new(9876).render()
        );
        assert!(output.stderr.is_empty());
    }

    fn assert_router_profile_contract(output: &str, port: u16) {
        assert!(!output.contains("[profiles.codex-router]\n"));
        assert!(output.contains("model_provider = \"codex-router\"\n"));
        assert!(output.contains("[model_providers.codex-router]\n"));
        assert!(output.contains("name = \"codex-router\"\n"));
        assert!(output.contains(format!("base_url = \"http://127.0.0.1:{port}/v1\"\n").as_str()));
        assert!(output.contains("wire_api = \"responses\"\n"));
        assert!(output.contains("requires_openai_auth = false\n"));
        assert!(output.contains("supports_websockets = true\n"));
        assert!(output.contains("env_key = \"CODEX_ROUTER_TOKEN\"\n"));
        assert!(!output.contains("env_http_headers"));
        assert!(!output.contains("sk-"));
        assert!(!output.contains("oauth"));
    }

    #[test]
    fn account_login_auth_json_writes_router_owned_state_and_guides_next_steps() {
        let test_root = TestRoot::new("account-login-auth-json");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        let auth_json = test_root.path().join("auth.json");
        let id_token = fake_id_token_with_chatgpt_account_id("chatgpt-account-id-canary");
        let auth_json_text = format!(
            r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"access-token-canary","refresh_token":"refresh-token-canary","id_token":"{id_token}"}}}}"#
        );
        must_ok(fs::write(&auth_json, &auth_json_text));

        let output = run_cli(
            [
                "codex-router",
                "account",
                "login",
                "--router-root",
                path_to_str(&router_root),
                "--label",
                "primary",
                "--auth-json",
                path_to_str(&auth_json),
                "--allow-plaintext-file-secrets",
            ],
            CliContext::new(Vec::new()),
        );

        let account_id = account_id("acct_primary");
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
            .unwrap_or_else(|| panic!("logged-in account metadata should exist"));
        assert_eq!(account.label(), "primary");
        assert_eq!(account.status(), AccountStatus::Enabled);
        assert_eq!(account.active_credential_generation(), Some(1));

        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        let bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
            secrets.read_secret(&bundle_key),
        )));
        assert_eq!(bundle.access_token().expose_secret(), "access-token-canary");
        assert_eq!(
            bundle.refresh_token().map(SecretString::expose_secret),
            Some("refresh-token-canary")
        );
        assert_eq!(
            bundle.chatgpt_account_id(),
            Some("chatgpt-account-id-canary")
        );
        assert!(output.stdout.contains("logged in account: primary\n"));
        assert!(output.stdout.contains("account_id: acct_primary\n"));
        assert!(
            output
                .stdout
                .contains("next: codex-router quota refresh --router-root ")
        );
        assert!(!output.stdout.contains("access-token-canary"));
        assert!(!output.stdout.contains("refresh-token-canary"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn account_login_device_auth_delegates_to_codex_and_imports_resulting_auth_json() {
        let test_root = TestRoot::new("account-login-device-auth");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        let codex_bin = test_root.path().join("fake-codex");
        let invocation_log = test_root.path().join("codex-invocation.log");
        let codex_home_mode_log = test_root.path().join("codex-home-mode.log");
        must_ok(fs::write(
            &codex_bin,
            format!(
                r#"#!/bin/sh
set -eu
printf '%s\n' "$*" > "{}"
if stat -f %Lp "$CODEX_HOME" > /dev/null 2>&1; then
  stat -f %Lp "$CODEX_HOME" > "{}"
else
  stat -c %a "$CODEX_HOME" > "{}"
fi
test "$1" = "login"
test "$2" = "--device-auth"
test -n "${{CODEX_HOME:-}}"
cat > "$CODEX_HOME/auth.json" <<'JSON'
{{"auth_mode":"chatgpt","tokens":{{"access_token":"device-access-canary","refresh_token":"device-refresh-canary"}}}}
JSON
"#,
                invocation_log.display(),
                codex_home_mode_log.display(),
                codex_home_mode_log.display()
            ),
        ));
        let mut permissions = must_ok(fs::metadata(&codex_bin)).permissions();
        permissions.set_mode(0o700);
        must_ok(fs::set_permissions(&codex_bin, permissions));

        let output = run_cli(
            [
                "codex-router",
                "account",
                "login",
                "--router-root",
                path_to_str(&router_root),
                "--label",
                "device primary",
                "--device-auth",
                "--codex-bin",
                path_to_str(&codex_bin),
                "--allow-plaintext-file-secrets",
            ],
            CliContext::new(Vec::new()),
        );

        let account_id = account_id("acct_device_primary");
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
            .unwrap_or_else(|| panic!("device-auth account metadata should exist"));
        assert_eq!(account.label(), "device primary");
        assert_eq!(account.status(), AccountStatus::Enabled);
        assert_eq!(account.active_credential_generation(), Some(1));

        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        let bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
            secrets.read_secret(&bundle_key),
        )));
        assert_eq!(
            bundle.access_token().expose_secret(),
            "device-access-canary"
        );
        assert_eq!(
            bundle.refresh_token().map(SecretString::expose_secret),
            Some("device-refresh-canary")
        );
        assert_eq!(
            must_ok(fs::read_to_string(invocation_log)),
            "login --device-auth\n"
        );
        assert_eq!(must_ok(fs::read_to_string(codex_home_mode_log)), "700\n");
        assert!(
            output
                .stdout
                .contains("logged in account: device primary\n")
        );
        assert!(output.stdout.contains("account_id: acct_device_primary\n"));
        assert!(!output.stdout.contains("device-access-canary"));
        assert!(!output.stdout.contains("device-refresh-canary"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn account_login_device_auth_cleans_temporary_codex_home_on_failure() {
        let test_root = TestRoot::new("account-login-device-auth-failure");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        let codex_bin = test_root.path().join("failing-codex");
        let codex_home_log = test_root.path().join("codex-home.log");
        must_ok(fs::write(
            &codex_bin,
            format!(
                r#"#!/bin/sh
set -eu
printf '%s\n' "$CODEX_HOME" > "{}"
exit 42
"#,
                codex_home_log.display()
            ),
        ));
        let mut permissions = must_ok(fs::metadata(&codex_bin)).permissions();
        permissions.set_mode(0o700);
        must_ok(fs::set_permissions(&codex_bin, permissions));

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = match run_with_io(
            vec![
                "codex-router".into(),
                "account".into(),
                "login".into(),
                "--router-root".into(),
                router_root.as_os_str().to_owned(),
                "--label".into(),
                "device primary".into(),
                "--device-auth".into(),
                "--codex-bin".into(),
                codex_bin.as_os_str().to_owned(),
                "--allow-plaintext-file-secrets".into(),
            ],
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("device-auth failure should surface"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("codex device-auth login failed"));
        let temporary_codex_home =
            PathBuf::from(must_ok(fs::read_to_string(codex_home_log)).trim());
        assert!(!temporary_codex_home.exists());
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn account_import_codex_auth_writes_router_owned_state_and_secrets() {
        let test_root = TestRoot::new("account-import");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        let auth_json = test_root.path().join("auth.json");
        let id_token = fake_id_token_with_chatgpt_account_id("chatgpt-account-id-canary");
        let auth_json_text = format!(
            r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"access-token-canary","refresh_token":"refresh-token-canary","id_token":"{id_token}"}}}}"#
        );
        must_ok(fs::write(&auth_json, &auth_json_text));

        let output = run_cli(
            [
                "codex-router",
                "account",
                "import-codex-auth",
                "--router-root",
                path_to_str(&router_root),
                "--label",
                "primary",
                "--auth-json",
                path_to_str(&auth_json),
                "--allow-plaintext-file-secrets",
            ],
            CliContext::new(Vec::new()),
        );

        let account_id = account_id("acct_primary");
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
            .unwrap_or_else(|| panic!("imported account metadata should exist"));
        assert_eq!(account.label(), "primary");
        assert_eq!(account.status(), AccountStatus::Enabled);
        assert_eq!(account.active_credential_generation(), Some(1));

        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        let bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
            secrets.read_secret(&bundle_key),
        )));
        assert_eq!(bundle.access_token().expose_secret(), "access-token-canary");
        assert_eq!(
            bundle.refresh_token().map(SecretString::expose_secret),
            Some("refresh-token-canary")
        );
        assert_eq!(
            bundle.chatgpt_account_id(),
            Some("chatgpt-account-id-canary")
        );
        assert!(output.stdout.contains("imported account: primary\n"));
        assert!(output.stdout.contains("account_id: acct_primary\n"));
        assert!(!output.stdout.contains("access-token-canary"));
        assert!(!output.stdout.contains("refresh-token-canary"));
        assert!(output.stderr.is_empty());
        assert_eq!(must_ok(fs::read_to_string(&auth_json)), auth_json_text);
    }

    #[test]
    fn account_import_codex_auth_redacts_refresh_token_in_error_paths() {
        let test_root = TestRoot::new("account-import-redaction");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        let auth_json = test_root.path().join("auth.json");
        must_ok(fs::write(
            &auth_json,
            r#"{"auth_mode":"chatgpt","tokens":{"refresh_token":"refresh-token-canary","access_token":""}}"#,
        ));

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = match run_with_io(
            vec![
                "codex-router".into(),
                "account".into(),
                "import-codex-auth".into(),
                "--router-root".into(),
                router_root.as_os_str().to_owned(),
                "--label".into(),
                "primary".into(),
                "--auth-json".into(),
                auth_json.as_os_str().to_owned(),
                "--allow-plaintext-file-secrets".into(),
            ],
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("missing access token must fail"),
            Err(error) => error,
        };
        let rendered_error = error.to_string();

        assert_eq!(rendered_error, "access token not found in auth json");
        assert!(!rendered_error.contains("refresh-token-canary"));
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn account_import_codex_auth_partial_secret_write_disables_account_until_repair() {
        let test_root = TestRoot::new("account-import-partial-secret");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let failing_secrets = FailingSecretStore::new();
        let request =
            AccountImportRequest::new(account_id("acct_primary"), "primary", "access-token-canary")
                .with_refresh_token("refresh-token-canary");

        let error = must_err(import_codex_auth_from_request(
            &state,
            &failing_secrets,
            request,
        ));

        assert!(error.to_string().contains("secret store"));
        let account = must_ok(AccountStateRepository::load_account(
            &state,
            &account_id("acct_primary"),
        ))
        .unwrap_or_else(|| panic!("failed import should leave disabled account metadata"));
        assert_eq!(account.status(), AccountStatus::Disabled);
        assert_eq!(account.active_credential_generation(), None);
        assert_eq!(failing_secrets.write_attempts(), 1);
    }

    #[test]
    fn account_import_codex_auth_invalidates_quota_snapshot_on_credential_mutation() {
        let test_root = TestRoot::new("account-import-invalidates-quota");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        let auth_json = test_root.path().join("auth.json");
        must_ok(fs::create_dir_all(&router_root));
        must_ok(fs::write(
            &auth_json,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"new-access-token","refresh_token":"new-refresh-token"}}"#,
        ));
        let account_id = account_id("acct_primary");
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(account_id.clone(), "primary", AccountStatus::Enabled)
                .with_active_credential_generation(1),
        ));
        for route_band in [
            "responses",
            "models",
            "memories_trace_summarize",
            "responses_compact",
            "code_review",
        ] {
            must_ok(QuotaSnapshotRepository::upsert_snapshot(
                &state,
                &PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                    .with_observed_unix_seconds(9_000)
                    .with_route_band(route_band, 99)
                    .with_reset_unix_seconds(10_000)
                    .with_stale_penalty(false),
            ));
        }

        let output = run_cli(
            [
                "codex-router",
                "account",
                "import-codex-auth",
                "--router-root",
                path_to_str(&router_root),
                "--label",
                "primary",
                "--auth-json",
                path_to_str(&auth_json),
                "--allow-plaintext-file-secrets",
            ],
            CliContext::new(Vec::new()),
        );

        let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
            .unwrap_or_else(|| panic!("account should remain registered"));
        assert_eq!(account.status(), AccountStatus::Enabled);
        assert_eq!(account.active_credential_generation(), Some(2));
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 2));
        let bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
            secrets.read_secret(&bundle_key),
        )));
        assert_eq!(bundle.access_token().expose_secret(), "new-access-token");
        assert_eq!(
            bundle.refresh_token().map(SecretString::expose_secret),
            Some("new-refresh-token")
        );
        for route_band in [
            "responses",
            "models",
            "memories_trace_summarize",
            "responses_compact",
            "code_review",
        ] {
            let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &account_id,
                route_band,
            ))
            .unwrap_or_else(|| panic!("{route_band} snapshot should remain as stale marker"));
            assert_eq!(snapshot.remaining_headroom(), 0);
            assert_eq!(snapshot.observed_unix_seconds(), 0);
            assert!(snapshot.stale_penalty());
        }
        assert!(output.stdout.contains("imported account: primary\n"));
        assert!(!output.stdout.contains("new-access-token"));
        assert!(!output.stdout.contains("new-refresh-token"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn quota_status_reads_sqlite_rows_without_provider_io() {
        let test_root = TestRoot::new("quota-status");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let primary_account = AccountRecord::new(
            account_id("acct_primary"),
            "primary",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &primary_account,
        ));
        must_ok(QuotaSnapshotRepository::upsert_snapshot(
            &state,
            &PersistedQuotaSnapshot::new(
                account_id("acct_primary"),
                QuotaSnapshotSource::MockEndpoint,
            )
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 72)
            .with_reset_unix_seconds(2_000)
            .with_stale_penalty(false),
        ));
        must_ok(QuotaSnapshotRepository::upsert_snapshot(
            &state,
            &PersistedQuotaSnapshot::new(
                account_id("acct_primary"),
                QuotaSnapshotSource::MockEndpoint,
            )
            .with_observed_unix_seconds(1_005)
            .with_route_band("models", 44)
            .with_reset_unix_seconds(3_000),
        ));

        let output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(&router_root),
            ],
            CliContext::new(Vec::new()),
        );

        assert!(output.stdout.contains("account"));
        assert!(output.stdout.contains("primary"));
        assert!(output.stdout.contains("72%"));
        assert!(output.stdout.contains("needs refresh"));
        assert!(!output.stdout.contains("acct_primary"));
        assert!(output.stdout.contains("responses"));
        assert!(output.stdout.contains("why"));
        assert!(!output.stdout.contains("models"));
        assert!(!output.stdout.contains("44%"));
        assert!(!output.stdout.contains("pp"));
        assert!(!output.stdout.contains("bottleneck"));
        assert!(!output.stdout.contains("0% left"));
        assert!(!output.stdout.contains("access-token"));
        assert!(!output.stdout.contains("refresh-token"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn quota_status_snapshot_rows_show_unknown_pace_until_window_metadata_exists() {
        let test_root = TestRoot::new("quota-status-snapshot-unknown-pace");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let primary_account = AccountRecord::new(
            account_id("acct_snapshot_pace"),
            "snapshot",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &primary_account,
        ));
        must_ok(QuotaSnapshotRepository::upsert_snapshot(
            &state,
            &PersistedQuotaSnapshot::new(
                account_id("acct_snapshot_pace"),
                QuotaSnapshotSource::MockEndpoint,
            )
            .with_observed_unix_seconds(9_900)
            .with_route_band("responses", 75)
            .with_reset_unix_seconds(20_000)
            .with_stale_penalty(false),
        ));

        let output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(&router_root),
                "--format",
                "plain",
                "--now-unix-seconds",
                "10000",
            ],
            CliContext::new(Vec::new()),
        );

        let lines = output.stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "account\tstatus\t5h\tweekly\tresets available\trouting\tnext use"
        );
        assert_eq!(
            lines[1],
            "snapshot\tenabled\t########-- 75% left resets in 2h 46m; needs refresh\t---------- no data needs refresh\t-\tfallback: needs refresh limiting window: 5h 75% left\tfallback"
        );
        assert_eq!(
            lines[2],
            "responses route\tnext: snapshot\twhy: fallback: needs refresh limiting window: 5h 75% left"
        );
        assert_eq!(lines.len(), 3);
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn quota_status_shows_two_user_quota_windows_per_account() {
        let test_root = TestRoot::new("quota-status-all-limits");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let primary_account = AccountRecord::new(
            account_id("acct_primary"),
            "primary",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &primary_account,
        ));
        let five_hour_window = PersistedSelectorQuotaWindow::new(
            account_id("acct_primary"),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(25)
        .with_reset_unix_seconds(20_000)
        .with_effective(true)
        .with_observed_unix_seconds(10_000);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id("acct_primary"),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(80)
        .with_reset_unix_seconds(614_800)
        .with_observed_unix_seconds(10_000);
        must_ok(
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &state,
                primary_account.account_id(),
                "responses",
                &[five_hour_window, weekly_window],
                10_000,
                20_000,
            ),
        );
        must_ok(QuotaSnapshotRepository::upsert_snapshot(
            &state,
            &PersistedQuotaSnapshot::new(
                account_id("acct_primary"),
                QuotaSnapshotSource::MockEndpoint,
            )
            .with_observed_unix_seconds(10_000)
            .with_route_band("responses", 25)
            .with_reset_unix_seconds(20_000)
            .with_reset_credits_available(1)
            .with_stale_penalty(false),
        ));

        let output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(&router_root),
                "--format",
                "plain",
                "--all-limits",
                "--now-unix-seconds",
                "11000",
            ],
            CliContext::new(Vec::new()),
        );

        let lines = output.stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "account\tstatus\t5h\tweekly\tresets available\trouting\tnext use"
        );
        assert_eq!(
            lines[1],
            "primary\tenabled\t###------- 25% left resets in 2h 30m\t########-- 80% left resets in 6d 23h\t1 available\tpreferred next: safest quota limiting window: 5h 25% left\tpreferred"
        );
        assert_eq!(
            lines[2],
            "responses route\tnext: primary\twhy: preferred next: safest quota limiting window: 5h 25% left"
        );
        assert_eq!(lines.len(), 3);
        assert!(!output.stdout.contains("acct_primary"));
        assert!(!output.stdout.contains("pp"));
        assert!(!output.stdout.contains("bottleneck"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn quota_status_json_exposes_burndown_debug_fields_without_secret_material() {
        let test_root = TestRoot::new("quota-status-json");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let primary_account = AccountRecord::new(
            account_id("acct_primary"),
            "primary",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &primary_account,
        ));
        let five_hour_window = PersistedSelectorQuotaWindow::new(
            account_id("acct_primary"),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(25)
        .with_reset_unix_seconds(20_000)
        .with_effective(true)
        .with_observed_unix_seconds(10_000);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id("acct_primary"),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(80)
        .with_reset_unix_seconds(614_800)
        .with_observed_unix_seconds(10_000);
        must_ok(
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &state,
                primary_account.account_id(),
                "responses",
                &[five_hour_window, weekly_window],
                10_000,
                20_000,
            ),
        );
        must_ok(QuotaSnapshotRepository::upsert_snapshot(
            &state,
            &PersistedQuotaSnapshot::new(
                account_id("acct_primary"),
                QuotaSnapshotSource::MockEndpoint,
            )
            .with_observed_unix_seconds(10_000)
            .with_route_band("responses", 25)
            .with_reset_unix_seconds(20_000)
            .with_reset_credits_available(1)
            .with_stale_penalty(false),
        ));

        let output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(&router_root),
                "--format",
                "json",
                "--now-unix-seconds",
                "11000",
            ],
            CliContext::new(Vec::new()),
        );

        let parsed: serde_json::Value = must_ok(serde_json::from_str(&output.stdout));
        assert_eq!(parsed["route_result"], "ok");
        assert_eq!(parsed["route_band"], "responses");
        assert_eq!(parsed["selected_pool"], "usable");
        assert_eq!(parsed["selected_pool_reason"], "usable_available");
        assert_eq!(parsed["preferred_next_account_id"], "acct_primary");
        assert_eq!(
            parsed["weighted_candidates"][0]["account_id"],
            "acct_primary"
        );
        assert_eq!(parsed["weighted_candidates"][0]["routing_weight"], 1);
        assert_eq!(parsed["accounts"][0]["account_id"], "acct_primary");
        assert_eq!(parsed["accounts"][0]["safe_account_label"], "primary");
        assert_eq!(parsed["accounts"][0]["availability"], "usable");
        assert_eq!(parsed["accounts"][0]["freshness"], "fresh");
        assert_eq!(
            parsed["accounts"][0]["routing_reason"],
            "preferred_highest_weight"
        );
        assert_eq!(parsed["accounts"][0]["preferred_next"], true);
        assert_eq!(parsed["accounts"][0]["reset_credits_available"], 1);
        assert_eq!(parsed["accounts"][0]["next_use"], "preferred");
        assert_eq!(parsed["accounts"][0]["short_pressure"], 25);
        assert_eq!(parsed["accounts"][0]["long_pressure"], 20);
        assert_eq!(parsed["accounts"][0]["limiting_window"], "5h");
        assert_eq!(
            parsed["accounts"][0]["window_slots"]["5h"]["evidence_state"],
            "known"
        );
        assert_eq!(
            parsed["accounts"][0]["window_slots"]["5h"]["remaining_headroom"],
            25
        );
        assert_eq!(
            parsed["accounts"][0]["window_slots"]["weekly"]["remaining_headroom"],
            80
        );
        assert_eq!(
            parsed["accounts"][0]["windows"][0]["remaining_headroom"],
            25
        );
        assert_eq!(
            parsed["accounts"][0]["windows"][1]["remaining_headroom"],
            80
        );
        assert!(!output.stdout.contains("access-token"));
        assert!(!output.stdout.contains("refresh-token"));
        assert!(!output.stdout.contains("authorization"));
        assert!(!output.stdout.contains("bottleneck"));
        assert!(!output.stdout.contains("pp"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn quota_refresh_rejects_non_provider_base_url_before_token_egress() {
        let test_root = TestRoot::new("quota-refresh-disallowed");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account_id = account_id("acct_refresh_reject");
        let account = AccountRecord::new(account_id.clone(), "reject", AccountStatus::Enabled);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let access_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(
            &access_key,
            &SecretString::new("quota-refresh-token-canary"),
        ));

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = match run_with_io(
            vec![
                "codex-router".into(),
                "quota".into(),
                "refresh".into(),
                "--router-root".into(),
                router_root.as_os_str().to_owned(),
                "--base-url".into(),
                "http://127.0.0.1:9".into(),
            ],
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("disallowed quota base URL must fail before token egress"),
            Err(error) => error,
        };
        let rendered_error = error.to_string();

        assert!(rendered_error.contains("quota refresh base URL is not allowed"));
        assert!(!rendered_error.contains("quota-refresh-token-canary"));
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn quota_refresh_resolver_refreshes_expired_access_token_before_provider_egress() {
        let test_root = TestRoot::new("quota-refresh-resolver");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account_id = account_id("acct_quota_refresh");
        let account = AccountRecord::new(account_id.clone(), "refresh", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let expired_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &expired_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "expired-quota-access-token",
                        Some("quota-refresh-token".to_owned()),
                    )
                    .with_expires_unix_seconds(900)
                    .to_secret_string(),
                ),
            ),
        );
        let refresh_client = RecordingRefreshClient::new(
            "acct_quota_refresh",
            "quota-refresh-token",
            AccountCredentialBundle::imported_codex_auth(
                "refreshed-quota-access-token",
                Some("refreshed-quota-refresh-token".to_owned()),
            )
            .with_expires_unix_seconds(2_000),
        );
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, refresh_client.clone(), 1_000);
        let provider = RecordingQuotaRefreshProvider::new(33);
        let mut stdout = Vec::new();

        must_ok(refresh_quota_with_dependencies(
            &mut stdout,
            router_root.clone(),
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_100,
        ));

        assert_eq!(refresh_client.calls(), 1);
        let recorded = provider.take_recorded();
        assert_eq!(
            recorded,
            vec![
                (
                    "acct_quota_refresh".to_owned(),
                    "refresh".to_owned(),
                    "responses".to_owned(),
                    "https://chatgpt.com/backend-api".to_owned(),
                    "refreshed-quota-access-token".to_owned(),
                ),
                (
                    "acct_quota_refresh".to_owned(),
                    "refresh".to_owned(),
                    "models".to_owned(),
                    "https://chatgpt.com/backend-api".to_owned(),
                    "refreshed-quota-access-token".to_owned(),
                )
            ]
        );
        let refreshed_snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &account_id,
            "responses",
        ))
        .unwrap_or_else(|| panic!("quota snapshot should be persisted"));
        assert_eq!(refreshed_snapshot.remaining_headroom(), 33);
        assert_eq!(
            refreshed_snapshot.source(),
            QuotaSnapshotSource::OpenAiEndpoint
        );
        assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");
    }

    #[test]
    fn quota_refresh_store_paths_persist_to_explicit_state_db_and_secret_root() {
        let test_root = TestRoot::new("quota-refresh-store-paths");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("custom-state.sqlite");
        let secret_root = test_root.path().join("custom-secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let account_id = account_id("acct_quota_store_paths");
        let account = AccountRecord::new(account_id.clone(), "store-paths", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "store-path-access-token",
                        Some("store-path-refresh-token".to_owned()),
                    )
                    .to_secret_string(),
                ),
            ),
        );
        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &state_path,
            &secret_root,
            1_000,
            NoopCredentialRefreshClient,
        ));
        let provider = RecordingQuotaRefreshProvider::new(61);
        let mut stdout = Vec::new();

        must_ok(refresh_quota_store_paths_with_dependencies(
            &mut stdout,
            &state_path,
            &secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_200,
        ));

        let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &account_id,
            "responses",
        ))
        .unwrap_or_else(|| panic!("explicit state-db snapshot should be persisted"));
        assert_eq!(snapshot.remaining_headroom(), 61);
        assert_eq!(snapshot.observed_unix_seconds(), 1_200);
        assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");
    }

    #[test]
    fn background_quota_refresh_worker_runs_immediate_cycle_without_waiting_for_interval() {
        let test_root = TestRoot::new("background-quota-refresh-immediate");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let account_id = account_id("acct_background_refresh");
        let account = AccountRecord::new(account_id.clone(), "background", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "background-access-token",
                        Some("background-refresh-token".to_owned()),
                    )
                    .to_secret_string(),
                ),
            ),
        );
        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &state_path,
            &secret_root,
            1_000,
            NoopCredentialRefreshClient,
        ));
        let (refresh_sender, refresh_receiver) = mpsc::channel();
        let provider = SignalingQuotaRefreshProvider::new(58, refresh_sender);

        let worker = start_background_quota_refresh_worker_with_dependencies(
            state_path,
            secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            resolver,
            provider,
            Duration::from_secs(3_600),
        );

        let observed_route_band = match refresh_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(route_band) => route_band,
            Err(error) => panic!("background refresh should run immediately: {error}"),
        };
        assert_eq!(observed_route_band, "responses");
        drop(worker);
        let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &account_id,
            "responses",
        ))
        .unwrap_or_else(|| panic!("background refresh should persist snapshot"));
        assert_eq!(snapshot.remaining_headroom(), 58);
        assert!(snapshot.observed_unix_seconds() > 0);
    }

    #[test]
    fn background_quota_refresh_worker_start_does_not_wait_for_slow_provider() {
        let test_root = TestRoot::new("background-quota-refresh-slow-provider");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let account_id = account_id("acct_background_refresh_slow_provider");
        let account = AccountRecord::new(account_id.clone(), "background", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "background-slow-access-token",
                        Some("background-slow-refresh-token".to_owned()),
                    )
                    .to_secret_string(),
                ),
            ),
        );
        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &state_path,
            &secret_root,
            1_000,
            NoopCredentialRefreshClient,
        ));
        let provider = SlowQuotaRefreshProvider::new(Duration::from_millis(500), 72);

        let start = Instant::now();
        let worker = start_background_quota_refresh_worker_with_dependencies(
            state_path,
            secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            resolver,
            provider,
            Duration::from_secs(0),
        );
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(250),
            "background worker startup waited for provider: {elapsed:?}"
        );
        drop(worker);
    }

    #[test]
    fn background_quota_refresh_worker_uses_fresh_time_for_each_cycle() {
        let test_root = TestRoot::new("background-quota-refresh-fresh-time");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let account_id = account_id("acct_background_refresh_fresh_time");
        let account = AccountRecord::new(account_id.clone(), "background", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "background-fresh-access-token",
                        Some("background-fresh-refresh-token".to_owned()),
                    )
                    .to_secret_string(),
                ),
            ),
        );
        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &state_path,
            &secret_root,
            1_000,
            NoopCredentialRefreshClient,
        ));
        let (refresh_sender, refresh_receiver) = mpsc::channel();
        let provider = SignalingQuotaRefreshProvider::new(64, refresh_sender);
        let clock = Arc::new(AtomicU64::new(1_300));
        let worker_clock = Arc::clone(&clock);

        let worker = start_background_quota_refresh_worker_with_clock(
            state_path,
            secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            resolver,
            provider,
            move || worker_clock.fetch_add(5, Ordering::SeqCst),
            Duration::from_millis(1),
        );

        for _call_index in 0..4 {
            if let Err(error) = refresh_receiver.recv_timeout(Duration::from_secs(2)) {
                panic!("background refresh should run multiple cycles: {error}");
            }
        }
        drop(worker);
        let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &account_id,
            "responses",
        ))
        .unwrap_or_else(|| panic!("background refresh should persist snapshot"));
        assert_eq!(snapshot.remaining_headroom(), 64);
        assert!(snapshot.observed_unix_seconds() > 1_300);
    }

    #[test]
    fn background_quota_refresh_worker_reports_refresh_failures() {
        let test_root = TestRoot::new("background-quota-refresh-diagnostics");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let account_id = account_id("acct_background_refresh_diagnostics");
        let account = AccountRecord::new(account_id.clone(), "background", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "background-diagnostic-token-canary",
                        Some("background-diagnostic-refresh-token".to_owned()),
                    )
                    .to_secret_string(),
                ),
            ),
        );
        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &state_path,
            &secret_root,
            1_000,
            NoopCredentialRefreshClient,
        ));
        let provider = AccountFailingQuotaRefreshProvider::new("background", 0);
        let (diagnostic_sender, diagnostic_receiver) = mpsc::channel();

        let worker = start_background_quota_refresh_worker_with_reporter(
            state_path,
            secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            resolver,
            provider,
            BackgroundQuotaRefreshRuntime::new(
                || 1_300,
                move |diagnostic| {
                    if let Err(error) = diagnostic_sender.send(diagnostic) {
                        panic!("background diagnostic should send: {error}");
                    }
                },
                Duration::from_secs(0),
            ),
        );

        let first_diagnostic = match diagnostic_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(diagnostic) => diagnostic,
            Err(error) => panic!("background refresh should report route failures: {error}"),
        };
        let second_diagnostic = match diagnostic_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(diagnostic) => diagnostic,
            Err(error) => panic!("background refresh should report command failure: {error}"),
        };
        drop(worker);
        let diagnostics = format!("{first_diagnostic}\n{second_diagnostic}");
        assert!(diagnostics.contains(
            "refresh failed: account=background route_band=responses error=quota refresh provider returned HTTP 429"
        ));
        assert!(diagnostics.contains("failed: 2"));
        assert!(diagnostics.contains(
            "background quota refresh failed: quota refresh provider response was unusable: quota refresh failed for all eligible route bands"
        ));
        assert!(!diagnostics.contains("background-diagnostic-token-canary"));
    }

    #[test]
    fn cli_credential_resolver_refreshes_expired_bundle_through_runtime_wrapper() {
        let test_root = TestRoot::new("cli-runtime-resolver-refresh");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account_id = account_id("acct_cli_runtime_refresh");
        let account = AccountRecord::new(account_id.clone(), "runtime", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets_root = router_root.join("secrets");
        let secrets = must_ok(FileSecretStore::open(&secrets_root));
        let expired_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &expired_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "expired-cli-runtime-access-token",
                        Some("cli-runtime-refresh-token".to_owned()),
                    )
                    .with_expires_unix_seconds(900)
                    .to_secret_string(),
                ),
            ),
        );
        let refresh_client = RecordingRefreshClient::new(
            "acct_cli_runtime_refresh",
            "cli-runtime-refresh-token",
            AccountCredentialBundle::imported_codex_auth(
                "refreshed-cli-runtime-access-token",
                Some("refreshed-cli-runtime-refresh-token".to_owned()),
            )
            .with_expires_unix_seconds(2_000),
        );
        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &router_root.join("state.sqlite"),
            &secrets_root,
            1_000,
            refresh_client.clone(),
        ));

        let resolved = must_ok(resolver.resolve_provider_credentials(&account_id));

        assert_eq!(
            resolved.access_token().expose_secret(),
            "refreshed-cli-runtime-access-token"
        );
        assert_eq!(refresh_client.calls(), 1);
        let loaded_account = must_ok(AccountStateRepository::load_account(&state, &account_id))
            .unwrap_or_else(|| panic!("account should remain registered"));
        assert_eq!(loaded_account.active_credential_generation(), Some(2));
    }

    #[test]
    fn quota_refresh_missing_refresh_token_fails_closed_before_provider_egress() {
        let test_root = TestRoot::new("quota-refresh-missing-refresh");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account_id = account_id("acct_quota_missing_refresh");
        let account = AccountRecord::new(account_id.clone(), "missing", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let expired_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &expired_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "expired-quota-access-token-canary",
                        None,
                    )
                    .with_expires_unix_seconds(900)
                    .to_secret_string(),
                ),
            ),
        );
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let provider = RecordingQuotaRefreshProvider::new(44);
        let mut stdout = Vec::new();

        let error = match refresh_quota_with_dependencies(
            &mut stdout,
            router_root,
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_100,
        ) {
            Ok(()) => panic!("missing refresh token should fail before provider egress"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "quota refresh provider response was unusable: quota refresh failed for all eligible route bands"
        );
        assert!(provider.take_recorded().is_empty());
        let rendered_stdout = must_ok(String::from_utf8(stdout));
        assert!(rendered_stdout.contains(&format!(
            "refresh failed: account=missing route_band=* error={}\n",
            CredentialResolverError::RefreshUnavailable
        )));
        assert!(rendered_stdout.contains("refreshed: 0\n"));
        assert!(rendered_stdout.contains("failed: 2\n"));
        assert!(!rendered_stdout.contains("expired-quota-access-token-canary"));
    }

    #[test]
    fn quota_refresh_continues_after_one_account_provider_failure() {
        let test_root = TestRoot::new("quota-refresh-partial-provider-failure");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let failing_account_id = account_id("acct_quota_provider_failing");
        let healthy_account_id = account_id("acct_quota_provider_healthy");
        let failing_account = AccountRecord::new(
            failing_account_id.clone(),
            "failing",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        let healthy_account = AccountRecord::new(
            healthy_account_id.clone(),
            "healthy",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &failing_account,
        ));
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &healthy_account,
        ));
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        for (account_id, access_token) in [
            (&failing_account_id, "failing-provider-token-canary"),
            (&healthy_account_id, "healthy-provider-token-canary"),
        ] {
            let bundle_key = must_ok(account_credential_bundle_key(account_id, 1));
            must_ok(
                secrets.write_secret(
                    &bundle_key,
                    &must_ok(
                        AccountCredentialBundle::imported_codex_auth(
                            access_token,
                            Some(format!("{access_token}-refresh")),
                        )
                        .with_expires_unix_seconds(2_000)
                        .to_secret_string(),
                    ),
                ),
            );
        }
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let provider = AccountFailingQuotaRefreshProvider::new("failing", 69);
        let mut stdout = Vec::new();

        must_ok(refresh_quota_with_dependencies(
            &mut stdout,
            router_root.clone(),
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_100,
        ));

        assert!(
            must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &failing_account_id,
                "responses",
            ))
            .is_none()
        );
        let healthy_snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &healthy_account_id,
            "responses",
        ))
        .unwrap_or_else(|| panic!("healthy account quota snapshot should be persisted"));
        assert_eq!(healthy_snapshot.remaining_headroom(), 69);
        let rendered_stdout = must_ok(String::from_utf8(stdout));
        assert!(rendered_stdout.contains(
            "refresh failed: account=failing route_band=responses error=quota refresh provider returned HTTP 429\n"
        ));
        assert!(rendered_stdout.contains(
            "refresh failed: account=failing route_band=models error=quota refresh provider returned HTTP 429\n"
        ));
        assert!(rendered_stdout.contains("refreshed: 2\n"));
        assert!(rendered_stdout.contains("failed: 2\n"));
        assert!(!rendered_stdout.contains("failing-provider-token-canary"));
        assert!(!rendered_stdout.contains("healthy-provider-token-canary"));
    }

    #[test]
    fn quota_refresh_writes_selector_windows_for_runtime_selection() {
        let test_root = TestRoot::new("quota-refresh-selector-windows");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account_id = account_id("acct_quota_selector");
        let account = AccountRecord::new(account_id.clone(), "selector", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "quota-selector-access-token",
                        Some("quota-selector-refresh-token".to_owned()),
                    )
                    .with_expires_unix_seconds(2_000)
                    .to_secret_string(),
                ),
            ),
        );
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let provider = RecordingQuotaRefreshProvider::new(37);
        let mut stdout = Vec::new();

        must_ok(refresh_quota_with_dependencies(
            &mut stdout,
            router_root.clone(),
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_100,
        ));

        let selector_inputs = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
            &state,
            "responses",
            1_100,
        ));
        assert_eq!(selector_inputs.len(), 1);
        let windows = selector_inputs[0].windows();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].limit_window_seconds(), 18_000);
        assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Eligible);
        assert_eq!(windows[0].remaining_headroom(), 37);
        assert_eq!(windows[0].reset_unix_seconds(), Some(20_000));
        assert_eq!(windows[0].observed_unix_seconds(), 1_100);
        assert!(windows[0].effective());
        assert_eq!(windows[1].limit_window_seconds(), 604_800);
        assert_eq!(windows[1].status(), SelectorQuotaWindowStatus::Eligible);
        assert_eq!(windows[1].remaining_headroom(), 50);
        assert_eq!(windows[1].reset_unix_seconds(), Some(614_800));
        assert_eq!(windows[1].observed_unix_seconds(), 1_100);
        assert!(!windows[1].effective());
        assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");

        let status_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(&router_root),
                "--format",
                "plain",
                "--now-unix-seconds",
                "1100",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(status_output.stdout.contains("selector"));
        assert!(status_output.stdout.contains("next"));
        assert!(!status_output.stdout.contains("needs probe"));
        assert!(status_output.stderr.is_empty());
    }

    #[test]
    fn quota_refresh_partial_window_response_is_unknown_fallback() {
        let test_root = TestRoot::new("quota-refresh-partial-window");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account_id = account_id("acct_quota_partial");
        let account = AccountRecord::new(account_id.clone(), "partial", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "partial-quota-token",
                        Some("partial-quota-refresh-token".to_owned()),
                    )
                    .with_expires_unix_seconds(2_000)
                    .to_secret_string(),
                ),
            ),
        );
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let provider = StaticQuotaRefreshProvider::new(vec![QuotaRefreshProviderWindow {
            limit_window_seconds: 18_000,
            remaining_headroom: 80,
            reset_unix_seconds: Some(20_000),
            effective: true,
        }]);
        let mut stdout = Vec::new();

        must_ok(refresh_quota_with_dependencies(
            &mut stdout,
            router_root.clone(),
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_100,
        ));

        let selector_inputs = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
            &state,
            "responses",
            1_100,
        ));
        assert_eq!(selector_inputs[0].windows().len(), 1);
        let status_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(&router_root),
                "--format",
                "plain",
                "--now-unix-seconds",
                "1100",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(status_output.stdout.contains("partial"));
        assert!(status_output.stdout.contains("needs refresh"));
        assert!(
            status_output
                .stdout
                .lines()
                .any(|line| line.ends_with("\tfallback"))
        );
        assert!(status_output.stdout.contains("fallback: needs refresh"));
        assert!(
            status_output
                .stdout
                .contains("responses route\tnext: partial")
        );
        assert!(!status_output.stdout.contains("partial-quota-token"));
        assert!(status_output.stderr.is_empty());
    }

    #[test]
    fn quota_refresh_missing_reset_response_is_unknown_fallback() {
        let test_root = TestRoot::new("quota-refresh-missing-reset");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account_id = account_id("acct_quota_missing_reset");
        let account =
            AccountRecord::new(account_id.clone(), "missing-reset", AccountStatus::Enabled)
                .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "missing-reset-quota-token",
                        Some("missing-reset-quota-refresh-token".to_owned()),
                    )
                    .with_expires_unix_seconds(2_000)
                    .to_secret_string(),
                ),
            ),
        );
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let provider = StaticQuotaRefreshProvider::new(vec![
            QuotaRefreshProviderWindow {
                limit_window_seconds: 18_000,
                remaining_headroom: 80,
                reset_unix_seconds: Some(20_000),
                effective: true,
            },
            QuotaRefreshProviderWindow {
                limit_window_seconds: 604_800,
                remaining_headroom: 90,
                reset_unix_seconds: None,
                effective: false,
            },
        ]);
        let mut stdout = Vec::new();

        must_ok(refresh_quota_with_dependencies(
            &mut stdout,
            router_root.clone(),
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            1_100,
        ));

        let status_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(&router_root),
                "--format",
                "plain",
                "--now-unix-seconds",
                "1100",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(status_output.stdout.contains("missing-reset"));
        assert!(status_output.stdout.contains("needs refresh"));
        assert!(
            status_output
                .stdout
                .lines()
                .any(|line| line.ends_with("\tfallback"))
        );
        assert!(status_output.stdout.contains("fallback: needs refresh"));
        assert!(
            status_output
                .stdout
                .contains("responses route\tnext: missing-reset")
        );
        assert!(!status_output.stdout.contains("missing-reset-quota-token"));
        assert!(status_output.stderr.is_empty());
    }

    #[test]
    fn quota_refresh_http_provider_fetches_usage_and_persists_sqlite_state() {
        let test_root = TestRoot::new("quota-refresh-http-provider");
        must_ok(fs::create_dir(test_root.path()));
        let router_root = test_root.path().join("router");
        must_ok(fs::create_dir_all(&router_root));
        let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
        let account_id = account_id("acct_quota_http");
        let account = AccountRecord::new(account_id.clone(), "quota-http", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
        let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "quota-http-access-token",
                        Some("quota-http-refresh-token".to_owned()),
                    )
                    .with_expires_unix_seconds(2_000)
                    .to_secret_string(),
                ),
            ),
        );

        let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(listener.local_addr());
        let server_thread = thread::spawn(move || {
            for _request_index in 0..2 {
                let (mut stream, _peer_address) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) => panic!("quota mock should accept: {error}"),
                };
                if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
                    panic!("quota mock should set read timeout: {error}");
                }
                let mut buffer = [0_u8; 4096];
                let bytes_read = match stream.read(&mut buffer) {
                    Ok(bytes_read) => bytes_read,
                    Err(error) => panic!("quota mock should read request: {error}"),
                };
                let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                assert!(request.starts_with("GET /api/codex/usage HTTP/1.1\r\n"));
                assert!(request.contains("authorization: Bearer quota-http-access-token\r\n"));
                let body = r#"{"rate_limit":{"primary_window":{"used_percent":25,"reset_at":2000,"limit_window_seconds":18000},"secondary_window":{"used_percent":80,"reset_at":9000,"limit_window_seconds":604800}},"reset_credits":{"available":1}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                if let Err(error) = stream.write_all(response.as_bytes()) {
                    panic!("quota mock should write response: {error}");
                }
            }
        });
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let provider = must_ok(HttpQuotaRefreshProvider::new());
        let mut stdout = Vec::new();

        must_ok(refresh_quota_with_dependencies(
            &mut stdout,
            router_root,
            format!("http://{address}"),
            &resolver,
            &provider,
            1_100,
        ));

        for route_band in ["responses", "models"] {
            let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &account_id,
                route_band,
            ))
            .unwrap_or_else(|| panic!("{route_band} quota snapshot should be persisted"));
            assert_eq!(snapshot.remaining_headroom(), 75);
            assert_eq!(snapshot.reset_unix_seconds(), Some(2_000));
            assert_eq!(snapshot.reset_credits_available(), Some(1));
            assert_eq!(snapshot.source(), QuotaSnapshotSource::OpenAiEndpoint);
        }
        let selector_inputs = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
            &state,
            "responses",
            1_100,
        ));
        assert_eq!(selector_inputs.len(), 1);
        let windows = selector_inputs[0].windows();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].limit_window_seconds(), 18_000);
        assert_eq!(windows[0].remaining_headroom(), 75);
        assert_eq!(windows[0].reset_unix_seconds(), Some(2_000));
        assert!(windows[0].effective());
        assert_eq!(windows[1].limit_window_seconds(), 604_800);
        assert_eq!(windows[1].remaining_headroom(), 20);
        assert_eq!(windows[1].reset_unix_seconds(), Some(9_000));
        assert!(!windows[1].effective());
        assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");

        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }
    }

    #[test]
    fn quota_refresh_http_provider_times_out_hanging_usage_endpoint() {
        let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(listener.local_addr());
        let server_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept: {error}"),
            };
            let mut buffer = [0_u8; 1024];
            let _bytes_read = stream.read(&mut buffer);
            thread::sleep(Duration::from_millis(200));
        });
        let provider = must_ok(HttpQuotaRefreshProvider::new_with_timeout(
            Duration::from_millis(10),
        ));

        let error = match provider.fetch_quota(QuotaRefreshProviderRequest::new(
            account_id("acct_timeout"),
            "timeout",
            "responses",
            format!("http://{address}"),
            SecretString::new("timeout-token-canary"),
        )) {
            Ok(response) => panic!("hanging quota endpoint should time out: {response:?}"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("quota refresh request failed"));
        assert!(!error.to_string().contains("timeout-token-canary"));
        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }
    }

    #[test]
    fn live_quota_command_rejects_api_key_auth_without_printing_key() {
        let test_root = TestRoot::new("live-quota-api-key");
        must_ok(fs::create_dir(test_root.path()));
        let auth_json = test_root.path().join("auth.json");
        must_ok(fs::write(
            &auth_json,
            r#"{"auth_mode":"api-key","OPENAI_API_KEY":"sk-local-secret-canary"}"#,
        ));

        let output = run_cli(
            [
                "codex-router",
                "live",
                "quota",
                "--auth-json",
                path_to_str(&auth_json),
                "--profile-label",
                "api-key-profile",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(output.stdout.contains("profile: api-key-profile\n"));
        assert!(output.stdout.contains("status: error\n"));
        assert!(
            output
                .stdout
                .contains("error: api_key_auth_not_quota_compatible\n")
        );
        assert!(!output.stdout.contains("sk-local-secret-canary"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn live_quota_profiles_root_fetches_oauth_usage_without_printing_token() {
        let test_root = TestRoot::new("live-quota-profiles");
        must_ok(fs::create_dir(test_root.path()));
        let profiles_root = test_root.path().join("profiles");
        let profile_root = profiles_root.join("main");
        must_ok(fs::create_dir_all(&profile_root));
        let auth_json = profile_root.join("auth.json");
        must_ok(fs::write(
            &auth_json,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"oauth-secret-canary"}}"#,
        ));
        let login_root = profiles_root.join(".login-ephemeral");
        must_ok(fs::create_dir_all(&login_root));
        must_ok(fs::write(
            login_root.join("auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"ignored-token-canary"}}"#,
        ));

        let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(listener.local_addr());
        let server_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept: {error}"),
            };
            if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
                panic!("quota mock should set read timeout: {error}");
            }
            let mut buffer = [0_u8; 4096];
            let bytes_read = match stream.read(&mut buffer) {
                Ok(bytes_read) => bytes_read,
                Err(error) => panic!("quota mock should read request: {error}"),
            };
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            assert!(request.starts_with("GET /api/codex/usage HTTP/1.1\r\n"));
            assert!(request.contains("authorization: Bearer oauth-secret-canary\r\n"));
            let body = r#"{"rate_limit":{"primary_window":{"used_percent":25,"reset_at":2000,"limit_window_seconds":18000},"secondary_window":{"used_percent":80,"reset_at":9000,"limit_window_seconds":604800}},"additional_rate_limits":[{}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                panic!("quota mock should write response: {error}");
            }
        });
        let base_url = format!("http://{address}");

        let output = run_cli(
            [
                "codex-router",
                "live",
                "quota",
                "--profiles-root",
                path_to_str(&profiles_root),
                "--base-url",
                base_url.as_str(),
            ],
            CliContext::new(Vec::new()),
        );

        assert!(output.stdout.contains("profile: main\n"));
        assert!(output.stdout.contains("status: ok\n"));
        assert!(
            output
                .stdout
                .contains("rate_limit.primary: remaining_percent=75")
        );
        assert!(
            output
                .stdout
                .contains("rate_limit.secondary: remaining_percent=20")
        );
        assert!(output.stdout.contains("additional_rate_limit_count: 1\n"));
        assert!(!output.stdout.contains("oauth-secret-canary"));
        assert!(!output.stdout.contains("ignored-token-canary"));
        assert!(!output.stdout.contains(".login-ephemeral"));
        assert!(output.stderr.is_empty());

        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }
    }

    #[test]
    fn token_export_command_emits_current_router_root_token_assignment() {
        let test_root = TestRoot::new("token-export-command");
        let store = must_ok(FileSecretStore::open(test_root.path()));
        let service = LocalRouterTokenService::new(store);
        let record = must_ok(service.rotate_with_token("quote'and\nnewline"));

        let output = run_cli(
            [
                "codex-router",
                "token",
                "export",
                "--router-root",
                path_to_str(test_root.path()),
                "--shell",
                "posix",
            ],
            CliContext::new(Vec::new()),
        );

        assert_eq!(
            output.stdout,
            export_token_assignment(
                "CODEX_ROUTER_TOKEN",
                record.token().expose_secret(),
                Shell::Posix
            )
        );
        assert_eq!(output.stdout.matches("CODEX_ROUTER_TOKEN=").count(), 1);
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn token_init_and_rotate_commands_do_not_print_secret_and_update_export() {
        let test_root = TestRoot::new("token-init-rotate-command");

        let init_output = run_cli(
            [
                "codex-router",
                "token",
                "init",
                "--router-root",
                path_to_str(test_root.path()),
            ],
            CliContext::new(Vec::new()),
        );
        assert_eq!(init_output.stdout, "generation: 1\n");
        assert!(!init_output.stdout.contains("CODEX_ROUTER_TOKEN="));
        assert!(init_output.stderr.is_empty());

        let first_export = run_cli(
            [
                "codex-router",
                "token",
                "export",
                "--router-root",
                path_to_str(test_root.path()),
            ],
            CliContext::new(Vec::new()),
        );
        assert!(
            first_export
                .stdout
                .starts_with("export CODEX_ROUTER_TOKEN='")
        );

        let rotate_output = run_cli(
            [
                "codex-router",
                "token",
                "rotate",
                "--router-root",
                path_to_str(test_root.path()),
            ],
            CliContext::new(Vec::new()),
        );
        assert_eq!(rotate_output.stdout, "generation: 2\n");
        assert!(!rotate_output.stdout.contains("CODEX_ROUTER_TOKEN="));
        assert!(rotate_output.stderr.is_empty());

        let second_export = run_cli(
            [
                "codex-router",
                "token",
                "export",
                "--router-root",
                path_to_str(test_root.path()),
            ],
            CliContext::new(Vec::new()),
        );
        assert!(
            second_export
                .stdout
                .starts_with("export CODEX_ROUTER_TOKEN='")
        );
        assert_ne!(first_export.stdout, second_export.stdout);
    }

    #[test]
    fn token_export_command_defaults_to_home_router_secret_root() {
        let command = match CliCommand::parse([
            OsString::from("token"),
            OsString::from("export"),
            OsString::from("--shell"),
            OsString::from("posix"),
        ]) {
            Ok(CliCommand::Token(command)) => command,
            Ok(other) => panic!("token command should parse, got {other:?}"),
            Err(error) => panic!("token command should parse: {error}"),
        };

        let TokenCommand::Export { router_root, shell } = command else {
            panic!("token export command should parse");
        };
        assert_eq!(router_root, default_router_secret_root_for_test());
        assert_eq!(shell, Shell::Posix);
    }

    #[test]
    fn account_list_command_defaults_to_home_router_root() {
        let command = match CliCommand::parse([OsString::from("account"), OsString::from("list")]) {
            Ok(CliCommand::Account(command)) => command,
            Ok(other) => panic!("account command should parse, got {other:?}"),
            Err(error) => panic!("account command should parse: {error}"),
        };

        let AccountCommand::List { router_root } = command else {
            panic!("account list command should parse");
        };
        assert_eq!(router_root, default_router_root_for_test());
    }

    #[test]
    fn quota_status_command_defaults_to_home_router_root() {
        let command = match CliCommand::parse([
            OsString::from("quota"),
            OsString::from("status"),
            OsString::from("--now-unix-seconds"),
            OsString::from("0"),
        ]) {
            Ok(CliCommand::Quota(command)) => command,
            Ok(other) => panic!("quota command should parse, got {other:?}"),
            Err(error) => panic!("quota command should parse: {error}"),
        };

        let QuotaCommand::Status { router_root, .. } = command else {
            panic!("quota status command should parse");
        };
        assert_eq!(router_root, default_router_root_for_test());
    }

    #[test]
    fn serve_command_starts_runtime_and_forwards_one_loopback_request() {
        let test_root = TestRoot::new("serve-command");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let token_service = LocalRouterTokenService::new(secrets.clone());
        let local_token = must_ok(token_service.rotate_with_token("current-token"));
        let account_id = account_id("acct_cli_serve");
        let account = AccountRecord::new(account_id.clone(), "cli-serve", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 100);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        persist_effective_selector_window(&state, &account_id, "responses", 100);
        let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
        let upstream_credential_bundle = must_ok(
            AccountCredentialBundle::imported_codex_auth(
                "cli-upstream-token",
                Some("cli-upstream-refresh-token".to_owned()),
            )
            .to_secret_string(),
        );
        must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

        let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let upstream_address = must_ok(upstream_listener.local_addr());
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock upstream should accept: {error}"),
            };
            let mut request = String::new();
            if let Err(error) = stream.read_to_string(&mut request) {
                panic!("mock upstream should read request: {error}");
            }
            if let Err(error) = upstream_sender.send(request) {
                panic!("mock upstream request should record: {error}");
            }
            if let Err(error) =
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
            {
                panic!("mock upstream should write response: {error}");
            }
        });
        let router_port = reserve_loopback_port();
        let router_port_text = router_port.to_string();
        let upstream_base_url = format!("http://{upstream_address}/v1");
        let client_thread = thread::spawn(move || {
            send_loopback_request_with_retry(
                router_port,
                local_token.token().expose_secret(),
                br#"{"model":"gpt-5","serve":true}"#,
            )
        });

        let output = run_cli(
            [
                "codex-router",
                "serve",
                "--listen-host",
                "127.0.0.1",
                "--port",
                router_port_text.as_str(),
                "--state-db",
                path_to_str(&state_path),
                "--secret-root",
                path_to_str(&secret_root),
                "--upstream-base-url",
                upstream_base_url.as_str(),
                "--now-unix-seconds",
                "1030",
                "--max-snapshot-age-seconds",
                "60",
                "--disable-background-quota-refresh",
                "--max-connections",
                "1",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(
            output
                .stdout
                .contains(format!("listening: 127.0.0.1:{router_port}\n").as_str())
        );
        assert!(output.stderr.is_empty());
        let client_response = match client_thread.join() {
            Ok(response) => response,
            Err(error) => panic!("client thread panicked: {error:?}"),
        };
        assert!(client_response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(client_response.ends_with("\r\ndata: ok\n\n"));

        let upstream_request = match upstream_receiver.recv() {
            Ok(request) => request,
            Err(error) => panic!("mock upstream request should be recorded: {error}"),
        };
        assert!(upstream_request.starts_with("POST /v1/responses HTTP/1.1\r\n"));
        assert!(upstream_request.contains("authorization: Bearer cli-upstream-token\r\n"));
        assert!(!upstream_request.contains("X-Codex-Router-Token"));
        assert!(!upstream_request.contains("current-token"));

        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
    }

    #[test]
    fn serve_command_defaults_quota_clock_to_system_time() {
        let before_parse = test_current_unix_seconds();
        let command = match CliCommand::parse([
            OsString::from("serve"),
            OsString::from("--state-db"),
            OsString::from("/tmp/codex-router-state.sqlite"),
            OsString::from("--secret-root"),
            OsString::from("/tmp/codex-router-secrets"),
            OsString::from("--upstream-base-url"),
            OsString::from("http://127.0.0.1:1/v1"),
        ]) {
            Ok(CliCommand::Serve(command)) => command,
            Ok(other) => panic!("serve command should parse, got {other:?}"),
            Err(error) => panic!("serve command should parse: {error}"),
        };
        let after_parse = test_current_unix_seconds();

        assert!(command.now_unix_seconds >= before_parse);
        assert!(command.now_unix_seconds <= after_parse);
        assert_ne!(command.now_unix_seconds, 0);
    }

    #[test]
    fn serve_command_defaults_to_home_router_paths_and_provider_upstream() {
        let command = match CliCommand::parse([OsString::from("serve")]) {
            Ok(CliCommand::Serve(command)) => command,
            Ok(other) => panic!("serve command should parse, got {other:?}"),
            Err(error) => panic!("serve command should parse: {error}"),
        };

        let router_root = default_router_root_for_test();
        assert_eq!(command.state_db, router_root.join("state.sqlite"));
        assert_eq!(command.secret_root, router_root.join("secrets"));
        assert_eq!(
            command.upstream_base_url,
            codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL
        );
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn serve_command_dispatches_websocket_upgrade_through_runtime() {
        let test_root = TestRoot::new("serve-command-websocket");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let token_service = LocalRouterTokenService::new(secrets.clone());
        must_ok(token_service.rotate_with_token("current-token"));
        let account_id = account_id("acct_cli_ws");
        let account = AccountRecord::new(account_id.clone(), "cli-ws", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 100);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        persist_effective_selector_window(&state, &account_id, "responses", 100);
        let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
        let upstream_credential_bundle = must_ok(
            AccountCredentialBundle::imported_codex_auth(
                "cli-ws-upstream-token",
                Some("cli-ws-upstream-refresh-token".to_owned()),
            )
            .to_secret_string(),
        );
        must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

        let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let upstream_address = must_ok(upstream_listener.local_addr());
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock websocket upstream should accept: {error}"),
            };
            let mut websocket = match accept_hdr(stream, |request: &Request, response: Response| {
                let authorization = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<missing>")
                    .to_owned();
                let local_token = request
                    .headers()
                    .get("x-codex-router-token")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                if let Err(error) = upstream_sender.send((authorization, local_token)) {
                    panic!("mock websocket upstream headers should record: {error}");
                }
                Ok(response)
            }) {
                Ok(websocket) => websocket,
                Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
            };
            let first_frame = match websocket.read() {
                Ok(message) => message,
                Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
            };
            if let Err(error) = upstream_sender.send((first_frame.to_string(), None)) {
                panic!("mock websocket upstream first frame should record: {error}");
            }
            if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
                panic!("mock websocket upstream should send response: {error}");
            }
        });
        let router_port = reserve_loopback_port();
        let router_port_text = router_port.to_string();
        let upstream_base_url = format!("http://{upstream_address}/v1");
        let client_thread = thread::spawn(move || {
            let mut client = connect_websocket_with_retry(router_port, "current-token");
            let first_frame = r#"{"type":"response.create","cli":true}"#;
            if let Err(error) = client.send(Message::text(first_frame)) {
                panic!("local websocket client should send first frame: {error}");
            }
            match client.read() {
                Ok(message) => message.to_string(),
                Err(error) => panic!("local websocket client should read response: {error}"),
            }
        });

        let output = run_cli(
            [
                "codex-router",
                "serve",
                "--listen-host",
                "127.0.0.1",
                "--port",
                router_port_text.as_str(),
                "--state-db",
                path_to_str(&state_path),
                "--secret-root",
                path_to_str(&secret_root),
                "--upstream-base-url",
                upstream_base_url.as_str(),
                "--now-unix-seconds",
                "1030",
                "--max-snapshot-age-seconds",
                "60",
                "--disable-background-quota-refresh",
                "--max-connections",
                "1",
                "--max-websocket-upstream-messages",
                "1",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(
            output
                .stdout
                .contains(format!("listening: 127.0.0.1:{router_port}\n").as_str())
        );
        assert!(output.stderr.is_empty());
        let client_response = match client_thread.join() {
            Ok(response) => response,
            Err(error) => panic!("client thread panicked: {error:?}"),
        };
        assert_eq!(client_response, r#"{"type":"response.completed"}"#);
        let (authorization, local_token) = match upstream_receiver.recv() {
            Ok(recorded) => recorded,
            Err(error) => panic!("upstream handshake should be recorded: {error}"),
        };
        assert_eq!(authorization, "Bearer cli-ws-upstream-token");
        assert_eq!(local_token, None);
        let (recorded_first_frame, _) = match upstream_receiver.recv() {
            Ok(recorded) => recorded,
            Err(error) => panic!("upstream first frame should be recorded: {error}"),
        };
        assert_eq!(
            recorded_first_frame,
            r#"{"type":"response.create","cli":true}"#
        );

        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
        }
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn served_router_http_uses_persisted_quota_while_background_refresh_is_blocked() {
        let test_root = TestRoot::new("serve-background-refresh-blocked");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let token_service = LocalRouterTokenService::new(secrets.clone());
        let local_token = must_ok(token_service.rotate_with_token("current-token"));
        let account_id = account_id("acct_background_served");
        let account = AccountRecord::new(account_id.clone(), "served", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 91);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        persist_effective_selector_window(&state, &account_id, "responses", 91);
        let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
        let upstream_credential_bundle = must_ok(
            AccountCredentialBundle::imported_codex_auth(
                "served-upstream-token",
                Some("served-upstream-refresh-token".to_owned()),
            )
            .to_secret_string(),
        );
        must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

        let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let upstream_address = must_ok(upstream_listener.local_addr());
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock HTTP upstream should accept: {error}"),
            };
            if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
                panic!("mock HTTP upstream should set read timeout: {error}");
            }
            let request = read_http_request_with_body(&mut stream);
            if let Err(error) = upstream_sender.send(("http".to_owned(), request)) {
                panic!("mock HTTP upstream request should record: {error}");
            }
            if let Err(error) =
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
            {
                panic!("mock HTTP upstream should write response: {error}");
            }
            if let Err(error) = stream.flush() {
                panic!("mock HTTP upstream should flush response: {error}");
            }
        });

        let router_port = reserve_loopback_port();
        let bind_address = must_ok(LoopbackBindAddress::new("127.0.0.1", router_port));
        let upstream_endpoint = must_ok(UpstreamEndpoint::new(format!(
            "http://{upstream_address}/v1"
        )));
        let runtime_config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            upstream_endpoint,
            state_path.clone(),
            secret_root.clone(),
            local_token.clone(),
        )
        .with_quota_clock(1_030, 60)
        .with_max_websocket_upstream_messages(1);
        let runtime = must_ok(LoopbackRouterRuntime::start(runtime_config));
        let runtime_address = runtime.local_addr();
        assert_eq!(runtime_address.port(), router_port);
        let router_thread = thread::spawn(move || {
            if let Err(error) = runtime.serve_protocol_connections(1) {
                panic!("router runtime should serve HTTP: {error}");
            }
        });

        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &state_path,
            &secret_root,
            1_000,
            NoopCredentialRefreshClient,
        ));
        let (refresh_started_sender, refresh_started_receiver) = mpsc::channel();
        let (release_refresh_sender, release_refresh_receiver) = mpsc::channel();
        let provider =
            BlockingQuotaRefreshProvider::new(13, refresh_started_sender, release_refresh_receiver);
        let worker = start_background_quota_refresh_worker_with_dependencies(
            state_path,
            secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            resolver,
            provider,
            Duration::from_secs(0),
        );
        if let Err(error) = refresh_started_receiver.recv_timeout(Duration::from_secs(2)) {
            panic!("background refresh should start and block in provider: {error}");
        }

        let http_response = send_loopback_request_with_retry(
            router_port,
            local_token.token().expose_secret(),
            br#"{"model":"gpt-5","served_http":true}"#,
        );
        if let Err(error) = release_refresh_sender.send(()) {
            panic!("test should release blocked quota refresh: {error}");
        }
        drop(worker);
        let (kind, http_request) = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(recorded) => recorded,
            Err(error) => panic!("HTTP upstream request should be recorded: {error}"),
        };

        assert!(
            http_response.starts_with("HTTP/1.1 200 OK\r\n"),
            "unexpected HTTP response: {http_response:?}"
        );
        assert!(http_response.ends_with("\r\ndata: ok\n\n"));
        assert_eq!(kind, "http");
        assert!(
            http_request.starts_with("POST /v1/responses HTTP/1.1\r\n"),
            "unexpected HTTP upstream request: {http_request:?}"
        );
        assert!(
            http_request.contains("authorization: Bearer served-upstream-token\r\n"),
            "unexpected HTTP upstream request: {http_request:?}"
        );
        assert!(!http_request.contains("current-token"));

        match router_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("router thread panicked: {error:?}"),
        }
        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn served_router_websocket_uses_persisted_quota_while_background_refresh_is_blocked() {
        let test_root = TestRoot::new("serve-websocket-background-refresh-blocked");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let token_service = LocalRouterTokenService::new(secrets.clone());
        let local_token = must_ok(token_service.rotate_with_token("current-token"));
        let account_id = account_id("acct_background_served_ws");
        let account = AccountRecord::new(account_id.clone(), "served-ws", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 91);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        persist_effective_selector_window(&state, &account_id, "responses", 91);
        let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
        let upstream_credential_bundle = must_ok(
            AccountCredentialBundle::imported_codex_auth(
                "served-ws-upstream-token",
                Some("served-ws-upstream-refresh-token".to_owned()),
            )
            .to_secret_string(),
        );
        must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

        let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let upstream_address = must_ok(upstream_listener.local_addr());
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock WebSocket upstream should accept: {error}"),
            };
            let mut websocket = match accept_hdr(stream, |request: &Request, response: Response| {
                let authorization = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<missing>")
                    .to_owned();
                if let Err(error) = upstream_sender.send(("ws-auth".to_owned(), authorization)) {
                    panic!("mock WebSocket upstream auth should record: {error}");
                }
                Ok(response)
            }) {
                Ok(websocket) => websocket,
                Err(error) => panic!("mock WebSocket upstream handshake should accept: {error}"),
            };
            let first_frame = match websocket.read() {
                Ok(message) => message,
                Err(error) => panic!("mock WebSocket upstream should read first frame: {error}"),
            };
            if let Err(error) =
                upstream_sender.send(("ws-frame".to_owned(), first_frame.to_string()))
            {
                panic!("mock WebSocket upstream first frame should record: {error}");
            }
            if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
                panic!("mock WebSocket upstream should send response: {error}");
            }
        });

        let router_port = reserve_loopback_port();
        let bind_address = must_ok(LoopbackBindAddress::new("127.0.0.1", router_port));
        let upstream_endpoint = must_ok(UpstreamEndpoint::new(format!(
            "http://{upstream_address}/v1"
        )));
        let runtime_config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            upstream_endpoint,
            state_path.clone(),
            secret_root.clone(),
            local_token.clone(),
        )
        .with_quota_clock(1_030, 60)
        .with_max_websocket_upstream_messages(1);
        let runtime = must_ok(LoopbackRouterRuntime::start(runtime_config));
        assert_eq!(runtime.local_addr().port(), router_port);
        let router_thread = thread::spawn(move || {
            if let Err(error) = runtime.serve_protocol_connections(1) {
                panic!("router runtime should serve WebSocket: {error}");
            }
        });

        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &state_path,
            &secret_root,
            1_000,
            NoopCredentialRefreshClient,
        ));
        let (refresh_started_sender, refresh_started_receiver) = mpsc::channel();
        let (release_refresh_sender, release_refresh_receiver) = mpsc::channel();
        let provider =
            BlockingQuotaRefreshProvider::new(13, refresh_started_sender, release_refresh_receiver);
        let worker = start_background_quota_refresh_worker_with_dependencies(
            state_path,
            secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            resolver,
            provider,
            Duration::from_secs(0),
        );
        if let Err(error) = refresh_started_receiver.recv_timeout(Duration::from_secs(2)) {
            panic!("background refresh should start and block in provider: {error}");
        }

        let mut client =
            connect_websocket_with_retry(router_port, local_token.token().expose_secret());
        let first_frame = r#"{"type":"response.create","served_ws":true}"#;
        if let Err(error) = client.send(Message::text(first_frame)) {
            panic!("local WebSocket client should send first frame: {error}");
        }
        let websocket_response = match client.read() {
            Ok(message) => message.to_string(),
            Err(error) => panic!("local WebSocket client should read response: {error}"),
        };
        assert_eq!(websocket_response, r#"{"type":"response.completed"}"#);

        if let Err(error) = release_refresh_sender.send(()) {
            panic!("test should release blocked quota refresh: {error}");
        }
        drop(worker);
        assert_eq!(
            upstream_receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|error| {
                    panic!("WebSocket upstream auth should be recorded: {error}");
                }),
            (
                "ws-auth".to_owned(),
                "Bearer served-ws-upstream-token".to_owned()
            )
        );
        assert_eq!(
            upstream_receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|error| {
                    panic!("WebSocket upstream first frame should be recorded: {error}");
                }),
            ("ws-frame".to_owned(), first_frame.to_owned())
        );

        match router_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("router thread panicked: {error:?}"),
        }
        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn serve_command_reloads_token_rotation_without_restart() {
        let test_root = TestRoot::new("serve-command-token-rotation");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let token_service = LocalRouterTokenService::new(secrets.clone());
        must_ok(token_service.rotate_with_token("token-a"));
        let account_id = account_id("acct_cli_rotate");
        let account = AccountRecord::new(account_id.clone(), "cli-rotate", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 100);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        persist_effective_selector_window(&state, &account_id, "responses", 100);
        let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
        let upstream_credential_bundle = must_ok(
            AccountCredentialBundle::imported_codex_auth(
                "cli-rotation-upstream-token",
                Some("cli-rotation-upstream-refresh-token".to_owned()),
            )
            .to_secret_string(),
        );
        must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

        let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let upstream_address = must_ok(upstream_listener.local_addr());
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock websocket upstream should accept: {error}"),
            };
            let mut websocket =
                match accept_hdr(stream, |_request: &Request, response: Response| {
                    Ok(response)
                }) {
                    Ok(websocket) => websocket,
                    Err(error) => {
                        panic!("mock websocket upstream handshake should accept: {error}")
                    }
                };
            let first_frame = match websocket.read() {
                Ok(message) => message,
                Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
            };
            if let Err(error) = upstream_sender.send(first_frame.to_string()) {
                panic!("mock websocket upstream first frame should record: {error}");
            }
            let _released = release_receiver.recv_timeout(std::time::Duration::from_secs(2));
            drop(websocket);

            let (mut stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock HTTP upstream should accept after rotation: {error}"),
            };
            let mut request = String::new();
            if let Err(error) = stream.read_to_string(&mut request) {
                panic!("mock HTTP upstream should read request: {error}");
            }
            if let Err(error) = upstream_sender.send(request) {
                panic!("mock HTTP upstream request should record: {error}");
            }
            if let Err(error) =
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
            {
                panic!("mock HTTP upstream should write response: {error}");
            }
        });

        let router_port = reserve_loopback_port();
        let router_port_text = router_port.to_string();
        let upstream_base_url = format!("http://{upstream_address}/v1");
        let state_arg = state_path;
        let secret_arg = secret_root.clone();
        let serve_thread = thread::spawn(move || {
            run_cli(
                [
                    "codex-router",
                    "serve",
                    "--listen-host",
                    "127.0.0.1",
                    "--port",
                    router_port_text.as_str(),
                    "--state-db",
                    path_to_str(&state_arg),
                    "--secret-root",
                    path_to_str(&secret_arg),
                    "--upstream-base-url",
                    upstream_base_url.as_str(),
                    "--now-unix-seconds",
                    "1030",
                    "--max-snapshot-age-seconds",
                    "60",
                    "--disable-background-quota-refresh",
                    "--max-connections",
                    "3",
                ],
                CliContext::new(Vec::new()),
            )
        });

        let mut client = connect_websocket_with_retry(router_port, "token-a");
        if let Err(error) = client.send(Message::text(r#"{"type":"response.create"}"#)) {
            panic!("local websocket client should send first frame: {error}");
        }
        let recorded_first_frame = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(frame) => frame,
            Err(error) => panic!("upstream should receive first frame before rotation: {error}"),
        };
        assert_eq!(recorded_first_frame, r#"{"type":"response.create"}"#);

        let rotate_output = run_cli(
            [
                "codex-router",
                "token",
                "rotate",
                "--router-root",
                path_to_str(&secret_root),
            ],
            CliContext::new(Vec::new()),
        );
        assert_eq!(rotate_output.stdout, "generation: 2\n");
        assert!(rotate_output.stderr.is_empty());
        let token_b = must_ok(token_service.load_current());

        let (old_close_sender, old_close_receiver) = mpsc::channel();
        let old_client_thread = thread::spawn(move || {
            let read_result = client.read().map(|message| message.to_string());
            if let Err(error) = old_close_sender.send(read_result) {
                panic!("old websocket close result should send: {error}");
            }
        });
        let old_close_result = match old_close_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result,
            Err(error) => {
                let _ = release_sender.send(());
                panic!("old-token websocket should close after token rotation: {error}");
            }
        };
        if let Ok(message) = old_close_result {
            panic!("old-token websocket should close, got message: {message}");
        }
        if let Err(error) = release_sender.send(()) {
            panic!("upstream release should send: {error}");
        }
        match old_client_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("old websocket client thread panicked: {error:?}"),
        }

        let old_token_response =
            send_loopback_request_with_retry(router_port, "token-a", br#"{"old":true}"#);
        assert!(old_token_response.starts_with("HTTP/1.1 401 Unauthorized\r\n"));

        let new_token_response = send_loopback_request_with_retry(
            router_port,
            token_b.token().expose_secret(),
            br#"{"new":true}"#,
        );
        assert!(new_token_response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(new_token_response.ends_with("\r\ndata: ok\n\n"));

        let upstream_http_request = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(request) => request,
            Err(error) => panic!("mock HTTP upstream request should be recorded: {error}"),
        };
        assert!(
            upstream_http_request.contains("authorization: Bearer cli-rotation-upstream-token\r\n")
        );
        assert!(!upstream_http_request.contains("X-Codex-Router-Token"));
        assert!(!upstream_http_request.contains("token-a"));
        assert!(!upstream_http_request.contains(token_b.token().expose_secret()));

        let output = match serve_thread.join() {
            Ok(output) => output,
            Err(error) => panic!("serve thread panicked: {error:?}"),
        };
        assert!(
            output
                .stdout
                .contains(format!("listening: 127.0.0.1:{router_port}\n").as_str())
        );
        assert!(output.stderr.is_empty());

        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
    }

    #[derive(Clone)]
    struct RecordingRefreshClient {
        expected_account_id: String,
        expected_refresh_token: String,
        response: AccountCredentialBundle,
        calls: Arc<AtomicUsize>,
    }

    impl RecordingRefreshClient {
        fn new(
            expected_account_id: &str,
            expected_refresh_token: &str,
            response: AccountCredentialBundle,
        ) -> Self {
            Self {
                expected_account_id: expected_account_id.to_owned(),
                expected_refresh_token: expected_refresh_token.to_owned(),
                response,
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl CredentialRefreshClient for RecordingRefreshClient {
        fn refresh_credentials(
            &self,
            account_id: &AccountId,
            refresh_token: &SecretString,
        ) -> Result<AccountCredentialBundle, CredentialResolverError> {
            assert_eq!(account_id.as_str(), self.expected_account_id);
            assert_eq!(refresh_token.expose_secret(), self.expected_refresh_token);
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
        }
    }

    type RecordedQuotaRefresh = (String, String, String, String, String);

    struct RecordingQuotaRefreshProvider {
        remaining_headroom: u32,
        recorded: RefCell<Vec<RecordedQuotaRefresh>>,
    }

    impl RecordingQuotaRefreshProvider {
        fn new(remaining_headroom: u32) -> Self {
            Self {
                remaining_headroom,
                recorded: RefCell::new(Vec::new()),
            }
        }

        fn take_recorded(&self) -> Vec<RecordedQuotaRefresh> {
            self.recorded.take()
        }
    }

    impl QuotaRefreshProvider for RecordingQuotaRefreshProvider {
        fn fetch_quota(
            &self,
            request: QuotaRefreshProviderRequest,
        ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
            self.recorded.borrow_mut().push((
                request.account_id().as_str().to_owned(),
                request.account_label().to_owned(),
                request.route_band().to_owned(),
                request.base_url().to_owned(),
                request.access_token().expose_secret().to_owned(),
            ));
            Ok(QuotaRefreshProviderResponse {
                windows: verified_quota_windows(self.remaining_headroom),
                reset_credits_available: None,
            })
        }
    }

    struct StaticQuotaRefreshProvider {
        windows: Vec<QuotaRefreshProviderWindow>,
    }

    impl StaticQuotaRefreshProvider {
        fn new(windows: Vec<QuotaRefreshProviderWindow>) -> Self {
            Self { windows }
        }
    }

    impl QuotaRefreshProvider for StaticQuotaRefreshProvider {
        fn fetch_quota(
            &self,
            _request: QuotaRefreshProviderRequest,
        ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
            Ok(QuotaRefreshProviderResponse {
                windows: self.windows.clone(),
                reset_credits_available: None,
            })
        }
    }

    struct SlowQuotaRefreshProvider {
        delay: Duration,
        remaining_headroom: u32,
    }

    impl SlowQuotaRefreshProvider {
        fn new(delay: Duration, remaining_headroom: u32) -> Self {
            Self {
                delay,
                remaining_headroom,
            }
        }
    }

    impl QuotaRefreshProvider for SlowQuotaRefreshProvider {
        fn fetch_quota(
            &self,
            _request: QuotaRefreshProviderRequest,
        ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
            thread::sleep(self.delay);
            Ok(QuotaRefreshProviderResponse {
                windows: verified_quota_windows(self.remaining_headroom),
                reset_credits_available: None,
            })
        }
    }

    struct BlockingQuotaRefreshProvider {
        remaining_headroom: u32,
        blocked_once: AtomicBool,
        started_sender: Mutex<Option<mpsc::Sender<()>>>,
        release_receiver: Mutex<mpsc::Receiver<()>>,
    }

    impl BlockingQuotaRefreshProvider {
        fn new(
            remaining_headroom: u32,
            started_sender: mpsc::Sender<()>,
            release_receiver: mpsc::Receiver<()>,
        ) -> Self {
            Self {
                remaining_headroom,
                blocked_once: AtomicBool::new(false),
                started_sender: Mutex::new(Some(started_sender)),
                release_receiver: Mutex::new(release_receiver),
            }
        }
    }

    impl QuotaRefreshProvider for BlockingQuotaRefreshProvider {
        fn fetch_quota(
            &self,
            _request: QuotaRefreshProviderRequest,
        ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
            if !self.blocked_once.swap(true, Ordering::SeqCst) {
                let maybe_started_sender = self
                    .started_sender
                    .lock()
                    .expect("started sender lock should be available")
                    .take();
                if let Some(started_sender) = maybe_started_sender {
                    if let Err(error) = started_sender.send(()) {
                        panic!("background refresh started signal should send: {error}");
                    }
                }
                self.release_receiver
                    .lock()
                    .expect("release receiver lock should be available")
                    .recv()
                    .expect("test should release blocked quota refresh");
            }
            Ok(QuotaRefreshProviderResponse {
                windows: verified_quota_windows(self.remaining_headroom),
                reset_credits_available: None,
            })
        }
    }

    struct SignalingQuotaRefreshProvider {
        remaining_headroom: u32,
        sender: mpsc::Sender<String>,
    }

    impl SignalingQuotaRefreshProvider {
        fn new(remaining_headroom: u32, sender: mpsc::Sender<String>) -> Self {
            Self {
                remaining_headroom,
                sender,
            }
        }
    }

    impl QuotaRefreshProvider for SignalingQuotaRefreshProvider {
        fn fetch_quota(
            &self,
            request: QuotaRefreshProviderRequest,
        ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
            if let Err(error) = self.sender.send(request.route_band().to_owned()) {
                panic!("background refresh signal should send: {error}");
            }
            Ok(QuotaRefreshProviderResponse {
                windows: verified_quota_windows(self.remaining_headroom),
                reset_credits_available: None,
            })
        }
    }

    struct AccountFailingQuotaRefreshProvider {
        failing_account_label: &'static str,
        remaining_headroom: u32,
    }

    impl AccountFailingQuotaRefreshProvider {
        fn new(failing_account_label: &'static str, remaining_headroom: u32) -> Self {
            Self {
                failing_account_label,
                remaining_headroom,
            }
        }
    }

    impl QuotaRefreshProvider for AccountFailingQuotaRefreshProvider {
        fn fetch_quota(
            &self,
            request: QuotaRefreshProviderRequest,
        ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
            if request.account_label() == self.failing_account_label {
                return Err(crate::quota::QuotaCommandError::ProviderStatus { status: 429 });
            }

            Ok(QuotaRefreshProviderResponse {
                windows: verified_quota_windows(self.remaining_headroom),
                reset_credits_available: None,
            })
        }
    }

    fn verified_quota_windows(remaining_headroom: u32) -> Vec<QuotaRefreshProviderWindow> {
        vec![
            QuotaRefreshProviderWindow {
                limit_window_seconds: 18_000,
                remaining_headroom,
                reset_unix_seconds: Some(20_000),
                effective: true,
            },
            QuotaRefreshProviderWindow {
                limit_window_seconds: 604_800,
                remaining_headroom: remaining_headroom.max(50),
                reset_unix_seconds: Some(614_800),
                effective: false,
            },
        ]
    }

    fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected Ok, got error: {error}"),
        }
    }

    fn must_err<T, E: std::fmt::Display>(result: Result<T, E>) -> E {
        match result {
            Ok(_) => panic!("expected error, got Ok"),
            Err(error) => error,
        }
    }

    fn fake_id_token_with_chatgpt_account_id(account_id: &str) -> String {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;

        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id
            }
        });
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(payload.to_string())
        )
    }

    fn remove_dir_all(path: &Path) {
        if let Err(error) = fs::remove_dir_all(path) {
            panic!(
                "failed to remove test directory {}: {error}",
                path.display()
            );
        }
    }

    struct CliRunOutput {
        stdout: String,
        stderr: String,
    }

    fn run_cli<const ARGUMENT_COUNT: usize>(
        args: [&str; ARGUMENT_COUNT],
        context: CliContext,
    ) -> CliRunOutput {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        must_ok(run_with_io(
            args.into_iter().map(Into::into),
            &context,
            &mut stdout,
            &mut stderr,
        ));

        CliRunOutput {
            stdout: must_ok(String::from_utf8(stdout)),
            stderr: must_ok(String::from_utf8(stderr)),
        }
    }

    fn path_to_str(path: &Path) -> &str {
        match path.to_str() {
            Some(path) => path,
            None => panic!("test path must be UTF-8"),
        }
    }

    fn test_current_unix_seconds() -> u64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs(),
            Err(error) => panic!("system clock should be after unix epoch: {error}"),
        }
    }

    fn account_id(value: &str) -> AccountId {
        match AccountId::new(value) {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        }
    }

    fn persist_effective_selector_window(
        state: &SqliteStateStore,
        account_id: &AccountId,
        route_band: &str,
        remaining_headroom: u32,
    ) {
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            route_band,
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(remaining_headroom)
        .with_reset_unix_seconds(18_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        must_ok(SelectorQuotaRepository::upsert_selector_window(
            state,
            &short_window,
        ));
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            route_band,
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(100)
        .with_reset_unix_seconds(604_800)
        .with_observed_unix_seconds(1_000);
        must_ok(SelectorQuotaRepository::upsert_selector_window(
            state,
            &weekly_window,
        ));
    }

    fn reserve_loopback_port() -> u16 {
        let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(listener.local_addr());
        address.port()
    }

    fn send_loopback_request_with_retry(port: u16, token: &str, body: &[u8]) -> String {
        let mut client = connect_with_retry(port);
        let request = format!(
            "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Codex-Router-Token: {token}\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        if let Err(error) = client.write_all(request.as_bytes()) {
            panic!("client request write should succeed: {error}");
        }
        if let Err(error) = client.shutdown(Shutdown::Write) {
            panic!("client write shutdown should succeed: {error}");
        }
        let mut response = String::new();
        if let Err(error) = client.read_to_string(&mut response) {
            panic!("client response read should succeed: {error}");
        }

        response
    }

    fn read_http_request_with_body(stream: &mut TcpStream) -> String {
        let mut request = Vec::new();
        loop {
            let mut buffer = [0_u8; 512];
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    request.extend_from_slice(&buffer[..bytes_read]);
                    if http_message_has_complete_body(&request) {
                        break;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    panic!("mock HTTP upstream timed out before full request: {error}");
                }
                Err(error) => panic!("mock HTTP upstream should read request: {error}"),
            }
        }

        String::from_utf8_lossy(&request).into_owned()
    }

    fn http_message_has_complete_body(message: &[u8]) -> bool {
        let text = String::from_utf8_lossy(message);
        let Some(header_end) = text.find("\r\n\r\n") else {
            return false;
        };
        let headers = &text[..header_end];
        let Some(content_length) = headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        }) else {
            return false;
        };
        message.len() >= header_end + "\r\n\r\n".len() + content_length
    }

    fn connect_with_retry(port: u16) -> TcpStream {
        for _attempt in 0..1_000 {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(stream) => return stream,
                Err(_error) => thread::yield_now(),
            }
        }

        panic!("client should connect to CLI serve listener");
    }

    fn connect_websocket_with_retry(
        port: u16,
        local_token: &str,
    ) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>> {
        for _attempt in 0..1_000 {
            let mut request =
                match format!("ws://127.0.0.1:{port}/v1/responses").into_client_request() {
                    Ok(request) => request,
                    Err(error) => panic!("local websocket request should build: {error}"),
                };
            let header_value = match HeaderValue::from_str(local_token) {
                Ok(value) => value,
                Err(error) => panic!("local websocket token header should build: {error}"),
            };
            request
                .headers_mut()
                .insert("X-Codex-Router-Token", header_value);
            match connect(request) {
                Ok((client, _response)) => return client,
                Err(_error) => thread::yield_now(),
            }
        }

        panic!("local websocket client should connect to CLI serve listener");
    }
}
