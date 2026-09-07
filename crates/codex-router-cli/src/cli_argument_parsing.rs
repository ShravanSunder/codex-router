//! Command vocabulary, option validation and native argument preservation.
use super::{
    AccountCommand, CliError, DEFAULT_CHATGPT_BACKEND_BASE_URL, DEFAULT_MAX_SNAPSHOT_AGE_SECONDS,
    DEFAULT_PROFILE_PORT, DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS, HostCommand, LiveCommand,
    QuotaCommand, Shell, default_router_root, router_secret_root_or_default,
};
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CliCommand {
    Serve(ServeCommand),
    Token(TokenCommand),
    Profile(ProfileCommand),
    Account(AccountCommand),
    Quota(QuotaCommand),
    Live(LiveCommand),
    Host(HostCommand),
    Version,
    Help,
}

impl CliCommand {
    pub(super) fn parse<I>(args: I) -> Result<Self, CliError>
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
            "doctor" => {
                parser.reject_remaining()?;
                Ok(Self::Profile(ProfileCommand::Doctor))
            }
            "token" => Ok(Self::Token(TokenCommand::parse(parser)?)),
            "account" => Ok(Self::Account(AccountCommand::parse(parser)?)),
            "quota" => Ok(Self::Quota(QuotaCommand::parse(parser)?)),
            "live" => Ok(Self::Live(LiveCommand::parse(parser)?)),
            "host" => Ok(Self::Host(
                HostCommand::parse(parser.remaining_arguments())
                    .map_err(|message| CliError::HostParse { message })?,
            )),
            "--version" | "-V" | "version" => {
                parser.reject_remaining()?;
                Ok(Self::Version)
            }
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
pub(super) struct ServeCommand {
    pub(super) listen_host: String,
    pub(super) port: u16,
    pub(super) state_db: PathBuf,
    pub(super) secret_root: PathBuf,
    pub(super) upstream_base_url: String,
    pub(super) now_unix_seconds: Option<u64>,
    pub(super) max_snapshot_age_seconds: u64,
    pub(super) quota_refresh_interval_seconds: u64,
    pub(super) background_quota_refresh_enabled: bool,
    pub(super) require_local_token: bool,
    pub(super) max_connections: usize,
    pub(super) audit_file: Option<PathBuf>,
    pub(super) websocket_registry_report_file: Option<PathBuf>,
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
            now_unix_seconds: options.now_unix_seconds,
            max_snapshot_age_seconds: options
                .max_snapshot_age_seconds
                .unwrap_or(DEFAULT_MAX_SNAPSHOT_AGE_SECONDS),
            quota_refresh_interval_seconds: options
                .quota_refresh_interval_seconds
                .unwrap_or(DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS),
            background_quota_refresh_enabled: !options.disable_background_quota_refresh,
            require_local_token: options.require_local_token,
            max_connections: options.max_connections.unwrap_or(usize::MAX),
            audit_file: options.audit_file,
            websocket_registry_report_file: options.websocket_registry_report_file,
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
    require_local_token: bool,
    max_connections: Option<usize>,
    audit_file: Option<PathBuf>,
    websocket_registry_report_file: Option<PathBuf>,
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
            require_local_token: false,
            max_connections: None,
            audit_file: None,
            websocket_registry_report_file: None,
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
                "--require-local-token" => {
                    options.require_local_token = true;
                }
                "--max-connections" => {
                    let value = parser.next_required_value("--max-connections")?;
                    options.max_connections =
                        Some(parse_usize_option("--max-connections", &value)?);
                }
                "--audit-file" => {
                    let value = parser.next_required_value("--audit-file")?;
                    options.audit_file = Some(PathBuf::from(value));
                }
                "--websocket-registry-report-file" => {
                    let value = parser.next_required_value("--websocket-registry-report-file")?;
                    options.websocket_registry_report_file = Some(PathBuf::from(value));
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
pub(super) enum TokenCommand {
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
pub(super) enum ProfileCommand {
    Print {
        port: u16,
    },
    Doctor,
    Write {
        port: u16,
        codex_home: PathBuf,
        dry_run: bool,
        approve_codex_home_write: bool,
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
                } = options;
                let codex_home = codex_home.ok_or(CliError::MissingOption {
                    option: "--codex-home",
                })?;
                Ok(Self::Write {
                    port,
                    codex_home,
                    dry_run,
                    approve_codex_home_write,
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
}

impl ProfileOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self {
            port: DEFAULT_PROFILE_PORT,
            codex_home: None,
            dry_run: false,
            approve_codex_home_write: false,
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
    pub(super) fn new(arguments: Vec<OsString>) -> Self {
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

    pub(crate) fn next_if_help(&mut self) -> Result<bool, CliError> {
        let Some(argument) = self.arguments.get(self.index) else {
            return Ok(false);
        };
        let argument = argument
            .clone()
            .into_string()
            .map_err(|value| CliError::NonUtf8Argument { value })?;
        if matches!(argument.as_str(), "--help" | "-h" | "help") {
            self.index += 1;
            Ok(true)
        } else {
            Ok(false)
        }
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

    pub(crate) fn remaining_arguments(&mut self) -> Vec<OsString> {
        let remaining = self
            .arguments
            .get(self.index..)
            .map_or_else(Vec::new, <[OsString]>::to_vec);
        self.index = self.arguments.len();
        remaining
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
