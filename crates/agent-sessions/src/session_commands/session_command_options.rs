//! Standalone argument parsing and native passthrough selection.
use super::{DEFAULT_SESSION_RECORD_LIMIT, SessionsCommandError, SessionsPickerRoot};
use clap::{Parser, ValueEnum};
use std::{
    ffi::{OsStr, OsString},
    str::FromStr,
};

/// Session search root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionsRoot {
    /// Exact current working directory.
    Cwd,
    /// Current Git checkout/worktree root.
    Checkout,
    /// All linked worktrees for the current Git repository.
    Repo,
    /// All known Codex sessions.
    Any,
}

/// Provider filter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionsProvider {
    /// Include all providers.
    Any,
    /// Use the current configured Codex provider.
    Current,
    /// Match one exact provider id.
    Id(String),
}

impl FromStr for SessionsProvider {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("provider must not be empty".to_owned());
        }
        match trimmed {
            "any" => Ok(Self::Any),
            "current" => Ok(Self::Current),
            provider_id => Ok(Self::Id(provider_id.to_owned())),
        }
    }
}

/// Session source filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SessionsSource {
    /// Top-level interactive sessions only.
    Interactive,
    /// Include all sources.
    All,
    /// Include subagent sessions only.
    Subagents,
}

/// Session sort order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SessionsSort {
    /// Most recently updated first.
    Updated,
    /// Most recently created first.
    Created,
}

/// Sessions output format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SessionsFormat {
    /// Human-readable table.
    Table,
    /// JSON records.
    Json,
}

/// Parsed sessions command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionsCommand {
    /// Scope filter.
    pub root: SessionsRoot,
    /// Provider filter.
    pub provider: SessionsProvider,
    /// Source filter.
    pub source: SessionsSource,
    /// Sort order.
    pub sort: SessionsSort,
    /// Render noninteractive list output.
    pub list: bool,
    /// Output format for list mode.
    pub format: SessionsFormat,
    /// Resume the latest session matching filters.
    pub last: bool,
    /// Resume one exact Codex session UUID without loading session records.
    pub id: Option<String>,
    /// Launch a new Codex session instead of resuming one.
    pub new: bool,
    /// Launch Codex locally instead of attaching to the hosted app-server.
    pub local: bool,
    /// Maximum matching sessions to load.
    pub limit: usize,
    /// Print the command that would be launched instead of executing it.
    pub dry_run: bool,
    /// Arguments passed through to Codex after the router profile is selected.
    pub codex_args: Vec<OsString>,
}

impl From<SessionsPickerRoot> for SessionsRoot {
    fn from(root: SessionsPickerRoot) -> Self {
        match root {
            SessionsPickerRoot::Cwd => Self::Cwd,
            SessionsPickerRoot::Repo => Self::Repo,
            SessionsPickerRoot::Any => Self::Any,
        }
    }
}

impl TryFrom<SessionsRoot> for SessionsPickerRoot {
    type Error = SessionsCommandError;

    fn try_from(root: SessionsRoot) -> Result<Self, Self::Error> {
        match root {
            SessionsRoot::Cwd => Ok(Self::Cwd),
            SessionsRoot::Repo => Ok(Self::Repo),
            SessionsRoot::Any => Ok(Self::Any),
            SessionsRoot::Checkout => Err(SessionsCommandError::InteractiveCheckoutUnsupported),
        }
    }
}

impl SessionsCommand {
    pub(crate) fn parse(mut arguments: Vec<OsString>) -> Result<Self, String> {
        let passthrough_separator_index = arguments
            .iter()
            .position(|argument| argument == OsStr::new("--"));
        let router_arguments_before_passthrough = passthrough_separator_index
            .and_then(|index| arguments.get(..index))
            .map(<[OsString]>::to_vec);
        let first_argument = arguments
            .first()
            .and_then(|argument| argument.to_str())
            .map(str::to_owned);
        if let Some(first_argument) = first_argument {
            if validate_exact_uuid_session_id(&first_argument).is_ok() {
                let positional_option_end = passthrough_separator_index.unwrap_or(arguments.len());
                let positional_options =
                    arguments.get(1..positional_option_end).unwrap_or_default();
                if contains_explicit_session_id_option(positional_options) {
                    return Err("positional session UUID cannot be combined with --id".to_owned());
                }
                arguments.insert(0, OsString::from("--id"));
            } else if resembles_uuid_session_id(&first_argument) {
                validate_exact_uuid_session_id(&first_argument)?;
            }
        }
        let mut argv = Vec::with_capacity(arguments.len() + 1);
        argv.push(OsString::from("sessions"));
        argv.extend(arguments);
        let parsed =
            ClapSessionsCommand::try_parse_from(argv).map_err(|error| error.to_string())?;
        if let Some(session_id) = parsed.id.as_deref() {
            validate_exact_uuid_session_id(session_id)?;
        }
        reject_legacy_router_options(&parsed.codex_args)?;
        if parsed.id.is_none() {
            reject_misplaced_positional_session_id(
                router_arguments_before_passthrough
                    .as_deref()
                    .unwrap_or(&parsed.codex_args),
            )?;
        }
        reject_interactive_limit(&parsed)?;
        reject_interactive_checkout(&parsed)?;
        Ok(Self {
            root: parsed.root()?,
            provider: parsed.provider,
            source: parsed.source,
            sort: parsed.sort,
            list: parsed.list,
            format: parsed.format,
            last: parsed.last,
            id: parsed.id,
            new: parsed.new,
            local: parsed.local,
            limit: parsed.limit.unwrap_or(DEFAULT_SESSION_RECORD_LIMIT),
            dry_run: parsed.dry_run,
            codex_args: parsed.codex_args,
        })
    }
}

fn contains_explicit_session_id_option(arguments: &[OsString]) -> bool {
    arguments.iter().any(|argument| {
        argument == OsStr::new("--id")
            || argument
                .to_str()
                .is_some_and(|argument| argument.starts_with("--id="))
    })
}

fn reject_legacy_router_options(codex_args: &[OsString]) -> Result<(), String> {
    if codex_args
        .iter()
        .any(|argument| argument == OsStr::new("--scope"))
    {
        return Err("--scope was removed; use --checkout, --repo, or --any".to_owned());
    }
    Ok(())
}

fn reject_misplaced_positional_session_id(codex_args: &[OsString]) -> Result<(), String> {
    if codex_args.iter().any(|argument| {
        argument
            .to_str()
            .is_some_and(|argument| validate_exact_uuid_session_id(argument).is_ok())
    }) {
        return Err("session UUID must be the first argument or use --id <uuid>".to_owned());
    }
    Ok(())
}

fn resembles_uuid_session_id(argument: &str) -> bool {
    let bytes = argument.as_bytes();
    (32..=36).contains(&bytes.len())
        && bytes
            .get(..8)
            .is_some_and(|prefix| prefix.iter().all(u8::is_ascii_alphanumeric))
        && bytes.iter().any(u8::is_ascii_digit)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

fn reject_interactive_limit(command: &ClapSessionsCommand) -> Result<(), String> {
    if command.limit.is_some() && !command.list {
        return Err("--limit only applies with --list".to_owned());
    }
    Ok(())
}

fn reject_interactive_checkout(command: &ClapSessionsCommand) -> Result<(), String> {
    let opens_picker = !command.list && !command.last && command.id.is_none() && !command.new;
    if command.checkout && opens_picker {
        return Err(
            "--checkout requires --list because the interactive picker supports cwd, repo, and all"
                .to_owned(),
        );
    }
    Ok(())
}

pub(super) fn validate_exact_uuid_session_id(session_id: &str) -> Result<(), String> {
    let bytes = session_id.as_bytes();
    let is_canonical_uuid = bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes.get(index) == Some(&b'-'))
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit());
    if !is_canonical_uuid {
        return Err("--id requires a complete UUID".to_owned());
    }
    Ok(())
}

#[derive(Debug, Parser)]
#[command(name = "sessions", disable_help_subcommand = true)]
struct ClapSessionsCommand {
    #[arg(long, conflicts_with_all = ["repo", "any"])]
    checkout: bool,
    #[arg(long, conflicts_with_all = ["checkout", "any"])]
    repo: bool,
    #[arg(long, conflicts_with_all = ["checkout", "repo"])]
    any: bool,
    #[arg(long, default_value = "any")]
    provider: SessionsProvider,
    #[arg(long, value_enum, default_value = "interactive")]
    source: SessionsSource,
    #[arg(long, value_enum, default_value = "updated")]
    sort: SessionsSort,
    #[arg(long)]
    list: bool,
    #[arg(long, value_enum, default_value = "table")]
    format: SessionsFormat,
    #[arg(long)]
    last: bool,
    /// Resume one complete canonical UUID directly without opening the picker.
    #[arg(long, conflicts_with_all = ["new", "last", "list"])]
    id: Option<String>,
    #[arg(long, conflicts_with_all = ["list", "last"])]
    new: bool,
    #[arg(long)]
    local: bool,
    #[arg(long)]
    limit: Option<usize>,
    #[arg(long)]
    dry_run: bool,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    codex_args: Vec<OsString>,
}

impl ClapSessionsCommand {
    fn root(&self) -> Result<SessionsRoot, String> {
        match (self.checkout, self.repo, self.any) {
            (true, false, false) => Ok(SessionsRoot::Checkout),
            (false, true, false) => Ok(SessionsRoot::Repo),
            (false, false, true) => Ok(SessionsRoot::Any),
            (false, false, false) if !self.list && !self.last && self.id.is_none() && !self.new => {
                Ok(SessionsRoot::Repo)
            }
            (false, false, false) => Ok(SessionsRoot::Cwd),
            _ => Err("--checkout, --repo, and --any cannot be used together".to_owned()),
        }
    }
}

/// Renders the parser-owned public help text without starting native work.
pub fn command_help() -> String {
    use clap::CommandFactory;
    ClapSessionsCommand::command()
        .name("agent-sessions")
        .render_long_help()
        .to_string()
}
