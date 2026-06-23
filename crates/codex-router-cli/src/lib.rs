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

use codex_router_proxy::server::LocalAuthReloader;
use codex_router_proxy::server::LoopbackBindAddress;
use codex_router_proxy::server::LoopbackRouterRuntime;
use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
use codex_router_proxy::server::LoopbackRouterRuntimeError;
use codex_router_proxy::server::ServerBindError;
use codex_router_proxy::upstream::UpstreamEndpoint;
use codex_router_proxy::upstream::UpstreamEndpointError;
use codex_router_secret_store::file_backend::FileSecretStore;

mod account;
pub mod doctor;
mod live;
mod observability;
pub mod profile;
mod quota;
pub mod token;

use account::AccountCommand;
use live::LiveCommand;
use profile::CodexRouterProfile;
use profile::CodexRouterProfileWriter;
use profile::ProfileWriteError;
use quota::QuotaCommand;
use thiserror::Error;
use token::LocalRouterTokenService;
use token::Shell;
use token::TokenCommandError;
use token::export_token_assignment;

const DEFAULT_PROFILE_PORT: u16 = 8787;
const LOCAL_TOKEN_ENV_VAR: &str = "CODEX_ROUTER_TOKEN";

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
            let debug_audit_file = command
                .debug_otel
                .then(|| command.router_root.join("debug").join("audit.jsonl"));
            let audit_file = command.audit_file.clone().or(debug_audit_file);
            let _otel_guard = if command.debug_otel {
                Some(observability::init_debug_otel(
                    observability::DebugOtelConfig {
                        router_root: command.router_root.clone(),
                        endpoint: command.otel_endpoint.clone(),
                        health_url: command.otel_health_url.clone(),
                        audit_file: audit_file.clone(),
                    },
                )?)
            } else {
                None
            };
            let secret_store = FileSecretStore::open(&command.secret_root)
                .map_err(TokenCommandError::SecretStore)?;
            let token_service = LocalRouterTokenService::new(secret_store.clone());
            let local_token = token_service.load_current()?;
            let initial_token_generation = local_token.generation();
            let bind_address = LoopbackBindAddress::new(&command.listen_host, command.port)?;
            let upstream_endpoint = UpstreamEndpoint::new(command.upstream_base_url)?;
            let mut runtime_config = LoopbackRouterRuntimeConfig::new(
                bind_address,
                upstream_endpoint,
                command.state_db,
                command.secret_root,
                local_token,
            )
            .with_max_websocket_upstream_messages(command.max_websocket_upstream_messages);
            if let Some(fixed_now_unix_seconds) = command.fixed_now_unix_seconds {
                runtime_config = runtime_config
                    .with_quota_clock(fixed_now_unix_seconds, command.max_snapshot_age_seconds);
            }
            if let Some(audit_file) = audit_file {
                runtime_config = runtime_config.with_audit_file(audit_file);
            }
            quota::validate_quota_refresh_endpoint(
                &command.quota_refresh_base_url,
                command.allow_insecure_quota_base_url,
                command.quota_refresh_timeout_seconds,
            )?;
            let runtime = LoopbackRouterRuntime::start(runtime_config)?;
            let _token_reload_watcher = LocalTokenReloadWatcher::start(
                secret_store,
                runtime.local_auth_reloader(),
                initial_token_generation,
            );
            let _quota_refresh_worker =
                BackgroundQuotaRefreshWorker::start(BackgroundQuotaRefreshConfig {
                    router_root: command.router_root,
                    base_url: command.quota_refresh_base_url,
                    allow_insecure_quota_base_url: command.allow_insecure_quota_base_url,
                    interval_seconds: command.quota_refresh_interval_seconds,
                    timeout_seconds: command.quota_refresh_timeout_seconds,
                    fixed_now_unix_seconds: command.fixed_now_unix_seconds,
                });

            writeln!(stdout, "listening: {}", runtime.local_addr()).map_err(CliError::Stdout)?;
            runtime.serve_protocol_connections(command.max_connections)?;
        }
        CliCommand::Token(TokenCommand::Init { router_root }) => {
            let paths = RouterRootPaths::new(router_root);
            let store =
                FileSecretStore::open(paths.secret_root).map_err(TokenCommandError::SecretStore)?;
            let service = LocalRouterTokenService::new(store);
            let record = service.initialize()?;
            writeln!(stdout, "generation: {}", record.generation().as_u64())
                .map_err(CliError::Stdout)?;
        }
        CliCommand::Token(TokenCommand::Rotate { router_root }) => {
            let paths = RouterRootPaths::new(router_root);
            let store =
                FileSecretStore::open(paths.secret_root).map_err(TokenCommandError::SecretStore)?;
            let service = LocalRouterTokenService::new(store);
            let record = service.rotate()?;
            writeln!(stdout, "generation: {}", record.generation().as_u64())
                .map_err(CliError::Stdout)?;
        }
        CliCommand::Token(TokenCommand::Export { router_root, shell }) => {
            let paths = RouterRootPaths::new(router_root);
            let store =
                FileSecretStore::open(paths.secret_root).map_err(TokenCommandError::SecretStore)?;
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
        CliCommand::Live(command) => live::run_live_command(stdout, command)?,
        CliCommand::Account(command) => account::run_account_command(stdout, command)?,
        CliCommand::Quota(command) => quota::run_quota_command(stdout, command)?,
        CliCommand::Help => {
            stdout
                .write_all(HELP_TEXT.as_bytes())
                .map_err(CliError::Stdout)?;
        }
    }

    stderr.flush().map_err(CliError::Stderr)?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouterRootPaths {
    state_db: PathBuf,
    secret_root: PathBuf,
}

impl RouterRootPaths {
    fn new(router_root: PathBuf) -> Self {
        Self {
            state_db: router_root.join("state.sqlite"),
            secret_root: router_root.join("secrets"),
        }
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct BackgroundQuotaRefreshConfig {
    router_root: PathBuf,
    base_url: String,
    allow_insecure_quota_base_url: bool,
    interval_seconds: u64,
    timeout_seconds: u64,
    fixed_now_unix_seconds: Option<u64>,
}

struct BackgroundQuotaRefreshWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl BackgroundQuotaRefreshWorker {
    fn start(config: BackgroundQuotaRefreshConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            if config.interval_seconds == 0 {
                run_background_quota_refresh_once(&config);
                return;
            }
            while wait_for_quota_refresh_interval(&stop_for_thread, config.interval_seconds) {
                run_background_quota_refresh_once(&config);
            }
        });

        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for BackgroundQuotaRefreshWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _result = thread.join();
        }
    }
}

fn run_background_quota_refresh_once(config: &BackgroundQuotaRefreshConfig) {
    let now_unix_seconds = config
        .fixed_now_unix_seconds
        .map_or_else(current_unix_seconds, Ok)
        .unwrap_or(0);
    let mut sink = std::io::sink();
    if let Err(error) = quota::refresh_quota_state(
        &mut sink,
        quota::QuotaRefreshRunConfig {
            router_root: config.router_root.clone(),
            account: None,
            base_url: config.base_url.clone(),
            allow_insecure_quota_base_url: config.allow_insecure_quota_base_url,
            timeout_seconds: config.timeout_seconds,
            now_unix_seconds,
        },
    ) {
        eprintln!("quota refresh failed: {error}");
    }
}

fn wait_for_quota_refresh_interval(stop: &AtomicBool, interval_seconds: u64) -> bool {
    let interval_ticks = interval_seconds.saturating_mul(10);
    for _tick in 0..interval_ticks {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
    !stop.load(Ordering::Relaxed)
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
    Live(LiveCommand),
    Account(AccountCommand),
    Quota(QuotaCommand),
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
            "live" => Ok(Self::Live(LiveCommand::parse(parser)?)),
            "account" => Ok(Self::Account(AccountCommand::parse(parser)?)),
            "quota" => Ok(Self::Quota(QuotaCommand::parse(parser)?)),
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
    router_root: PathBuf,
    state_db: PathBuf,
    secret_root: PathBuf,
    upstream_base_url: String,
    quota_refresh_base_url: String,
    allow_insecure_quota_base_url: bool,
    quota_refresh_interval_seconds: u64,
    quota_refresh_timeout_seconds: u64,
    fixed_now_unix_seconds: Option<u64>,
    max_snapshot_age_seconds: u64,
    max_websocket_upstream_messages: usize,
    max_connections: usize,
    audit_file: Option<PathBuf>,
    debug_otel: bool,
    otel_endpoint: String,
    otel_health_url: String,
}

impl ServeCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let options = ServeCommandOptions::parse(parser)?;
        let listen_host = options
            .listen_host
            .unwrap_or_else(|| "127.0.0.1".to_owned());
        let port = options.port.unwrap_or(DEFAULT_PROFILE_PORT);
        let router_root = options.router_root.ok_or(CliError::MissingOption {
            option: "--router-root",
        })?;
        let router_paths = RouterRootPaths::new(router_root.clone());
        let upstream_base_url = options.upstream_base_url.ok_or(CliError::MissingOption {
            option: "--upstream-base-url",
        })?;

        Ok(Self {
            listen_host,
            port,
            router_root,
            state_db: router_paths.state_db,
            secret_root: router_paths.secret_root,
            upstream_base_url,
            quota_refresh_base_url: options
                .quota_refresh_base_url
                .unwrap_or_else(quota::default_quota_base_url),
            allow_insecure_quota_base_url: options.allow_insecure_quota_base_url,
            quota_refresh_interval_seconds: options.quota_refresh_interval_seconds.unwrap_or(300),
            quota_refresh_timeout_seconds: options.quota_refresh_timeout_seconds.unwrap_or(30),
            fixed_now_unix_seconds: options.now_unix_seconds,
            max_snapshot_age_seconds: options.max_snapshot_age_seconds.unwrap_or(300),
            max_websocket_upstream_messages: options
                .max_websocket_upstream_messages
                .unwrap_or(usize::MAX),
            max_connections: options.max_connections.unwrap_or(usize::MAX),
            audit_file: options.audit_file,
            debug_otel: options.debug_otel,
            otel_endpoint: options
                .otel_endpoint
                .unwrap_or_else(observability::default_otel_endpoint),
            otel_health_url: options
                .otel_health_url
                .unwrap_or_else(observability::default_otel_health_url),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServeCommandOptions {
    listen_host: Option<String>,
    port: Option<u16>,
    router_root: Option<PathBuf>,
    upstream_base_url: Option<String>,
    quota_refresh_base_url: Option<String>,
    allow_insecure_quota_base_url: bool,
    quota_refresh_interval_seconds: Option<u64>,
    quota_refresh_timeout_seconds: Option<u64>,
    now_unix_seconds: Option<u64>,
    max_snapshot_age_seconds: Option<u64>,
    max_websocket_upstream_messages: Option<usize>,
    max_connections: Option<usize>,
    audit_file: Option<PathBuf>,
    debug_otel: bool,
    otel_endpoint: Option<String>,
    otel_health_url: Option<String>,
}

impl ServeCommandOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self {
            listen_host: None,
            port: None,
            router_root: None,
            upstream_base_url: None,
            quota_refresh_base_url: None,
            allow_insecure_quota_base_url: false,
            quota_refresh_interval_seconds: None,
            quota_refresh_timeout_seconds: None,
            now_unix_seconds: None,
            max_snapshot_age_seconds: None,
            max_websocket_upstream_messages: None,
            max_connections: None,
            audit_file: None,
            debug_otel: false,
            otel_endpoint: None,
            otel_health_url: None,
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
                "--router-root" => {
                    let value = parser.next_required_value("--router-root")?;
                    options.router_root = Some(PathBuf::from(value));
                }
                "--upstream-base-url" => {
                    options.upstream_base_url =
                        Some(parser.next_required_value("--upstream-base-url")?);
                }
                "--quota-refresh-base-url" => {
                    options.quota_refresh_base_url =
                        Some(parser.next_required_value("--quota-refresh-base-url")?);
                }
                "--allow-insecure-quota-base-url" => {
                    options.allow_insecure_quota_base_url = true;
                }
                "--quota-refresh-interval-seconds" => {
                    let value = parser.next_required_value("--quota-refresh-interval-seconds")?;
                    options.quota_refresh_interval_seconds = Some(parse_u64_option(
                        "--quota-refresh-interval-seconds",
                        &value,
                    )?);
                }
                "--quota-refresh-timeout-seconds" => {
                    let value = parser.next_required_value("--quota-refresh-timeout-seconds")?;
                    options.quota_refresh_timeout_seconds =
                        Some(parse_u64_option("--quota-refresh-timeout-seconds", &value)?);
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
                "--audit-file" => {
                    let value = parser.next_required_value("--audit-file")?;
                    options.audit_file = Some(PathBuf::from(value));
                }
                "--debug-otel" => {
                    options.debug_otel = true;
                }
                "--otel-endpoint" => {
                    options.otel_endpoint = Some(parser.next_required_value("--otel-endpoint")?);
                }
                "--otel-health-url" => {
                    options.otel_health_url =
                        Some(parser.next_required_value("--otel-health-url")?);
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
        self.router_root.ok_or_else(|| CliError::MissingOption {
            option: "--router-root",
        })
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
        self.router_root.ok_or(CliError::MissingOption {
            option: "--router-root",
        })
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
struct ArgumentParser {
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

    fn next_string(&mut self) -> Result<Option<String>, CliError> {
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

    fn next_required_value(&mut self, option: &'static str) -> Result<String, CliError> {
        self.next_string()?
            .ok_or(CliError::MissingOptionValue { option })
    }

    fn reject_remaining(&mut self) -> Result<(), CliError> {
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
    /// Account command failed.
    #[error(transparent)]
    Account(#[from] account::AccountCommandError),
    /// Quota command failed.
    #[error(transparent)]
    Quota(#[from] quota::QuotaCommandError),
    /// Debug observability setup failed.
    #[error(transparent)]
    Observability(#[from] observability::ObservabilityError),

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
  account import-codex-auth --router-root <path> --label <label> --auth-json <path> --allow-plaintext-file-secrets
  account list --router-root <path>
  account enable --router-root <path> --account <id-or-label>
  account disable --router-root <path> --account <id-or-label>
  quota status --router-root <path> [--format table|plain] [--all-limits]
  quota refresh --router-root <path> [--account <id-or-label>] [--base-url <url>] [--allow-insecure-quota-base-url]
  serve --router-root <path> --upstream-base-url <url> [--quota-refresh-base-url <url>] [--allow-insecure-quota-base-url] [--quota-refresh-interval-seconds <seconds>] [--quota-refresh-timeout-seconds <seconds>] [--audit-file <path>] [--debug-otel]
  token init --router-root <path>
  token rotate --router-root <path>
  token export --router-root <path> [--shell posix]
  profile print [--port <port>]
  profile doctor
  profile write --codex-home <path> [--port <port>] [--dry-run]
  profile write --codex-home <path> --approve-codex-home-write --preview-token <token>
  live quota --auth-json <path> [--profile-label <label>] [--base-url <url>] [--allow-insecure-quota-base-url] [--format plain|table] [--all-limits]
  live quota --profiles-root <path> [--base-url <url>] [--allow-insecure-quota-base-url] [--format plain|table] [--all-limits]
";

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::io::Read;
    use std::io::Write;
    use std::net::Shutdown;
    use std::net::TcpListener;
    use std::net::TcpStream;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tungstenite::Message;
    use tungstenite::accept_hdr;
    use tungstenite::client::IntoClientRequest;
    use tungstenite::connect;
    use tungstenite::handshake::server::Request;
    use tungstenite::handshake::server::Response;
    use tungstenite::http::HeaderValue;

    use codex_router_core::ids::AccountId;
    use codex_router_core::redaction::SecretString;
    use codex_router_secret_store::account_tokens::upstream_access_token_key;
    use codex_router_secret_store::account_tokens::upstream_refresh_token_key;
    use codex_router_secret_store::file_backend::FileSecretStore;
    use codex_router_secret_store::file_backend::SecretStore;
    use codex_router_state::account::AccountRecord;
    use codex_router_state::account::AccountStatus;
    use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
    use codex_router_state::quota_snapshot::PersistedQuotaStatusRow;
    use codex_router_state::quota_snapshot::QuotaSnapshotSource;
    use codex_router_state::quota_snapshot::QuotaStatusState;
    use codex_router_state::repositories::AccountCredentialRepository;
    use codex_router_state::repositories::AccountStateRepository;
    use codex_router_state::repositories::QuotaSnapshotRepository;
    use codex_router_state::repositories::QuotaStatusRepository;
    use codex_router_state::sqlite::SqliteStateStore;

    use super::CliCommand;
    use super::CliContext;
    use super::CliError;
    use super::package_name;
    use super::run_with_io;
    use crate::doctor::DoctorAccountState;
    use crate::doctor::DoctorReport;
    use crate::doctor::QuotaDoctorState;
    use crate::profile::CodexRouterProfile;
    use crate::profile::CodexRouterProfileWriter;
    use crate::profile::ProfileWriteError;
    use crate::quota;
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

    impl Drop for TestRoot {
        fn drop(&mut self) {
            if self.path.exists() {
                remove_dir_all(&self.path);
            }
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

        assert!(output.stdout.contains("live quota --auth-json <path>"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn token_export_is_single_assignment_without_prose() {
        let assignment =
            export_token_assignment("CODEX_ROUTER_TOKEN", "quote'and\nnewline", Shell::Posix);

        assert!(assignment.starts_with("CODEX_ROUTER_TOKEN='"));
        assert!(assignment.ends_with("'\n"));
        assert_eq!(assignment.matches("CODEX_ROUTER_TOKEN=").count(), 1);
        assert!(assignment.contains("'\\''"));
        assert!(!assignment.contains("export "));
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
        assert!(rendered.contains(
            "env_http_headers = { \"X-Codex-Router-Token\" = \"CODEX_ROUTER_TOKEN\" }\n"
        ));
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

        assert!(output.stdout.contains("account=profile-1"));
        assert!(output.stdout.contains("route=responses"));
        assert!(output.stdout.contains("status=failed"));
        assert!(output.stdout.contains("headroom=0"));
        assert!(
            output
                .stdout
                .contains("notes=api_key_auth_not_quota_compatible")
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
                "--allow-insecure-quota-base-url",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(output.stdout.contains("account=profile-1"));
        assert!(output.stdout.contains("route=responses"));
        assert!(output.stdout.contains("status=fresh"));
        assert!(output.stdout.contains("headroom=20"));
        assert!(output.stdout.contains("window=weekly"));
        assert!(output.stdout.contains("notes=effective"));
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
    fn live_quota_table_format_expands_all_quota_windows_without_tokens() {
        let test_root = TestRoot::new("live-quota-table-all-limits");
        must_ok(fs::create_dir(test_root.path()));
        let profile_root = test_root.path().join("profiles").join("main");
        must_ok(fs::create_dir_all(&profile_root));
        let auth_json = profile_root.join("auth.json");
        must_ok(fs::write(
            &auth_json,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"oauth-secret-canary"}}"#,
        ));

        let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(listener.local_addr());
        let server_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept: {error}"),
            };
            let mut buffer = [0_u8; 4096];
            let bytes_read = match stream.read(&mut buffer) {
                Ok(bytes_read) => bytes_read,
                Err(error) => panic!("quota mock should read request: {error}"),
            };
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            assert!(request.contains("authorization: Bearer oauth-secret-canary\r\n"));
            let body = r#"{
                "rate_limit": {
                    "primary_window": {"used_percent":25,"reset_at":1800,"limit_window_seconds":1000},
                    "secondary_window": {"used_percent":80,"reset_at":9000,"limit_window_seconds":604800}
                },
                "code_review_rate_limit": {
                    "primary_window": {"used_percent":30,"reset_at":3000,"limit_window_seconds":18000},
                    "secondary_window": null
                },
                "additional_rate_limits": [{
                    "limit_name": "Workspace credits",
                    "metered_feature": "codex",
                    "rate_limit": {
                        "primary_window": {"used_percent":27,"reset_at":4000,"limit_window_seconds":2592000},
                        "secondary_window": null
                    }
                }]
            }"#;
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
                path_to_str(&test_root.path().join("profiles")),
                "--base-url",
                base_url.as_str(),
                "--allow-insecure-quota-base-url",
                "--format",
                "table",
                "--all-limits",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(output.stdout.contains("Account"));
        assert!(output.stdout.contains("Route"));
        assert!(output.stdout.contains("Headroom"));
        assert!(output.stdout.contains("Window"));
        assert!(output.stdout.contains("Pace"));
        assert!(output.stdout.contains("Runout"));
        assert!(output.stdout.contains("profile-1"));
        assert!(!output.stdout.contains("main"));
        assert!(output.stdout.contains("responses"));
        assert!(output.stdout.contains("5h"));
        assert!(output.stdout.contains("weekly"));
        assert!(output.stdout.contains("20"));
        assert!(output.stdout.contains("2h 8m"));
        assert!(output.stdout.contains("code_review"));
        assert!(output.stdout.contains("Workspace credits"));
        assert!(output.stdout.contains("73"));
        assert!(output.stdout.contains("save 25%"));
        assert!(!output.stdout.contains("ahead"));
        assert!(!output.stdout.contains("behind"));
        assert!(output.stdout.contains("after reset"));
        assert!(!output.stdout.contains("oauth-secret-canary"));
        assert!(output.stderr.is_empty());

        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }
    }

    #[test]
    fn token_export_command_emits_current_router_root_token_assignment() {
        let test_root = TestRoot::new("token-export-command");
        let store = must_ok(FileSecretStore::open(test_root.path().join("secrets")));
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
        assert!(first_export.stdout.starts_with("CODEX_ROUTER_TOKEN='"));

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
        assert!(second_export.stdout.starts_with("CODEX_ROUTER_TOKEN='"));
        assert_ne!(first_export.stdout, second_export.stdout);
    }

    #[test]
    fn token_export_command_requires_router_root() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = match run_with_io(
            vec![
                "codex-router".into(),
                "token".into(),
                "export".into(),
                "--shell".into(),
                "posix".into(),
            ],
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("token export without router root must fail"),
            Err(error) => error,
        };

        assert_eq!(error.to_string(), "missing required option: --router-root");
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn account_import_codex_auth_requires_plaintext_acknowledgement() {
        let test_root = TestRoot::new("account-import-ack");
        must_ok(fs::create_dir_all(test_root.path()));
        let auth_json = test_root.path().join("auth.json");
        must_ok(fs::write(
            &auth_json,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"access-token-canary"}}"#,
        ));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let error = match run_with_io(
            vec![
                "codex-router".into(),
                "account".into(),
                "import-codex-auth".into(),
                "--router-root".into(),
                test_root.path().into(),
                "--label".into(),
                "work".into(),
                "--auth-json".into(),
                auth_json.into_os_string(),
            ],
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("import without plaintext acknowledgement should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("--allow-plaintext-file-secrets"));
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
        assert!(!error.to_string().contains("access-token-canary"));
    }

    #[test]
    fn account_import_codex_auth_writes_router_owned_state_and_secrets() {
        let test_root = TestRoot::new("account-import-success");
        must_ok(fs::create_dir_all(test_root.path()));
        let auth_json = test_root.path().join("auth.json");
        let auth_content = r#"{"auth_mode":"chatgpt","tokens":{"access_token":" access-token-canary ","refresh_token":" refresh-token-canary ","expires_at":2000}}"#;
        must_ok(fs::write(&auth_json, auth_content));

        let output = run_cli(
            [
                "codex-router",
                "account",
                "import-codex-auth",
                "--router-root",
                path_to_str(test_root.path()),
                "--label",
                "work",
                "--auth-json",
                path_to_str(&auth_json),
                "--allow-plaintext-file-secrets",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(output.stdout.contains("account: acct_"));
        assert!(output.stdout.contains("label: work\n"));
        assert!(output.stdout.contains("status: enabled\n"));
        assert!(output.stdout.contains("import: codex-auth\n"));
        assert!(output.stderr.is_empty());
        assert!(!output.stdout.contains("access-token-canary"));
        assert!(!output.stdout.contains("refresh-token-canary"));
        assert_eq!(must_ok(fs::read_to_string(&auth_json)), auth_content);

        let account_id_text = output_account_id(&output.stdout);
        let account_id = account_id(account_id_text);
        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        assert_eq!(
            AccountStateRepository::load_account(&state, &account_id),
            Ok(Some(AccountRecord::new(
                account_id.clone(),
                "work",
                AccountStatus::Enabled
            )))
        );
        let credential_metadata = match must_ok(
            AccountCredentialRepository::load_credential_metadata(&state, &account_id),
        ) {
            Some(metadata) => metadata,
            None => panic!("credential metadata should persist"),
        };
        assert!(credential_metadata.has_refresh_token());
        assert_eq!(credential_metadata.expires_at_unix_seconds(), Some(2_000));

        let secrets = must_ok(FileSecretStore::open(test_root.path().join("secrets")));
        let access_key = must_ok(upstream_access_token_key(&account_id));
        let refresh_key = must_ok(upstream_refresh_token_key(&account_id));
        assert_eq!(
            must_ok(secrets.read_secret(&access_key)).expose_secret(),
            "access-token-canary"
        );
        assert_eq!(
            must_ok(secrets.read_secret(&refresh_key)).expose_secret(),
            "refresh-token-canary"
        );
    }

    #[test]
    fn account_import_codex_auth_rejects_duplicate_label_without_overwriting_secrets() {
        let test_root = TestRoot::new("account-import-duplicate-label");
        must_ok(fs::create_dir_all(test_root.path()));
        let first_auth_json = test_root.path().join("first-auth.json");
        let second_auth_json = test_root.path().join("second-auth.json");
        must_ok(fs::write(
            &first_auth_json,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"first-access-token","refresh_token":"first-refresh-token"}}"#,
        ));
        must_ok(fs::write(
            &second_auth_json,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"second-access-token","refresh_token":"second-refresh-token"}}"#,
        ));
        let first_output = run_cli(
            [
                "codex-router",
                "account",
                "import-codex-auth",
                "--router-root",
                path_to_str(test_root.path()),
                "--label",
                "work",
                "--auth-json",
                path_to_str(&first_auth_json),
                "--allow-plaintext-file-secrets",
            ],
            CliContext::new(Vec::new()),
        );
        let account_id_text = output_account_id(&first_output.stdout);
        let account_id = account_id(account_id_text);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let error = match run_with_io(
            [
                "codex-router",
                "account",
                "import-codex-auth",
                "--router-root",
                path_to_str(test_root.path()),
                "--label",
                "work",
                "--auth-json",
                path_to_str(&second_auth_json),
                "--allow-plaintext-file-secrets",
            ]
            .into_iter()
            .map(Into::into),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("duplicate label import should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("account label already exists"));
        assert!(must_ok(String::from_utf8(stdout)).is_empty());
        assert!(must_ok(String::from_utf8(stderr)).is_empty());
        let secrets = must_ok(FileSecretStore::open(test_root.path().join("secrets")));
        let access_key = must_ok(upstream_access_token_key(&account_id));
        let refresh_key = must_ok(upstream_refresh_token_key(&account_id));
        assert_eq!(
            must_ok(secrets.read_secret(&access_key)).expose_secret(),
            "first-access-token"
        );
        assert_eq!(
            must_ok(secrets.read_secret(&refresh_key)).expose_secret(),
            "first-refresh-token"
        );
    }

    #[test]
    fn account_list_enable_and_disable_use_router_owned_state() {
        let test_root = TestRoot::new("account-lifecycle");
        must_ok(fs::create_dir_all(test_root.path()));
        let auth_json = test_root.path().join("auth.json");
        must_ok(fs::write(
            &auth_json,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"access","refresh_token":"refresh","expires_at":3000}}"#,
        ));
        let import_output = run_cli(
            [
                "codex-router",
                "account",
                "import-codex-auth",
                "--router-root",
                path_to_str(test_root.path()),
                "--label",
                "work",
                "--auth-json",
                path_to_str(&auth_json),
                "--allow-plaintext-file-secrets",
            ],
            CliContext::new(Vec::new()),
        );
        let account_id_text = output_account_id(&import_output.stdout);

        let list_output = run_cli(
            [
                "codex-router",
                "account",
                "list",
                "--router-root",
                path_to_str(test_root.path()),
            ],
            CliContext::new(Vec::new()),
        );
        assert!(list_output.stdout.contains(account_id_text));
        assert!(list_output.stdout.contains("\twork\tenabled\t"));
        assert!(list_output.stdout.contains("refresh=present"));
        assert!(list_output.stdout.contains("expires_at=3000"));

        let disable_output = run_cli(
            [
                "codex-router",
                "account",
                "disable",
                "--router-root",
                path_to_str(test_root.path()),
                "--account",
                "work",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(disable_output.stdout.contains("status: disabled\n"));

        let enable_output = run_cli(
            [
                "codex-router",
                "account",
                "enable",
                "--router-root",
                path_to_str(test_root.path()),
                "--account",
                account_id_text,
            ],
            CliContext::new(Vec::new()),
        );
        assert!(enable_output.stdout.contains("status: enabled\n"));

        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        let account_id = account_id(account_id_text);
        assert_eq!(
            match must_ok(AccountStateRepository::load_account(&state, &account_id)) {
                Some(account) => account.status(),
                None => panic!("account should exist"),
            },
            AccountStatus::Enabled
        );
    }

    #[test]
    fn account_import_codex_auth_rejects_api_key_without_printing_key() {
        let test_root = TestRoot::new("account-import-api-key");
        must_ok(fs::create_dir_all(test_root.path()));
        let auth_json = test_root.path().join("auth.json");
        must_ok(fs::write(
            &auth_json,
            r#"{"auth_mode":"api_key","OPENAI_API_KEY":"sk-local-secret-canary"}"#,
        ));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let error = match run_with_io(
            vec![
                "codex-router".into(),
                "account".into(),
                "import-codex-auth".into(),
                "--router-root".into(),
                test_root.path().into(),
                "--label".into(),
                "work".into(),
                "--auth-json".into(),
                auth_json.into_os_string(),
                "--allow-plaintext-file-secrets".into(),
            ],
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("api-key import should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("not API-key auth"));
        assert!(!format!("{error:?}").contains("sk-local-secret-canary"));
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn account_import_codex_auth_rejects_email_like_label() {
        let test_root = TestRoot::new("account-import-email-label");
        must_ok(fs::create_dir_all(test_root.path()));
        let auth_json = test_root.path().join("auth.json");
        must_ok(fs::write(
            &auth_json,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"access-token-canary"}}"#,
        ));
        for label in [
            "person@example.com",
            "person@example",
            "person@corp",
            "team west",
            "good\nstatus: failed",
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();

            let error = match run_with_io(
                vec![
                    "codex-router".into(),
                    "account".into(),
                    "import-codex-auth".into(),
                    "--router-root".into(),
                    test_root.path().into(),
                    "--label".into(),
                    label.into(),
                    "--auth-json".into(),
                    auth_json.clone().into_os_string(),
                    "--allow-plaintext-file-secrets".into(),
                ],
                &CliContext::new(Vec::new()),
                &mut stdout,
                &mut stderr,
            ) {
                Ok(()) => panic!("email-like label should fail: {label}"),
                Err(error) => error,
            };

            assert!(error.to_string().contains("non-email local label"));
            assert!(!error.to_string().contains(label));
            assert!(!format!("{error:?}").contains("access-token-canary"));
            assert!(stdout.is_empty());
            assert!(stderr.is_empty());
        }
    }

    #[test]
    fn quota_status_reads_sqlite_rows_without_provider_io() {
        let test_root = TestRoot::new("quota-status-sqlite");
        must_ok(fs::create_dir_all(test_root.path()));
        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        let account_id = account_id("acct_quota_status");
        let account = AccountRecord::new(account_id.clone(), "work", AccountStatus::Enabled);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let effective =
            quota_status_row(account_id.clone(), "responses", "rate_limit", "effective")
                .with_observed_unix_seconds(1_000)
                .with_status(QuotaStatusState::Fresh)
                .with_used_percent(80)
                .with_remaining_headroom(20)
                .with_reset_unix_seconds(9_000)
                .with_limit_window_seconds(604_800)
                .with_effective(true);
        let primary = quota_status_row(account_id, "responses", "rate_limit", "5h")
            .with_observed_unix_seconds(1_000)
            .with_status(QuotaStatusState::Fresh)
            .with_used_percent(25)
            .with_remaining_headroom(75)
            .with_reset_unix_seconds(1_800)
            .with_limit_window_seconds(1_000);
        must_ok(QuotaStatusRepository::upsert_status_row(&state, &effective));
        must_ok(QuotaStatusRepository::upsert_status_row(&state, &primary));

        let compact_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(compact_output.stdout.contains("Account"));
        assert!(compact_output.stdout.contains("Route"));
        assert!(compact_output.stdout.contains("Headroom"));
        assert!(compact_output.stdout.contains("work"));
        assert!(compact_output.stdout.contains("20% [##--------]"));
        assert!(compact_output.stdout.contains("effective"));
        assert!(!compact_output.stdout.contains("75% [#######---]"));
        assert!(!compact_output.stdout.contains("ahead"));
        assert!(!compact_output.stdout.contains("behind"));

        let detailed_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
                "--all-limits",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(detailed_output.stdout.contains("20% [##--------]"));
        assert!(detailed_output.stdout.contains("75% [#######---]"));
        assert!(detailed_output.stdout.contains("save 25%"));
        assert!(detailed_output.stderr.is_empty());
    }

    #[test]
    fn quota_status_plain_reports_enabled_accounts_without_rows_as_unknown() {
        let test_root = TestRoot::new("quota-status-unknown-account");
        must_ok(fs::create_dir_all(test_root.path()));
        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        let account = AccountRecord::new(
            account_id("acct_quota_unknown"),
            "fresh-import",
            AccountStatus::Enabled,
        );
        must_ok(AccountStateRepository::upsert_account(&state, &account));

        let output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
                "--format",
                "plain",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(output.stdout.contains("account=fresh-import"));
        assert!(output.stdout.contains("route=responses"));
        assert!(output.stdout.contains("status=unknown"));
        assert!(output.stdout.contains("notes=not%20refreshed"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn quota_status_plain_percent_encodes_space_containing_values() {
        let test_root = TestRoot::new("quota-status-plain-encoded");
        must_ok(fs::create_dir_all(test_root.path()));
        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        let account_id = account_id("acct_quota_plain_encoded");
        let account = AccountRecord::new(account_id.clone(), "team west", AccountStatus::Enabled);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let status_row = quota_status_row(account_id, "responses", "Workspace credits", "30d")
            .with_observed_unix_seconds(1_000)
            .with_status(QuotaStatusState::Fresh)
            .with_remaining_headroom(73)
            .with_reset_unix_seconds(8_200)
            .with_limit_window_seconds(7_200);
        must_ok(QuotaStatusRepository::upsert_status_row(
            &state,
            &status_row,
        ));

        let output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
                "--format",
                "plain",
                "--all-limits",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );

        assert!(output.stdout.contains("account=team_west"));
        assert!(output.stdout.contains("notes=Workspace%20credits"));
        assert!(!output.stdout.contains("account=team west"));
        assert!(!output.stdout.contains("notes=Workspace credits"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn quota_status_rejects_json_format_explicitly() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run_with_io(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                "/tmp/codex-router-format-json",
                "--format",
                "json",
            ]
            .into_iter()
            .map(Into::into),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        );

        match result {
            Err(CliError::UnknownOption { option }) => {
                assert_eq!(option, "--format json is not implemented for quota status");
            }
            Ok(()) => panic!("quota status --format json should fail"),
            Err(error) => panic!("unexpected quota status format error: {error}"),
        }
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn quota_status_read_only_does_not_create_missing_state_database() {
        let test_root = TestRoot::new("quota-status-missing-db");
        must_ok(fs::create_dir_all(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run_with_io(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
            ]
            .into_iter()
            .map(Into::into),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        );

        match result {
            Err(CliError::Quota(crate::quota::QuotaCommandError::State(_))) => {}
            Ok(()) => panic!("quota status without state database should fail"),
            Err(error) => panic!("unexpected quota status missing db error: {error}"),
        }
        assert!(!state_path.exists());
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn quota_refresh_rejects_non_provider_base_url_before_token_egress() {
        let test_root = TestRoot::new("quota-refresh-rejects-base-url");
        must_ok(fs::create_dir_all(test_root.path()));
        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        let account_id = account_id("acct_reject_base_url");
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(account_id.clone(), "work", AccountStatus::Enabled),
        ));
        let secrets = must_ok(FileSecretStore::open(test_root.path().join("secrets")));
        let access_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(&access_key, &SecretString::new("no-egress-token-canary")));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run_with_io(
            [
                "codex-router",
                "quota",
                "refresh",
                "--router-root",
                path_to_str(test_root.path()),
                "--base-url",
                "http://example.test",
                "--now-unix-seconds",
                "1300",
            ]
            .into_iter()
            .map(Into::into),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        );

        match result {
            Err(CliError::Quota(crate::quota::QuotaCommandError::LiveQuota(error))) => {
                assert!(error.to_string().contains("https://chatgpt.com"));
            }
            Ok(()) => panic!("non-provider quota base URL should fail"),
            Err(error) => panic!("unexpected quota base URL error: {error}"),
        }
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn quota_refresh_past_reset_window_replaces_old_snapshot_with_failed_zero() {
        let test_root = TestRoot::new("quota-refresh-past-reset");
        must_ok(fs::create_dir_all(test_root.path()));
        let account_id = account_id("acct_past_reset");
        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(account_id.clone(), "work", AccountStatus::Enabled),
        ));
        let old_snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 99);
        let old_models_snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("models", 77);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(
            &state,
            &old_snapshot,
        ));
        must_ok(QuotaSnapshotRepository::upsert_snapshot(
            &state,
            &old_models_snapshot,
        ));
        let secrets = must_ok(FileSecretStore::open(test_root.path().join("secrets")));
        let access_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(&access_key, &SecretString::new("provider-token-canary")));

        let quota_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(quota_listener.local_addr());
        let server_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match quota_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept: {error}"),
            };
            let mut request_buffer = [0_u8; 4096];
            let request_bytes = match stream.read(&mut request_buffer) {
                Ok(request_bytes) => request_bytes,
                Err(error) => panic!("quota mock should read request: {error}"),
            };
            let request = String::from_utf8_lossy(&request_buffer[..request_bytes]);
            assert!(request.contains("authorization: Bearer provider-token-canary\r\n"));
            let body = r#"{"rate_limit":{"primary_window":{"used_percent":25,"reset_at":1200,"limit_window_seconds":1000}}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                panic!("quota mock should write response: {error}");
            }
        });

        let quota_base_url = format!("http://{address}");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run_with_io(
            [
                "codex-router",
                "quota",
                "refresh",
                "--router-root",
                path_to_str(test_root.path()),
                "--base-url",
                quota_base_url.as_str(),
                "--allow-insecure-quota-base-url",
                "--now-unix-seconds",
                "1300",
            ]
            .into_iter()
            .map(Into::into),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        );

        match result {
            Err(CliError::Quota(crate::quota::QuotaCommandError::RefreshFailed {
                failed_accounts,
            })) => assert_eq!(failed_accounts, 1),
            Ok(()) => panic!("past-reset quota window should fail closed"),
            Err(error) => panic!("unexpected past-reset quota error: {error}"),
        }
        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }
        assert!(
            must_ok(String::from_utf8(stdout))
                .contains("failed: acct_past_reset reason=provider-quota-refresh-failed")
        );
        assert!(must_ok(String::from_utf8(stderr)).is_empty());
        assert_eq!(
            must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &account_id,
                "responses",
            ))
            .map(|snapshot| snapshot.remaining_headroom()),
            Some(0)
        );
        assert_eq!(
            must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &account_id,
                "models",
            ))
            .map(|snapshot| snapshot.remaining_headroom()),
            Some(0)
        );

        let status_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
                "--format",
                "plain",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(status_output.stdout.contains("status=failed"));
        assert!(status_output.stdout.contains("headroom=0"));
        assert!(
            status_output
                .stdout
                .contains("notes=provider%20quota%20missing%20usable%20windows")
        );

        let detailed_status_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
                "--format",
                "plain",
                "--all-limits",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(
            detailed_status_output
                .stdout
                .contains("notes=provider%20quota%20window%20invalid")
        );
    }

    #[test]
    fn quota_refresh_persists_selector_snapshot_and_status_rows() {
        let test_root = TestRoot::new("quota-refresh");
        must_ok(fs::create_dir_all(test_root.path()));
        let account_id = account_id("acct_quota_refresh");
        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        let account = AccountRecord::new(account_id.clone(), "work", AccountStatus::Enabled);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let secrets = must_ok(FileSecretStore::open(test_root.path().join("secrets")));
        let access_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(&access_key, &SecretString::new("provider-token-canary")));

        let quota_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(quota_listener.local_addr());
        let server_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match quota_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept: {error}"),
            };
            let mut request_buffer = [0_u8; 4096];
            let request_bytes = match stream.read(&mut request_buffer) {
                Ok(request_bytes) => request_bytes,
                Err(error) => panic!("quota mock should read request: {error}"),
            };
            let request = String::from_utf8_lossy(&request_buffer[..request_bytes]);
            assert!(request.starts_with("GET /api/codex/usage HTTP/1.1\r\n"));
            assert!(request.contains("authorization: Bearer provider-token-canary\r\n"));
            let body = r#"{
                "rate_limit": {
                    "primary_window": {"used_percent":25,"reset_at":1800,"limit_window_seconds":1000},
                    "secondary_window": {"used_percent":80,"reset_at":9000,"limit_window_seconds":604800}
                },
                "code_review_rate_limit": {
                    "primary_window": {"used_percent":30,"reset_at":3000,"limit_window_seconds":18000}
                },
                "additional_rate_limits": [{
                    "limit_name": "Workspace credits",
                    "rate_limit": {
                        "primary_window": {"used_percent":27,"reset_at":4000,"limit_window_seconds":2592000}
                    }
                }]
            }"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                panic!("quota mock should write response: {error}");
            }
        });

        let quota_base_url = format!("http://{address}");
        let refresh_output = run_cli(
            [
                "codex-router",
                "quota",
                "refresh",
                "--router-root",
                path_to_str(test_root.path()),
                "--account",
                "work",
                "--base-url",
                quota_base_url.as_str(),
                "--allow-insecure-quota-base-url",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(
            refresh_output
                .stdout
                .contains("refreshed: acct_quota_refresh route=responses")
        );
        assert!(
            refresh_output
                .stdout
                .contains("refreshed: acct_quota_refresh route=code_review")
        );
        assert!(refresh_output.stderr.is_empty());

        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }

        assert_eq!(
            must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &account_id,
                "responses",
            ))
            .map(|snapshot| snapshot.remaining_headroom()),
            Some(20)
        );
        let status_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
                "--all-limits",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(
            status_output.stdout.contains("Workspace credits"),
            "{}",
            status_output.stdout
        );
        assert!(status_output.stdout.contains("code_review"));
        assert!(status_output.stdout.contains("save 25%"));
        assert!(!status_output.stdout.contains("provider-token-canary"));
    }

    #[test]
    fn quota_refresh_populates_models_route_band_used_by_serve() {
        let test_root = TestRoot::new("quota-refresh-models-route");
        must_ok(fs::create_dir_all(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let token_service = LocalRouterTokenService::new(secrets.clone());
        let local_token = must_ok(token_service.rotate_with_token("current-token"));
        let account_id = account_id("acct_models_after_refresh");
        let account = AccountRecord::new(account_id.clone(), "models", AccountStatus::Enabled);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let upstream_token_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(
            &upstream_token_key,
            &SecretString::new("models-upstream-token"),
        ));

        let quota_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let quota_address = must_ok(quota_listener.local_addr());
        let quota_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match quota_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept: {error}"),
            };
            let mut request_buffer = [0_u8; 4096];
            let request_bytes = match stream.read(&mut request_buffer) {
                Ok(request_bytes) => request_bytes,
                Err(error) => panic!("quota mock should read request: {error}"),
            };
            let request = String::from_utf8_lossy(&request_buffer[..request_bytes]);
            assert!(request.contains("authorization: Bearer models-upstream-token\r\n"));
            let body = r#"{
                "rate_limit": {
                    "primary_window": {"used_percent":25,"reset_at":1800,"limit_window_seconds":1000},
                    "secondary_window": {"used_percent":80,"reset_at":9000,"limit_window_seconds":604800}
                },
                "code_review_rate_limit": {
                    "primary_window": {"used_percent":30,"reset_at":3000,"limit_window_seconds":18000}
                },
                "additional_rate_limits": []
            }"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                panic!("quota mock should write response: {error}");
            }
        });
        let quota_base_url = format!("http://{quota_address}");
        let refresh_output = run_cli(
            [
                "codex-router",
                "quota",
                "refresh",
                "--router-root",
                path_to_str(test_root.path()),
                "--base-url",
                quota_base_url.as_str(),
                "--allow-insecure-quota-base-url",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(
            refresh_output
                .stdout
                .contains("refreshed: acct_models_after_refresh route=models")
        );
        match quota_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }
        assert_eq!(
            must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &account_id,
                "models",
            ))
            .map(|snapshot| snapshot.remaining_headroom()),
            Some(20)
        );

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
            if let Err(error) = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 12\r\n\r\n{\"data\":[]}",
            ) {
                panic!("mock upstream should write response: {error}");
            }
        });
        let router_port = reserve_loopback_port();
        let router_port_text = router_port.to_string();
        let upstream_base_url = format!("http://{upstream_address}/v1");
        let client_thread = thread::spawn(move || {
            send_loopback_get_request_with_retry(router_port, local_token.token().expose_secret())
        });

        let output = run_cli(
            [
                "codex-router",
                "serve",
                "--listen-host",
                "127.0.0.1",
                "--port",
                router_port_text.as_str(),
                "--router-root",
                path_to_str(test_root.path()),
                "--upstream-base-url",
                upstream_base_url.as_str(),
                "--quota-refresh-interval-seconds",
                "3600",
                "--now-unix-seconds",
                "1300",
                "--max-snapshot-age-seconds",
                "60",
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
        let upstream_request = match upstream_receiver.recv() {
            Ok(request) => request,
            Err(error) => panic!("mock upstream request should be recorded: {error}"),
        };
        assert!(upstream_request.starts_with("GET /v1/models HTTP/1.1\r\n"));
        assert!(upstream_request.contains("authorization: Bearer models-upstream-token\r\n"));

        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
    }

    #[test]
    fn quota_refresh_persists_failure_row_and_continues_healthy_accounts() {
        let test_root = TestRoot::new("quota-refresh-partial-failure");
        must_ok(fs::create_dir_all(test_root.path()));
        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        let missing_account_id = account_id("acct_missing_quota_secret");
        let healthy_account_id = account_id("acct_healthy_quota_refresh");
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(missing_account_id, "missing", AccountStatus::Enabled),
        ));
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                healthy_account_id.clone(),
                "healthy",
                AccountStatus::Enabled,
            ),
        ));
        let secrets = must_ok(FileSecretStore::open(test_root.path().join("secrets")));
        let access_key = must_ok(upstream_access_token_key(&healthy_account_id));
        must_ok(secrets.write_secret(&access_key, &SecretString::new("healthy-provider-token")));

        let quota_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(quota_listener.local_addr());
        let server_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match quota_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept healthy account request: {error}"),
            };
            let mut request_buffer = [0_u8; 4096];
            let request_bytes = match stream.read(&mut request_buffer) {
                Ok(request_bytes) => request_bytes,
                Err(error) => panic!("quota mock should read request: {error}"),
            };
            let request = String::from_utf8_lossy(&request_buffer[..request_bytes]);
            assert!(request.contains("authorization: Bearer healthy-provider-token\r\n"));
            let body = r#"{
                "rate_limit": {
                    "primary_window": {"used_percent":35,"reset_at":2300,"limit_window_seconds":1000}
                },
                "code_review_rate_limit": {
                    "primary_window": {"used_percent":30,"reset_at":3000,"limit_window_seconds":18000}
                },
                "additional_rate_limits": []
            }"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                panic!("quota mock should write response: {error}");
            }
        });

        let quota_base_url = format!("http://{address}");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run_with_io(
            [
                "codex-router",
                "quota",
                "refresh",
                "--router-root",
                path_to_str(test_root.path()),
                "--base-url",
                quota_base_url.as_str(),
                "--allow-insecure-quota-base-url",
                "--now-unix-seconds",
                "1300",
            ]
            .into_iter()
            .map(Into::into),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        );

        match result {
            Err(CliError::Quota(crate::quota::QuotaCommandError::RefreshFailed {
                failed_accounts,
            })) => assert_eq!(failed_accounts, 1),
            Ok(()) => panic!("partial refresh failure should return an error"),
            Err(error) => panic!("unexpected refresh error: {error}"),
        }
        let stdout_text = must_ok(String::from_utf8(stdout));
        assert!(
            stdout_text.contains("failed: acct_missing_quota_secret reason=credential-unavailable")
        );
        assert!(stdout_text.contains("refreshed: acct_healthy_quota_refresh route=responses"));
        assert!(must_ok(String::from_utf8(stderr)).is_empty());

        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }
        assert_eq!(
            must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &healthy_account_id,
                "responses",
            ))
            .map(|snapshot| snapshot.remaining_headroom()),
            Some(65)
        );

        let status_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
                "--all-limits",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(status_output.stdout.contains("missing"));
        assert!(status_output.stdout.contains("failed"));
        assert!(status_output.stdout.contains("credential unavailable"));
        assert!(status_output.stdout.contains("healthy"));
        assert!(status_output.stdout.contains("65% [######----]"));
        assert!(!status_output.stdout.contains("healthy-provider-token"));
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
        let account = AccountRecord::new(account_id.clone(), "cli-serve", AccountStatus::Enabled);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 100);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        let upstream_token_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(
            &upstream_token_key,
            &SecretString::new("cli-upstream-token"),
        ));

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
                "--router-root",
                path_to_str(test_root.path()),
                "--upstream-base-url",
                upstream_base_url.as_str(),
                "--now-unix-seconds",
                "1030",
                "--max-snapshot-age-seconds",
                "60",
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
    fn serve_command_runs_background_quota_refresh_without_request_path_quota_io() {
        let test_root = TestRoot::new("serve-command-background-quota");
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let secrets = must_ok(FileSecretStore::open(&secret_root));
        let token_service = LocalRouterTokenService::new(secrets.clone());
        let local_token = must_ok(token_service.rotate_with_token("current-token"));
        let account_id = account_id("acct_cli_background_quota");
        let account = AccountRecord::new(account_id.clone(), "cli-bg", AccountStatus::Enabled);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 100);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        let upstream_token_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(
            &upstream_token_key,
            &SecretString::new("cli-background-upstream-token"),
        ));

        let quota_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let quota_address = must_ok(quota_listener.local_addr());
        let quota_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match quota_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept: {error}"),
            };
            let mut request_buffer = [0_u8; 4096];
            let request_bytes = match stream.read(&mut request_buffer) {
                Ok(request_bytes) => request_bytes,
                Err(error) => panic!("quota mock should read request: {error}"),
            };
            let request = String::from_utf8_lossy(&request_buffer[..request_bytes]);
            assert!(request.starts_with("GET /api/codex/usage HTTP/1.1\r\n"));
            assert!(request.contains("authorization: Bearer cli-background-upstream-token\r\n"));
            let body = r#"{
                "rate_limit": {
                    "primary_window": {"used_percent":25,"reset_at":1800,"limit_window_seconds":1000},
                    "secondary_window": {"used_percent":80,"reset_at":9000,"limit_window_seconds":604800}
                },
                "additional_rate_limits": []
            }"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                panic!("quota mock should write response: {error}");
            }
        });

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
        let quota_base_url = format!("http://{quota_address}");
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
                "--router-root",
                path_to_str(test_root.path()),
                "--upstream-base-url",
                upstream_base_url.as_str(),
                "--quota-refresh-base-url",
                quota_base_url.as_str(),
                "--allow-insecure-quota-base-url",
                "--quota-refresh-interval-seconds",
                "0",
                "--quota-refresh-timeout-seconds",
                "2",
                "--now-unix-seconds",
                "1300",
                "--max-snapshot-age-seconds",
                "60",
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
        let upstream_request = match upstream_receiver.recv() {
            Ok(request) => request,
            Err(error) => panic!("mock upstream request should be recorded: {error}"),
        };
        assert!(upstream_request.starts_with("POST /v1/responses HTTP/1.1\r\n"));
        assert!(
            upstream_request.contains("authorization: Bearer cli-background-upstream-token\r\n")
        );

        match quota_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }
        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
        assert_eq!(
            must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &account_id,
                "responses",
            ))
            .map(|snapshot| snapshot.remaining_headroom()),
            Some(20)
        );
    }

    #[test]
    fn quota_refresh_missing_code_review_replaces_old_snapshot_with_failed_zero() {
        let test_root = TestRoot::new("quota-refresh-missing-code-review");
        must_ok(fs::create_dir_all(test_root.path()));
        let account_id = account_id("acct_missing_code_review");
        let state = must_ok(SqliteStateStore::open(
            &test_root.path().join("state.sqlite"),
        ));
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(account_id.clone(), "work", AccountStatus::Enabled),
        ));
        let old_snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("code_review", 88);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(
            &state,
            &old_snapshot,
        ));
        must_ok(QuotaStatusRepository::upsert_status_row(
            &state,
            &quota_status_row(
                account_id.clone(),
                "code_review",
                "code_review",
                "effective",
            )
            .with_observed_unix_seconds(1_000)
            .with_status(QuotaStatusState::Fresh)
            .with_remaining_headroom(88)
            .with_effective(true),
        ));
        let secrets = must_ok(FileSecretStore::open(test_root.path().join("secrets")));
        let access_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(&access_key, &SecretString::new("provider-token-canary")));

        let quota_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(quota_listener.local_addr());
        let server_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match quota_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("quota mock should accept: {error}"),
            };
            let mut request_buffer = [0_u8; 4096];
            let request_bytes = match stream.read(&mut request_buffer) {
                Ok(request_bytes) => request_bytes,
                Err(error) => panic!("quota mock should read request: {error}"),
            };
            let request = String::from_utf8_lossy(&request_buffer[..request_bytes]);
            assert!(request.contains("authorization: Bearer provider-token-canary\r\n"));
            let body = r#"{
                "rate_limit": {
                    "primary_window": {"used_percent":25,"reset_at":1800,"limit_window_seconds":1000}
                },
                "additional_rate_limits": []
            }"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                panic!("quota mock should write response: {error}");
            }
        });

        let quota_base_url = format!("http://{address}");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run_with_io(
            [
                "codex-router",
                "quota",
                "refresh",
                "--router-root",
                path_to_str(test_root.path()),
                "--base-url",
                quota_base_url.as_str(),
                "--allow-insecure-quota-base-url",
                "--now-unix-seconds",
                "1300",
            ]
            .into_iter()
            .map(Into::into),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        );

        match result {
            Err(CliError::Quota(crate::quota::QuotaCommandError::RefreshFailed {
                failed_accounts,
            })) => assert_eq!(failed_accounts, 1),
            Ok(()) => panic!("missing code-review quota should fail closed"),
            Err(error) => panic!("unexpected missing code-review error: {error}"),
        }
        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("quota mock thread panicked: {error:?}"),
        }
        let stdout_text = must_ok(String::from_utf8(stdout));
        assert!(stdout_text.contains("refreshed: acct_missing_code_review route=responses"));
        assert!(stdout_text.contains("refreshed: acct_missing_code_review route=code_review"));
        assert!(
            stdout_text
                .contains("failed: acct_missing_code_review reason=provider-quota-refresh-failed")
        );
        assert!(must_ok(String::from_utf8(stderr)).is_empty());
        assert_eq!(
            must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
                &state,
                &account_id,
                "code_review",
            ))
            .map(|snapshot| snapshot.remaining_headroom()),
            Some(0)
        );
        let status_output = run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(test_root.path()),
                "--format",
                "plain",
                "--all-limits",
                "--now-unix-seconds",
                "1300",
            ],
            CliContext::new(Vec::new()),
        );
        assert!(status_output.stdout.contains("route=code_review"));
        assert!(status_output.stdout.contains("status=failed"));
        assert!(
            status_output
                .stdout
                .contains("notes=provider%20quota%20missing%20usable%20windows")
        );
    }

    #[test]
    fn serve_command_defaults_quota_clock_to_dynamic_system_time() {
        let command = match CliCommand::parse([
            OsString::from("serve"),
            OsString::from("--router-root"),
            OsString::from("/tmp/codex-router"),
            OsString::from("--upstream-base-url"),
            OsString::from("http://127.0.0.1:1/v1"),
        ]) {
            Ok(CliCommand::Serve(command)) => command,
            Ok(other) => panic!("serve command should parse, got {other:?}"),
            Err(error) => panic!("serve command should parse: {error}"),
        };

        assert_eq!(command.fixed_now_unix_seconds, None);
        assert_eq!(
            command.state_db,
            PathBuf::from("/tmp/codex-router/state.sqlite")
        );
        assert_eq!(
            command.secret_root,
            PathBuf::from("/tmp/codex-router/secrets")
        );
        assert_eq!(command.quota_refresh_interval_seconds, 300);
        assert_eq!(command.quota_refresh_timeout_seconds, 30);
        assert_eq!(
            command.quota_refresh_base_url,
            quota::default_quota_base_url()
        );
    }

    #[test]
    fn serve_command_rejects_disallowed_quota_refresh_base_url_before_listening() {
        let test_root = TestRoot::new("serve-command-invalid-quota-base-url");
        must_ok(fs::create_dir(test_root.path()));
        let secrets = must_ok(FileSecretStore::open(test_root.path().join("secrets")));
        let token_service = LocalRouterTokenService::new(secrets);
        let _local_token = must_ok(token_service.rotate_with_token("current-token"));
        let router_port = reserve_loopback_port();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let error = match run_with_io(
            [
                "codex-router",
                "serve",
                "--listen-host",
                "127.0.0.1",
                "--port",
                router_port.to_string().as_str(),
                "--router-root",
                path_to_str(test_root.path()),
                "--upstream-base-url",
                "http://127.0.0.1:1/v1",
                "--quota-refresh-base-url",
                "http://127.0.0.1:1",
                "--max-connections",
                "1",
            ]
            .into_iter()
            .map(Into::into),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ) {
            Ok(()) => panic!("serve should reject disallowed quota refresh base URL"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("quota base URL must be https://chatgpt.com/backend-api")
        );
        assert!(must_ok(String::from_utf8(stdout)).is_empty());
        assert!(must_ok(String::from_utf8(stderr)).is_empty());
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
        let account = AccountRecord::new(account_id.clone(), "cli-ws", AccountStatus::Enabled);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 100);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        let upstream_token_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(
            &upstream_token_key,
            &SecretString::new("cli-ws-upstream-token"),
        ));

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
                "--router-root",
                path_to_str(test_root.path()),
                "--upstream-base-url",
                upstream_base_url.as_str(),
                "--now-unix-seconds",
                "1030",
                "--max-snapshot-age-seconds",
                "60",
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
        let account = AccountRecord::new(account_id.clone(), "cli-rotate", AccountStatus::Enabled);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 100);
        must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
        let upstream_token_key = must_ok(upstream_access_token_key(&account_id));
        must_ok(secrets.write_secret(
            &upstream_token_key,
            &SecretString::new("cli-rotation-upstream-token"),
        ));

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
        let router_root_arg = test_root.path().to_path_buf();
        let serve_thread = thread::spawn(move || {
            run_cli(
                [
                    "codex-router",
                    "serve",
                    "--listen-host",
                    "127.0.0.1",
                    "--port",
                    router_port_text.as_str(),
                    "--router-root",
                    path_to_str(&router_root_arg),
                    "--upstream-base-url",
                    upstream_base_url.as_str(),
                    "--now-unix-seconds",
                    "1030",
                    "--max-snapshot-age-seconds",
                    "60",
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
                path_to_str(test_root.path()),
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

    fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected Ok, got error: {error}"),
        }
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

    fn output_account_id(output: &str) -> &str {
        match output
            .lines()
            .find_map(|line| line.strip_prefix("account: "))
        {
            Some(account_id) => account_id,
            None => panic!("import output should include account id"),
        }
    }

    fn account_id(value: &str) -> AccountId {
        match AccountId::new(value) {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        }
    }

    fn quota_status_row(
        account_id: AccountId,
        route_band: &str,
        family: &str,
        window_label: &str,
    ) -> PersistedQuotaStatusRow {
        PersistedQuotaStatusRow::new(
            account_id,
            QuotaSnapshotSource::MockEndpoint,
            route_band,
            family,
            window_label,
        )
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

    fn send_loopback_get_request_with_retry(port: u16, token: &str) -> String {
        let mut client = connect_with_retry(port);
        let request = format!(
            "GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Codex-Router-Token: {token}\r\nContent-Length: 0\r\n\r\n",
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
