//! Command vocabulary, option validation and native argument preservation.
#[path = "cli_argument_parsing/serve_command.rs"]
mod serve_command;
pub(super) use serve_command::ServeCommand;

#[cfg(test)]
#[path = "cli_argument_parsing/debug_plaintext_tests.rs"]
mod debug_plaintext_tests;

use super::{
    AccountCommand, CliError, DEFAULT_PROFILE_PORT, HostCommand, LiveCommand, QuotaCommand, Shell,
    router_secret_root_or_default,
};
use std::ffi::OsString;
use std::num::NonZeroU64;
use std::path::PathBuf;

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
    Help(&'static str),
}

impl CliCommand {
    pub(super) fn parse<I>(args: I) -> Result<Self, CliError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut parser = ArgumentParser::new(args.into_iter().collect());
        let Some(command) = parser.next_string()? else {
            return Ok(Self::Help(super::HELP_TEXT));
        };
        if is_binary_name(&command) {
            let Some(command_after_binary_name) = parser.next_string()? else {
                return Ok(Self::Help(super::HELP_TEXT));
            };
            return Self::parse_after_binary(command_after_binary_name, &mut parser);
        }

        Self::parse_after_binary(command, &mut parser)
    }

    fn parse_after_binary(command: String, parser: &mut ArgumentParser) -> Result<Self, CliError> {
        match command.as_str() {
            "serve" => {
                if parser.next_if_help()? {
                    parser.reject_remaining()?;
                    return Ok(Self::Help(serve_command::HELP_TEXT));
                }
                Ok(Self::Serve(ServeCommand::parse(parser)?))
            }
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
            "--help" | "-h" | "help" => Ok(Self::Help(super::HELP_TEXT)),
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

fn parse_nonzero_u64_option(option: &'static str, value: &str) -> Result<NonZeroU64, CliError> {
    let parsed_value = parse_u64_option(option, value)?;
    NonZeroU64::new(parsed_value).ok_or_else(|| CliError::ZeroNumericOption {
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

#[cfg(test)]
mod tests {
    use super::ArgumentParser;
    use super::ServeCommand;
    use std::ffi::OsString;

    #[test]
    fn serve_session_pin_idle_ttl_defaults_to_75_minutes_and_accepts_override() {
        let mut default_parser = ArgumentParser::new(Vec::new());
        let default_command = ServeCommand::parse(&mut default_parser)
            .unwrap_or_else(|error| panic!("default serve command should parse: {error}"));
        assert_eq!(default_command.session_pin_idle_ttl_seconds, 75 * 60);

        let arguments = [
            OsString::from("--session-pin-idle-ttl-seconds"),
            OsString::from("1800"),
        ];
        let mut configured_parser = ArgumentParser::new(arguments.into());
        let configured_command = ServeCommand::parse(&mut configured_parser)
            .unwrap_or_else(|error| panic!("configured serve command should parse: {error}"));
        assert_eq!(configured_command.session_pin_idle_ttl_seconds, 1_800);
    }

    #[test]
    fn serve_session_pin_idle_ttl_rejects_zero_during_parse() {
        let arguments = [
            OsString::from("--session-pin-idle-ttl-seconds"),
            OsString::from("0"),
        ];
        let mut parser = ArgumentParser::new(arguments.into());
        let error = ServeCommand::parse(&mut parser)
            .expect_err("session pin idle TTL must be greater than zero");

        assert!(matches!(
            error,
            super::CliError::ZeroNumericOption {
                option: "--session-pin-idle-ttl-seconds",
                value
            } if value == "0"
        ));
    }

    #[test]
    fn serve_quota_refresh_interval_accepts_values_above_three_hundred_seconds() {
        for (value, expected) in [("300", 300), ("400", 400)] {
            let arguments = [
                OsString::from("--quota-refresh-interval-seconds"),
                OsString::from(value),
            ];
            let mut parser = ArgumentParser::new(arguments.into());
            let command = ServeCommand::parse(&mut parser)
                .unwrap_or_else(|error| panic!("interval should parse: {error}"));
            assert_eq!(command.quota_refresh_interval_seconds, expected);
        }
    }

    #[test]
    fn serve_claude_five_hour_reserve_percent_defaults_and_accepts_one_through_ninety_nine() {
        let mut default_parser = ArgumentParser::new(Vec::new());
        let default_command = ServeCommand::parse(&mut default_parser)
            .unwrap_or_else(|error| panic!("default serve command should parse: {error}"));
        assert_eq!(default_command.claude_five_hour_reserve_percent.get(), 95);

        for (value, expected) in [("1", 1), ("99", 99), ("90", 90)] {
            let arguments = [
                OsString::from("--claude-five-hour-reserve-percent"),
                OsString::from(value),
            ];
            let mut parser = ArgumentParser::new(arguments.into());
            let command = ServeCommand::parse(&mut parser)
                .unwrap_or_else(|error| panic!("serve percent should parse: {error}"));
            assert_eq!(command.claude_five_hour_reserve_percent.get(), expected);
        }
    }

    #[test]
    fn serve_claude_five_hour_reserve_percent_rejects_values_outside_supported_range() {
        for value in ["0", "100", "101"] {
            let arguments = [
                OsString::from("--claude-five-hour-reserve-percent"),
                OsString::from(value),
            ];
            let mut parser = ArgumentParser::new(arguments.into());
            let error = ServeCommand::parse(&mut parser)
                .expect_err("Claude reserve percent must be between 1 and 99");

            assert!(matches!(
                error,
                super::CliError::NumericOptionOutOfRange {
                    option: "--claude-five-hour-reserve-percent",
                    value: parsed_value,
                    minimum: 1,
                    maximum: 99,
                } if parsed_value == value
            ));
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn serve_debug_claude_upstream_override_requires_isolation_marker() {
        let arguments = [
            OsString::from("--debug-claude-upstream-base-url"),
            OsString::from("http://127.0.0.1:18888"),
        ];
        let mut parser = ArgumentParser::new(arguments.into());
        let error = ServeCommand::parse(&mut parser)
            .expect_err("debug endpoint override requires isolated debug serve mode");
        assert!(matches!(
            error,
            super::CliError::MissingOption {
                option: "--require-debug-isolation"
            }
        ));

        let arguments = [
            OsString::from("--require-debug-isolation"),
            OsString::from("--debug-claude-upstream-base-url"),
            OsString::from("http://127.0.0.1:18888"),
        ];
        let mut parser = ArgumentParser::new(arguments.into());
        let command = ServeCommand::parse(&mut parser)
            .expect("isolated debug serve accepts the debug Claude endpoint override");
        assert_eq!(
            command.debug_claude_upstream_base_url.as_deref(),
            Some("http://127.0.0.1:18888")
        );
        assert!(command.require_debug_isolation);
        assert_eq!(
            command.upstream_base_url,
            super::super::DEFAULT_CHATGPT_BACKEND_BASE_URL
        );
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_serve_does_not_expose_debug_claude_override_options() {
        for arguments in [
            vec![OsString::from("--require-debug-isolation")],
            vec![
                OsString::from("--debug-claude-upstream-base-url"),
                OsString::from("http://127.0.0.1:18888"),
            ],
        ] {
            let mut parser = ArgumentParser::new(arguments);
            let error = ServeCommand::parse(&mut parser)
                .expect_err("release serve does not accept debug Claude override options");
            assert!(matches!(error, super::CliError::UnknownOption { .. }));
        }
    }
}
