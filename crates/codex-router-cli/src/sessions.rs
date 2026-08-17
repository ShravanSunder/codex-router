//! Router-owned Codex session picker command contract.

use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use clap::Parser;
use clap::ValueEnum;
use serde::Serialize;
use serde_json::Value;
use sqlx::QueryBuilder;
use sqlx::Row;
use sqlx::Sqlite;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use thiserror::Error;

use crate::CliContext;
use crate::presentation::session_picker::SessionsPickerDataQuery;
use crate::presentation::session_picker::SessionsPickerOutcome;
use crate::presentation::session_picker::SessionsPickerRecordLoader;
use crate::presentation::session_picker::SessionsPickerRequest;
use crate::presentation::session_picker::SessionsPickerRoot;
use crate::presentation::session_picker::run_sessions_picker;

const SESSION_TITLE_MAX_CHARS: usize = 96;
const SESSION_CONTEXT_MAX_CHARS: usize = 32;
const SESSION_CONVERSATION_MAX_READ_BYTES: u64 = 1024 * 1024;
const SESSION_CONVERSATION_MAX_SNIPPETS: usize = 10;
const SESSION_CONVERSATION_SNIPPET_MAX_CHARS: usize = 180;
const DEFAULT_SESSION_RECORD_LIMIT: usize = 100;
const SESSION_RECORD_PAGE_SIZE: usize = 250;

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionRecordPageCursor {
    sort_value: Option<i64>,
    session_id: String,
}

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

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionRecordQuery {
    root: SessionsRoot,
    provider: SessionsProvider,
    source: SessionsSource,
    sort: SessionsSort,
    last: bool,
    limit: usize,
    search: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionSearchExpression {
    terms: Vec<SessionSearchTerm>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SessionSearchTerm {
    Bare(String),
    SessionId(String),
    Branch(String),
    Repository(String),
}

struct SessionSearchDocument<'a> {
    session_id: &'a str,
    name: &'a str,
    title: &'a str,
    preview: &'a str,
    first_user_message: &'a str,
    branch: &'a str,
    origin: &'a str,
    cwd: &'a str,
}

impl SessionSearchExpression {
    pub(crate) fn parse(input: &str) -> Self {
        let terms = tokenize_session_search(input)
            .into_iter()
            .map(|token| {
                let normalized = token.to_lowercase();
                if let Some(value) = normalized.strip_prefix("id:") {
                    Self::term_with_value(SessionSearchTerm::SessionId, value)
                } else if let Some(value) = normalized.strip_prefix("b:") {
                    Self::term_with_value(SessionSearchTerm::Branch, value)
                } else if let Some(value) = normalized.strip_prefix("branch:") {
                    Self::term_with_value(SessionSearchTerm::Branch, value)
                } else if let Some(value) = normalized.strip_prefix("repo:") {
                    Self::term_with_value(SessionSearchTerm::Repository, value)
                } else {
                    SessionSearchTerm::Bare(normalized)
                }
            })
            .collect();
        Self { terms }
    }

    fn term_with_value(
        constructor: impl FnOnce(String) -> SessionSearchTerm,
        value: &str,
    ) -> SessionSearchTerm {
        constructor(value.to_owned())
    }

    fn matches(&self, document: &SessionSearchDocument<'_>) -> bool {
        let session_id = document.session_id.to_lowercase();
        let name = document.name.to_lowercase();
        let title = document.title.to_lowercase();
        let preview = document.preview.to_lowercase();
        let first_user_message = document.first_user_message.to_lowercase();
        let branch = document.branch.to_lowercase();
        let origin = document.origin.to_lowercase();
        let cwd = document.cwd.to_lowercase();

        self.terms.iter().all(|term| match term {
            SessionSearchTerm::Bare(value) => {
                !value.is_empty()
                    && [
                        session_id.as_str(),
                        name.as_str(),
                        title.as_str(),
                        preview.as_str(),
                        first_user_message.as_str(),
                        origin.as_str(),
                        cwd.as_str(),
                    ]
                    .iter()
                    .any(|field| field.contains(value))
            }
            SessionSearchTerm::SessionId(value) => !value.is_empty() && session_id.contains(value),
            SessionSearchTerm::Branch(value) => !value.is_empty() && branch.contains(value),
            SessionSearchTerm::Repository(value) => {
                !value.is_empty() && (origin.contains(value) || cwd.contains(value))
            }
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

fn tokenize_session_search(input: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in input.trim().chars() {
        match character {
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    terms.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
}

impl SessionRecordQuery {
    fn from_command(command: &SessionsCommand) -> Self {
        Self {
            root: command.root,
            provider: command.provider.clone(),
            source: command.source,
            sort: command.sort,
            last: command.last,
            limit: command.limit,
            search: String::new(),
        }
    }

    fn from_picker_query(query: SessionsPickerDataQuery) -> Self {
        Self {
            root: query.root.into(),
            provider: query.provider,
            source: query.source,
            sort: query.sort,
            last: false,
            limit: DEFAULT_SESSION_RECORD_LIMIT,
            search: query.search,
        }
    }
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

fn validate_exact_uuid_session_id(session_id: &str) -> Result<(), String> {
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
            (false, false, false) => Ok(SessionsRoot::Cwd),
            _ => Err("--checkout, --repo, and --any cannot be used together".to_owned()),
        }
    }
}

/// Runs the sessions command.
pub fn run_sessions_command<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
) -> Result<(), SessionsCommandError> {
    let launch_target = sessions_launch_target(&command, context)?;
    let mut runner = ProcessSessionsCommandRunner { launch_target };
    let mut picker = TerminalSessionsPicker::for_context(context);
    run_sessions_command_with_dependencies(stdout, command, context, &mut runner, &mut picker)
}

/// Runs the sessions command with injectable launch and picker dependencies.
pub(crate) fn run_sessions_command_with_dependencies<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
    runner: &mut impl SessionsCommandRunner,
    picker: &mut impl SessionsPicker,
) -> Result<(), SessionsCommandError> {
    let launch_target = sessions_launch_target(&command, context)?;
    if let Some(session_id) = command.id.as_deref() {
        return run_id_session(stdout, &command, &launch_target, runner, session_id);
    }
    if command.new {
        return run_new_session(stdout, command, &launch_target, runner);
    }
    if command.last {
        return run_last_session(stdout, command, context, &launch_target, runner);
    }
    if !command.list {
        return run_interactive_session(command, context, runner, picker);
    }
    match command.format {
        SessionsFormat::Json => write_sessions_json(stdout, command, context),
        SessionsFormat::Table => write_sessions_table(stdout, command, context),
    }
}

fn run_id_session<W: Write>(
    stdout: &mut W,
    command: &SessionsCommand,
    launch_target: &SessionsLaunchTarget,
    runner: &mut impl SessionsCommandRunner,
    session_id: &str,
) -> Result<(), SessionsCommandError> {
    if validate_exact_uuid_session_id(session_id).is_err() {
        return Err(SessionsCommandError::InvalidResumeSessionId);
    }
    if command.dry_run {
        write_codex_resume_dry_run(stdout, launch_target, &command.codex_args, session_id)?;
        return Ok(());
    }
    runner.run_codex_resume(&command.codex_args, session_id)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SessionsLaunchTarget {
    Hosted {
        app_server_socket: PathBuf,
        invoking_cwd: PathBuf,
    },
    Local {
        invoking_cwd: PathBuf,
    },
}

impl SessionsLaunchTarget {
    fn new_launch(&self, codex_args: &[OsString]) -> codex_router_codex::SessionLaunch {
        match self {
            Self::Hosted {
                app_server_socket,
                invoking_cwd,
            } => {
                codex_router_codex::SessionLaunch::new(app_server_socket, invoking_cwd, codex_args)
            }
            Self::Local { invoking_cwd } => {
                codex_router_codex::SessionLaunch::local(invoking_cwd, codex_args)
            }
        }
    }

    fn resume_launch(
        &self,
        codex_args: &[OsString],
        session_id: &str,
    ) -> codex_router_codex::SessionLaunch {
        match self {
            Self::Hosted {
                app_server_socket,
                invoking_cwd,
            } => codex_router_codex::SessionLaunch::resume(
                app_server_socket,
                invoking_cwd,
                codex_args,
                session_id,
            ),
            Self::Local { invoking_cwd } => codex_router_codex::SessionLaunch::resume_local(
                invoking_cwd,
                codex_args,
                session_id,
            ),
        }
    }
}

fn sessions_launch_target(
    command: &SessionsCommand,
    context: &CliContext,
) -> Result<SessionsLaunchTarget, SessionsCommandError> {
    let invoking_cwd = normalize_path(context.current_dir());
    if command.local {
        return Ok(SessionsLaunchTarget::Local { invoking_cwd });
    }
    let codex_paths = codex_router_codex::CodexPaths::from_codex_home(codex_home(context)?);
    let app_server_socket = crate::app_server_socket_or_default(context, &codex_paths)
        .map_err(|message| SessionsCommandError::AppServerSocket(message.to_owned()))?;
    Ok(SessionsLaunchTarget::Hosted {
        app_server_socket,
        invoking_cwd,
    })
}

fn write_sessions_json<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
) -> Result<(), SessionsCommandError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(SessionsCommandError::Runtime)?;
    let records = runtime.block_on(load_session_records(command, context))?;
    serde_json::to_writer(&mut *stdout, &records).map_err(SessionsCommandError::Json)?;
    writeln!(stdout).map_err(SessionsCommandError::Stdout)?;
    Ok(())
}

fn write_sessions_table<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
) -> Result<(), SessionsCommandError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(SessionsCommandError::Runtime)?;
    let records = runtime.block_on(load_session_records(command, context))?;
    for (index, record) in records.iter().enumerate() {
        if index > 0 {
            writeln!(stdout).map_err(SessionsCommandError::Stdout)?;
        }
        writeln!(stdout, "{}", human_session_row(record)).map_err(SessionsCommandError::Stdout)?;
    }
    Ok(())
}

async fn load_session_records(
    command: SessionsCommand,
    context: &CliContext,
) -> Result<Vec<SessionRecord>, SessionsCommandError> {
    load_session_records_for_query(SessionRecordQuery::from_command(&command), context).await
}

async fn load_session_records_for_query(
    query: SessionRecordQuery,
    context: &CliContext,
) -> Result<Vec<SessionRecord>, SessionsCommandError> {
    load_session_records_for_query_with_identity(query, context, None).await
}

async fn load_session_records_for_query_with_identity(
    query: SessionRecordQuery,
    context: &CliContext,
    repository_identity: Option<RepositoryIdentity>,
) -> Result<Vec<SessionRecord>, SessionsCommandError> {
    let root_filter = RootFilter::from_query(query.root, context, repository_identity);
    let codex_home_path = codex_home(context)?;
    let provider_filter = ProviderFilter::from_command(&query.provider, &codex_home_path)?;

    let state_database_path = codex_home_path.join("state_5.sqlite");
    let options = SqliteConnectOptions::new()
        .filename(&state_database_path)
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::from_millis(0))
        .pragma("query_only", "ON");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(SessionsCommandError::Sqlx)?;

    let mut records = Vec::new();
    let search_expression = SessionSearchExpression::parse(&query.search);
    let target_limit = if query.last { 1 } else { query.limit };
    let mut page_cursor = None;
    while target_limit == 0 || records.len() < target_limit {
        let page_size = session_record_candidate_page_size();
        let mut builder = session_record_page_query(
            &root_filter,
            &provider_filter,
            query.source,
            query.sort,
            page_size,
            page_cursor.as_ref(),
        );
        let rows = builder
            .build()
            .fetch_all(&pool)
            .await
            .map_err(SessionsCommandError::Sqlx)?;

        if rows.is_empty() {
            break;
        }
        let page_was_full = rows.len() == page_size;
        page_cursor = rows.last().map(|row| SessionRecordPageCursor {
            sort_value: match query.sort {
                SessionsSort::Created => row.get::<Option<i64>, _>("created_at_ms"),
                SessionsSort::Updated => row.get::<Option<i64>, _>("recency_at_ms"),
            },
            session_id: row.get("id"),
        });

        for row in rows {
            let source = row.get::<Option<String>, _>("source");
            let thread_source = row.get::<Option<String>, _>("thread_source");
            let cwd = row.get::<Option<String>, _>("cwd");
            let name = row.get::<Option<String>, _>("name");
            let title = row.get::<Option<String>, _>("title");
            let preview = row.get::<Option<String>, _>("preview");
            let first_user_message = row.get::<Option<String>, _>("first_user_message");
            let record = SessionRecord {
                session_id: row.get("id"),
                rollout_path: deferred_rollout_source(
                    &codex_home_path,
                    row.get::<Option<String>, _>("rollout_path").as_deref(),
                ),
                cwd,
                provider: row.get::<Option<String>, _>("model_provider"),
                model: row.get::<Option<String>, _>("model"),
                source,
                thread_source,
                git_branch: row.get::<Option<String>, _>("git_branch"),
                git_origin_url: row.get::<Option<String>, _>("git_origin_url"),
                name: name.clone(),
                title: title.clone(),
                preview: preview.clone(),
                first_user_message: first_user_message.clone(),
                display_title: display_title_from_session_fields(
                    name.as_deref(),
                    title.as_deref(),
                    preview.as_deref(),
                    first_user_message.as_deref(),
                ),
                created_at_ms: row.get::<Option<i64>, _>("created_at_ms"),
                updated_at_ms: row.get::<Option<i64>, _>("updated_at_ms"),
                recency_at_ms: row.get::<Option<i64>, _>("recency_at_ms"),
            };
            if !session_record_matches_root(&record, &root_filter)
                || !record.matches_search(&search_expression)
            {
                continue;
            }
            records.push(record);
            if target_limit != 0 && records.len() >= target_limit {
                break;
            }
        }
        if !page_was_full {
            break;
        }
    }
    pool.close().await;

    Ok(records)
}

fn session_record_candidate_page_size() -> usize {
    SESSION_RECORD_PAGE_SIZE
}

fn session_record_page_query(
    root_filter: &RootFilter,
    provider_filter: &ProviderFilter,
    source: SessionsSource,
    sort: SessionsSort,
    page_size: usize,
    page_cursor: Option<&SessionRecordPageCursor>,
) -> QueryBuilder<Sqlite> {
    let (sort_column, sort_index) = match sort {
        SessionsSort::Created => ("created_at_ms", "idx_threads_created_at_ms"),
        SessionsSort::Updated => ("recency_at_ms", "idx_threads_recency_at_ms"),
    };
    let mut builder = QueryBuilder::<Sqlite>::new(
        r#"
            SELECT
                id, rollout_path, cwd, model_provider, model, source, thread_source, git_branch,
                git_origin_url, name, title, preview, first_user_message,
                created_at_ms, updated_at_ms, recency_at_ms
            FROM threads INDEXED BY "#,
    );
    builder.push(sort_index).push(" WHERE archived = 0");
    append_session_record_filters(&mut builder, root_filter, provider_filter, source);
    if let Some(cursor) = page_cursor {
        builder.push(" AND (").push(sort_column);
        if let Some(sort_value) = cursor.sort_value {
            builder
                .push(" < ")
                .push_bind(sort_value)
                .push(" OR ")
                .push(sort_column)
                .push(" IS NULL OR (")
                .push(sort_column)
                .push(" = ")
                .push_bind(sort_value)
                .push(" AND id < ")
                .push_bind(cursor.session_id.clone())
                .push(")");
        } else {
            builder
                .push(" IS NULL AND id < ")
                .push_bind(cursor.session_id.clone());
        }
        builder.push(")");
    }
    builder
        .push(" ORDER BY ")
        .push(sort_column)
        .push(" DESC, id DESC LIMIT ")
        .push_bind(i64::try_from(page_size).unwrap_or(i64::MAX));
    builder
}

fn append_session_record_filters(
    builder: &mut QueryBuilder<Sqlite>,
    root_filter: &RootFilter,
    provider_filter: &ProviderFilter,
    source: SessionsSource,
) {
    append_root_filter(builder, root_filter);
    append_provider_filter(builder, provider_filter);
    append_source_filter(builder, source);
}

fn append_root_filter(builder: &mut QueryBuilder<Sqlite>, root_filter: &RootFilter) {
    match root_filter {
        RootFilter::Any => {}
        RootFilter::Cwd(_) => {}
        RootFilter::Checkout(checkout_root) => {
            builder.push(" AND (");
            append_path_scope_filter(builder, checkout_root);
            builder.push(")");
        }
        RootFilter::Repo(_) => {}
    }
}

fn append_path_scope_filter(builder: &mut QueryBuilder<Sqlite>, root: &Path) {
    for (index, path_value) in path_sql_values(root).into_iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder
            .push("cwd = ")
            .push_bind(path_value.clone())
            .push(" OR cwd LIKE ")
            .push_bind(path_child_like_pattern(&path_value))
            .push(" ESCAPE '\\'");
    }
}

fn append_provider_filter(builder: &mut QueryBuilder<Sqlite>, provider_filter: &ProviderFilter) {
    match provider_filter {
        ProviderFilter::Any => {}
        ProviderFilter::Id(provider_id) => {
            builder
                .push(" AND model_provider = ")
                .push_bind(provider_id.clone());
        }
    }
}

fn append_source_filter(builder: &mut QueryBuilder<Sqlite>, source: SessionsSource) {
    match source {
        SessionsSource::All => {}
        SessionsSource::Interactive => {
            builder.push(
                " AND source IN ('cli', 'vscode') \
                 AND (thread_source IS NULL OR thread_source NOT IN ('exec', 'app_server', 'subagent'))",
            );
        }
        SessionsSource::Subagents => {
            builder.push(
                " AND (thread_source = 'subagent' \
                 OR source = 'subagent' \
                 OR source LIKE ",
            );
            builder.push_bind("%subagent%").push(" ESCAPE '\\')");
        }
    }
}

fn path_sql_values(path: &Path) -> Vec<String> {
    let path = path.to_string_lossy().into_owned();
    let mut values = vec![path.clone()];
    if let Some(stripped_path) = path.strip_prefix("/private/") {
        values.push(format!("/{stripped_path}"));
    } else if path.starts_with("/var/") {
        values.push(format!("/private{path}"));
    }
    values.sort();
    values.dedup();
    values
}

fn path_child_like_pattern(path: &str) -> String {
    let path = path.trim_end_matches('/');
    format!("{}/%", escape_like(path))
}

fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn run_interactive_session(
    command: SessionsCommand,
    context: &CliContext,
    runner: &mut impl SessionsCommandRunner,
    picker: &mut impl SessionsPicker,
) -> Result<(), SessionsCommandError> {
    picker.ensure_available()?;
    let picker_root = SessionsPickerRoot::try_from(command.root)?;
    let picker_provider = command.provider.clone();
    let picker_source = command.source;
    let picker_sort = command.sort;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(SessionsCommandError::Runtime)?;
    let repository_identity = RepositoryIdentity::discover(context.current_dir());
    let records = runtime.block_on(load_session_records_for_query_with_identity(
        SessionRecordQuery::from_command(&command),
        context,
        Some(repository_identity.clone()),
    ))?;
    let request = SessionsPickerRequest {
        root: picker_root,
        provider: picker_provider,
        source: picker_source,
        sort: picker_sort,
        current_dir: normalize_path(context.current_dir()),
        repository_identity: repository_identity.clone(),
        current_provider: current_provider_for_picker(context),
        new_session_args_display: codex_args_display(&command.codex_args),
        records: records
            .iter()
            .map(SessionPickerRecord::from_record)
            .collect(),
    };
    let record_loader = session_picker_record_loader(context.clone(), repository_identity);
    let Some(outcome) = picker.select_session(request, Some(record_loader))? else {
        return Err(SessionsCommandError::PickerCanceled);
    };
    match outcome {
        SessionsPickerOutcome::ResumeSession(session_id) => {
            validate_resume_session_id(&session_id)?;
            runner.run_codex_resume(&command.codex_args, &session_id)
        }
        SessionsPickerOutcome::StartNewSession => runner.run_codex_new(&command.codex_args),
        SessionsPickerOutcome::TerminalTooNarrow => Err(SessionsCommandError::TerminalTooNarrow),
    }
}

fn session_picker_record_loader(
    context: CliContext,
    repository_identity: RepositoryIdentity,
) -> SessionsPickerRecordLoader {
    std::sync::Arc::new(move |query| {
        let record_query = SessionRecordQuery::from_picker_query(query);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime
            .block_on(load_session_records_for_query_with_identity(
                record_query,
                &context,
                Some(repository_identity.clone()),
            ))
            .map(|records| {
                records
                    .iter()
                    .map(SessionPickerRecord::from_record)
                    .collect()
            })
            .map_err(|error| error.to_string())
    })
}

fn run_last_session<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
    launch_target: &SessionsLaunchTarget,
    runner: &mut impl SessionsCommandRunner,
) -> Result<(), SessionsCommandError> {
    let dry_run = command.dry_run;
    let codex_args = command.codex_args.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(SessionsCommandError::Runtime)?;
    let mut records = runtime.block_on(load_session_records(command, context))?;
    let Some(record) = records.drain(..).next() else {
        return Err(SessionsCommandError::NoSessionsMatch);
    };
    validate_resume_session_id(&record.session_id)?;

    if dry_run {
        write_codex_resume_dry_run(stdout, launch_target, &codex_args, &record.session_id)?;
        return Ok(());
    }

    runner.run_codex_resume(&codex_args, &record.session_id)
}

fn run_new_session<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    launch_target: &SessionsLaunchTarget,
    runner: &mut impl SessionsCommandRunner,
) -> Result<(), SessionsCommandError> {
    if command.dry_run {
        write_codex_new_dry_run(stdout, launch_target, &command.codex_args)?;
        return Ok(());
    }

    runner.run_codex_new(&command.codex_args)
}

fn write_codex_new_dry_run<W: Write>(
    stdout: &mut W,
    launch_target: &SessionsLaunchTarget,
    codex_args: &[OsString],
) -> Result<(), SessionsCommandError> {
    write!(stdout, "codex").map_err(SessionsCommandError::Stdout)?;
    write_codex_args(stdout, &launch_target.new_launch(codex_args).arguments())?;
    writeln!(stdout).map_err(SessionsCommandError::Stdout)
}

fn write_codex_resume_dry_run<W: Write>(
    stdout: &mut W,
    launch_target: &SessionsLaunchTarget,
    codex_args: &[OsString],
    session_id: &str,
) -> Result<(), SessionsCommandError> {
    write!(stdout, "codex").map_err(SessionsCommandError::Stdout)?;
    write_codex_args(
        stdout,
        &launch_target
            .resume_launch(codex_args, session_id)
            .arguments(),
    )?;
    writeln!(stdout).map_err(SessionsCommandError::Stdout)
}

fn write_codex_args<W: Write>(
    stdout: &mut W,
    codex_args: &[OsString],
) -> Result<(), SessionsCommandError> {
    for argument in codex_args {
        write!(stdout, " {}", argument.to_string_lossy()).map_err(SessionsCommandError::Stdout)?;
    }
    Ok(())
}

fn codex_args_display(codex_args: &[OsString]) -> String {
    if codex_args.is_empty() {
        return String::new();
    }
    codex_args
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Interactive session picker.
pub(crate) trait SessionsPicker {
    /// Verifies the picker can run before expensive session loading.
    fn ensure_available(&self) -> Result<(), SessionsCommandError> {
        Ok(())
    }

    /// Selects one session id, or `None` when the picker was canceled.
    fn select_session(
        &mut self,
        request: SessionsPickerRequest,
        record_loader: Option<SessionsPickerRecordLoader>,
    ) -> Result<Option<SessionsPickerOutcome>, SessionsCommandError>;
}

struct TerminalSessionsPicker {
    terminal_available: bool,
}

impl TerminalSessionsPicker {
    fn for_context(context: &CliContext) -> Self {
        let forced_non_tty = context.env_var("CODEX_ROUTER_FORCE_NON_TTY").is_some();
        Self {
            terminal_available: !forced_non_tty
                && std::io::stdin().is_terminal()
                && std::io::stdout().is_terminal(),
        }
    }
}

impl SessionsPicker for TerminalSessionsPicker {
    fn ensure_available(&self) -> Result<(), SessionsCommandError> {
        if !self.terminal_available {
            return Err(SessionsCommandError::InteractiveRequiresTerminal);
        }
        Ok(())
    }

    fn select_session(
        &mut self,
        request: SessionsPickerRequest,
        record_loader: Option<SessionsPickerRecordLoader>,
    ) -> Result<Option<SessionsPickerOutcome>, SessionsCommandError> {
        run_sessions_picker(request, record_loader).map_err(SessionsCommandError::Picker)
    }
}

/// Runs a selected Codex session.
pub(crate) trait SessionsCommandRunner {
    /// Launches `codex --profile codex-router`.
    fn run_codex_new(&mut self, codex_args: &[OsString]) -> Result<(), SessionsCommandError>;

    /// Launches `codex --profile codex-router resume <session_id>`.
    fn run_codex_resume(
        &mut self,
        codex_args: &[OsString],
        session_id: &str,
    ) -> Result<(), SessionsCommandError>;
}

struct ProcessSessionsCommandRunner {
    launch_target: SessionsLaunchTarget,
}

impl SessionsCommandRunner for ProcessSessionsCommandRunner {
    fn run_codex_new(&mut self, codex_args: &[OsString]) -> Result<(), SessionsCommandError> {
        let launch = self.launch_target.new_launch(codex_args);
        let status = Command::new("codex")
            .args(launch.arguments())
            .status()
            .map_err(SessionsCommandError::CodexLaunch)?;
        if !status.success() {
            return Err(SessionsCommandError::CodexExit {
                status: status.to_string(),
            });
        }

        Ok(())
    }

    fn run_codex_resume(
        &mut self,
        codex_args: &[OsString],
        session_id: &str,
    ) -> Result<(), SessionsCommandError> {
        let launch = self.launch_target.resume_launch(codex_args, session_id);
        let status = Command::new("codex")
            .args(launch.arguments())
            .status()
            .map_err(SessionsCommandError::CodexLaunch)?;
        if !status.success() {
            return Err(SessionsCommandError::CodexExit {
                status: status.to_string(),
            });
        }

        Ok(())
    }
}

#[derive(Debug)]
enum ProviderFilter {
    Any,
    Id(String),
}

impl ProviderFilter {
    fn from_command(
        provider: &SessionsProvider,
        codex_home: &Path,
    ) -> Result<Self, SessionsCommandError> {
        match provider {
            SessionsProvider::Any => Ok(Self::Any),
            SessionsProvider::Id(provider_id) => Ok(Self::Id(provider_id.clone())),
            SessionsProvider::Current => Ok(Self::Id(resolve_current_provider(codex_home)?)),
        }
    }
}

#[derive(Debug)]
enum RootFilter {
    Any,
    Cwd(Vec<PathBuf>),
    Checkout(PathBuf),
    Repo(RepositoryIdentity),
}

impl RootFilter {
    fn from_query(
        root: SessionsRoot,
        context: &CliContext,
        repository_identity: Option<RepositoryIdentity>,
    ) -> Self {
        match root {
            SessionsRoot::Any => Self::Any,
            SessionsRoot::Cwd => Self::Cwd(path_identity_candidates(context.current_dir())),
            SessionsRoot::Checkout => {
                let identity = RepositoryIdentity::discover(context.current_dir());
                if identity.fallback_cwd.is_some() {
                    Self::Cwd(path_identity_candidates(context.current_dir()))
                } else {
                    find_worktree_root(context.current_dir()).map_or_else(
                        || Self::Cwd(path_identity_candidates(context.current_dir())),
                        Self::Checkout,
                    )
                }
            }
            SessionsRoot::Repo => Self::Repo(
                repository_identity
                    .unwrap_or_else(|| RepositoryIdentity::discover(context.current_dir())),
            ),
        }
    }
}

fn path_identity_candidates(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![path.to_path_buf(), normalize_path(path)];
    candidates.sort();
    candidates.dedup();
    candidates
}

fn codex_home(context: &CliContext) -> Result<PathBuf, SessionsCommandError> {
    codex_home_from_environment(
        context.env_var("CODEX_HOME").map(PathBuf::from),
        context.env_var("HOME").map(PathBuf::from),
    )
}

fn codex_home_from_environment(
    codex_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Result<PathBuf, SessionsCommandError> {
    if let Some(codex_home) = codex_home {
        return Ok(codex_home);
    }
    let Some(home) = home else {
        return Err(SessionsCommandError::CodexHomeUnavailable);
    };
    Ok(home.join(".codex"))
}

fn resolve_current_provider(codex_home: &Path) -> Result<String, SessionsCommandError> {
    for config_path in [
        codex_home.join("codex-router.config.toml"),
        codex_home.join("config.toml"),
    ] {
        match fs::read_to_string(&config_path) {
            Ok(content) => {
                if let Some(provider) = parse_model_provider(&content) {
                    return Ok(provider);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(SessionsCommandError::ConfigRead {
                    path: config_path,
                    source,
                });
            }
        }
    }
    Err(SessionsCommandError::CurrentProviderUnavailable)
}

fn current_provider_for_picker(context: &CliContext) -> Option<String> {
    codex_home(context)
        .ok()
        .and_then(|codex_home| resolve_current_provider(&codex_home).ok())
}

fn parse_model_provider(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "model_provider" {
            continue;
        }
        let value = value.trim();
        if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
            continue;
        }
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            continue;
        };
        return Some(value.to_owned());
    }
    None
}

fn validate_resume_session_id(session_id: &str) -> Result<(), SessionsCommandError> {
    let trimmed = session_id.trim();
    if trimmed.is_empty()
        || trimmed != session_id
        || trimmed.starts_with('-')
        || trimmed
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(SessionsCommandError::UnsafeSessionId);
    }
    Ok(())
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_error| path.to_path_buf())
}

fn find_worktree_root(current_dir: &Path) -> Option<PathBuf> {
    for ancestor in current_dir.ancestors() {
        if ancestor.join(".git").exists() {
            return Some(normalize_path(ancestor));
        }
    }
    None
}

fn checkout_root(current_dir: &Path) -> PathBuf {
    find_worktree_root(current_dir).unwrap_or_else(|| normalize_path(current_dir))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepositoryIdentity {
    pub(crate) normalized_origin: Option<String>,
    pub(crate) live_roots: Vec<PathBuf>,
    pub(crate) repository_basename: String,
    pub(crate) fallback_cwd: Option<PathBuf>,
}

impl RepositoryIdentity {
    fn discover(current_dir: &Path) -> Self {
        let current_checkout = checkout_root(current_dir);
        let discovered_live_roots = repo_roots(current_dir);
        let raw_origin = git_stdout(current_dir, &["remote", "get-url", "origin"]);
        let normalized_origin = raw_origin.as_deref().and_then(normalize_git_origin_url);
        let git_common_dir = git_stdout(
            current_dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .map(PathBuf::from);
        let has_git_repository_evidence = normalized_origin.is_some()
            || git_common_dir.is_some()
            || !discovered_live_roots.is_empty();
        let live_roots = live_roots_with_current_checkout_fallback(
            discovered_live_roots,
            &current_checkout,
            has_git_repository_evidence,
        );
        let primary_worktree = live_roots.first().map(PathBuf::as_path);
        let repository_basename = if has_git_repository_evidence {
            repository_basename_from_evidence(
                normalized_origin.as_deref(),
                git_common_dir.as_deref(),
                primary_worktree,
                &current_checkout,
            )
        } else {
            String::new()
        };
        Self {
            normalized_origin,
            live_roots,
            repository_basename,
            fallback_cwd: (!has_git_repository_evidence).then(|| normalize_path(current_dir)),
        }
    }
}

fn live_roots_with_current_checkout_fallback(
    mut live_roots: Vec<PathBuf>,
    current_checkout: &Path,
    has_git_repository_evidence: bool,
) -> Vec<PathBuf> {
    if has_git_repository_evidence && live_roots.is_empty() {
        live_roots.push(current_checkout.to_path_buf());
    }
    live_roots
}

fn git_stdout(current_dir: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(current_dir)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    non_empty_trimmed(&value).map(str::to_owned)
}

fn normalize_git_origin_url(origin: &str) -> Option<String> {
    let origin = origin
        .trim()
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    if origin.is_empty() {
        return None;
    }
    let (host, repository_path) = if let Some((_scheme, remainder)) = origin.split_once("://") {
        let remainder = remainder
            .rsplit_once('@')
            .map_or(remainder, |(_, value)| value);
        remainder.split_once('/')?
    } else if let Some((host_with_user, repository_path)) = origin.split_once(':') {
        let host = host_with_user
            .rsplit_once('@')
            .map_or(host_with_user, |(_, value)| value);
        (host, repository_path)
    } else {
        let (host, repository_path) = origin.split_once('/')?;
        if !host.contains('.') {
            return None;
        }
        (host, repository_path)
    };
    let host = host.trim().to_lowercase();
    let repository_path = repository_path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or(repository_path.trim_matches('/'));
    if host.is_empty() || repository_path.is_empty() {
        return None;
    }
    Some(format!("{host}/{repository_path}"))
}

fn repository_basename_from_evidence(
    normalized_origin: Option<&str>,
    git_common_dir: Option<&Path>,
    primary_worktree: Option<&Path>,
    current_checkout: &Path,
) -> String {
    normalized_origin
        .and_then(|origin| origin.rsplit('/').next())
        .or_else(|| {
            git_common_dir.and_then(|common_dir| {
                if common_dir.file_name() == Some(OsStr::new(".git")) {
                    common_dir.parent().and_then(Path::file_name)
                } else {
                    common_dir.file_name()
                }
                .and_then(OsStr::to_str)
            })
        })
        .or_else(|| {
            primary_worktree
                .and_then(Path::file_name)
                .and_then(OsStr::to_str)
        })
        .or_else(|| current_checkout.file_name().and_then(OsStr::to_str))
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn session_belongs_to_repository(
    identity: &RepositoryIdentity,
    row_origin: Option<&str>,
    cwd: &Path,
) -> bool {
    if let Some(fallback_cwd) = &identity.fallback_cwd {
        return normalized_paths_resolve_to_same_location(cwd, fallback_cwd);
    }
    let row_origin = row_origin.and_then(non_empty_trimmed);
    let normalized_row_origin = row_origin.and_then(normalize_git_origin_url);
    if let (Some(current_origin), Some(_)) = (&identity.normalized_origin, row_origin) {
        return normalized_row_origin.is_some_and(|row_origin| row_origin == *current_origin);
    }
    let is_under_live_root = identity
        .live_roots
        .iter()
        .any(|root| path_is_equal_or_child_for_repo(cwd, root));
    let matches_historical_basename = !identity.repository_basename.is_empty()
        && cwd.file_name().and_then(OsStr::to_str).is_some_and(|leaf| {
            leaf == identity.repository_basename
                || leaf
                    .strip_prefix(&identity.repository_basename)
                    .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('-'))
        });

    match (&identity.normalized_origin, row_origin) {
        (Some(_), Some(_)) => false,
        (Some(_), None) | (None, None) => is_under_live_root || matches_historical_basename,
        (None, Some(_)) => is_under_live_root,
    }
}

fn path_is_equal_or_child_for_repo(candidate: &Path, parent: &Path) -> bool {
    path_sql_values(candidate).into_iter().any(|candidate| {
        path_sql_values(parent).into_iter().any(|parent| {
            candidate == parent
                || candidate
                    .strip_prefix(&parent)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    })
}

fn repo_roots(current_dir: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(current_dir)
        .arg("worktree")
        .arg("list")
        .arg("--porcelain")
        .output();
    if let Ok(output) = output
        && output.status.success()
        && let Ok(stdout) = String::from_utf8(output.stdout)
    {
        let roots = parse_git_worktree_roots(&stdout);
        if !roots.is_empty() {
            return roots;
        }
    }
    Vec::new()
}

fn parse_git_worktree_roots(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .map(|path| normalize_path(&path))
        .collect()
}

fn deferred_rollout_source(
    codex_home_path: &Path,
    rollout_path: Option<&str>,
) -> Option<SessionConversationSource> {
    let rollout_path = rollout_path.and_then(non_empty_trimmed)?;
    let path = Path::new(rollout_path);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    let session_history_root = codex_home_path.join("sessions");
    if !path.starts_with(&session_history_root) {
        return None;
    }
    Some(SessionConversationSource {
        rollout_path: rollout_path.to_owned(),
        codex_home_path: codex_home_path.to_path_buf(),
    })
}

fn validated_rollout_path(codex_home_path: &Path, rollout_path: Option<&str>) -> Option<String> {
    let rollout_path = rollout_path.and_then(non_empty_trimmed)?;
    let path = Path::new(rollout_path);
    let Ok(canonical_path) = path.canonicalize() else {
        return None;
    };
    let session_history_root = codex_home_path.join("sessions");
    let trusted_root = session_history_root
        .canonicalize()
        .or_else(|_| codex_home_path.canonicalize())
        .ok()?;
    if !canonical_path.starts_with(&trusted_root) {
        return None;
    }
    Some(canonical_path.display().to_string())
}

#[derive(Debug, Serialize)]
struct SessionRecord {
    session_id: String,
    #[serde(skip)]
    rollout_path: Option<SessionConversationSource>,
    #[serde(skip)]
    display_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_branch: Option<String>,
    #[serde(skip)]
    git_origin_url: Option<String>,
    #[serde(skip)]
    name: Option<String>,
    #[serde(skip)]
    title: Option<String>,
    #[serde(skip)]
    preview: Option<String>,
    #[serde(skip)]
    first_user_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recency_at_ms: Option<i64>,
}

impl SessionRecord {
    fn display_title(&self) -> &str {
        self.display_title.as_deref().unwrap_or("Untitled session")
    }

    fn branch(&self) -> &str {
        self.git_branch.as_deref().unwrap_or("-")
    }

    fn matches_search(&self, expression: &SessionSearchExpression) -> bool {
        let normalized_origin = self
            .git_origin_url
            .as_deref()
            .and_then(normalize_git_origin_url)
            .unwrap_or_default();
        expression.matches(&SessionSearchDocument {
            session_id: &self.session_id,
            name: self.name.as_deref().unwrap_or_default(),
            title: self.title.as_deref().unwrap_or_default(),
            preview: self.preview.as_deref().unwrap_or_default(),
            first_user_message: self.first_user_message.as_deref().unwrap_or_default(),
            branch: self.git_branch.as_deref().unwrap_or_default(),
            origin: &normalized_origin,
            cwd: self.cwd.as_deref().unwrap_or_default(),
        })
    }
}

fn session_record_matches_root(record: &SessionRecord, root_filter: &RootFilter) -> bool {
    match root_filter {
        RootFilter::Cwd(current_dirs) => record.cwd.as_deref().is_some_and(|cwd| {
            current_dirs
                .iter()
                .any(|current_dir| paths_resolve_to_same_location(Path::new(cwd), current_dir))
        }),
        RootFilter::Repo(identity) => record.cwd.as_deref().is_some_and(|cwd| {
            let normalized_cwd = normalize_path(Path::new(cwd));
            session_belongs_to_repository(
                identity,
                record.git_origin_url.as_deref(),
                &normalized_cwd,
            )
        }),
        RootFilter::Any | RootFilter::Checkout(_) => true,
    }
}

pub(crate) fn paths_resolve_to_same_location(left: &Path, right: &Path) -> bool {
    let left = normalize_path(left);
    let right = normalize_path(right);
    normalized_paths_resolve_to_same_location(&left, &right)
}

pub(crate) fn normalized_paths_resolve_to_same_location(left: &Path, right: &Path) -> bool {
    path_sql_values(left)
        .iter()
        .any(|left| path_sql_values(right).contains(left))
}

/// Picker display row for one session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionPickerRecord {
    pub(crate) session_id: String,
    pub(crate) title: String,
    pub(crate) full_title: String,
    pub(crate) explicit_name: Option<String>,
    pub(crate) recency: String,
    pub(crate) created: String,
    pub(crate) recency_at_ms: Option<i64>,
    pub(crate) created_at_ms: Option<i64>,
    pub(crate) branch: String,
    pub(crate) persisted_branch: String,
    pub(crate) context: String,
    pub(crate) cwd: Option<String>,
    pub(crate) normalized_cwd: Option<String>,
    pub(crate) git_origin_url: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) preview: Option<String>,
    pub(crate) first_user_message: String,
    pub(crate) conversation: SessionConversationPreview,
    pub(crate) conversation_source: Option<SessionConversationSource>,
    pub(crate) source: Option<String>,
    pub(crate) thread_source: Option<String>,
}

/// Sanitized conversation snippets for human-only session detail UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionConversationPreview {
    pub(crate) snippets: Vec<String>,
    pub(crate) unavailable_reason: Option<String>,
}

/// Deferred, validated-on-read conversation history source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionConversationSource {
    rollout_path: String,
    codex_home_path: PathBuf,
}

#[cfg(test)]
impl SessionConversationSource {
    pub(crate) fn for_test(rollout_path: impl Into<String>, codex_home_path: PathBuf) -> Self {
        Self {
            rollout_path: rollout_path.into(),
            codex_home_path,
        }
    }
}

impl SessionPickerRecord {
    pub(crate) fn matches_search(&self, expression: &SessionSearchExpression) -> bool {
        let normalized_origin = self
            .git_origin_url
            .as_deref()
            .and_then(normalize_git_origin_url)
            .unwrap_or_default();
        expression.matches(&SessionSearchDocument {
            session_id: &self.session_id,
            name: self.explicit_name.as_deref().unwrap_or_default(),
            title: &self.full_title,
            preview: self.preview.as_deref().unwrap_or_default(),
            first_user_message: &self.first_user_message,
            branch: &self.persisted_branch,
            origin: &normalized_origin,
            cwd: self.cwd.as_deref().unwrap_or_default(),
        })
    }

    fn from_record(record: &SessionRecord) -> Self {
        Self {
            session_id: record.session_id.clone(),
            title: record.display_title().to_owned(),
            explicit_name: record.name.clone(),
            full_title: record.title.clone().unwrap_or_default(),
            recency: format_recency_at_ms(record.recency_at_ms),
            created: format_recency_at_ms(record.created_at_ms),
            recency_at_ms: record.recency_at_ms,
            created_at_ms: record.created_at_ms,
            branch: record.branch().to_owned(),
            persisted_branch: record.git_branch.clone().unwrap_or_default(),
            context: record
                .cwd
                .as_deref()
                .map(session_context_from_cwd)
                .unwrap_or_else(|| "-".to_owned()),
            cwd: record.cwd.clone(),
            normalized_cwd: record.cwd.as_deref().map(|cwd| {
                normalize_path(Path::new(cwd))
                    .to_string_lossy()
                    .into_owned()
            }),
            git_origin_url: record.git_origin_url.clone(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            preview: record.preview.clone(),
            first_user_message: record.first_user_message.clone().unwrap_or_default(),
            conversation: SessionConversationPreview::unavailable("history not loaded"),
            conversation_source: record.rollout_path.clone(),
            source: record.source.clone(),
            thread_source: record.thread_source.clone(),
        }
    }
}

impl SessionConversationPreview {
    pub(crate) fn from_rollout_source(source: Option<&SessionConversationSource>) -> Self {
        let Some(source) = source else {
            return Self::unavailable("history unavailable");
        };
        let Some(path) =
            validated_rollout_path(&source.codex_home_path, Some(&source.rollout_path))
        else {
            return Self::unavailable("history unavailable");
        };
        Self::from_rollout_path(Some(&path))
    }

    pub(crate) fn from_rollout_path(rollout_path: Option<&str>) -> Self {
        let Some(rollout_path) = rollout_path.and_then(non_empty_trimmed) else {
            return Self::unavailable("history unavailable");
        };
        let path = Path::new(rollout_path);
        if !path.is_file() {
            return Self::unavailable("history unavailable");
        }

        let Ok(text) = read_history_tail(path) else {
            return Self::unavailable("history unavailable");
        };
        let snippets = extract_recent_conversation_snippets(&text);
        if snippets.is_empty() {
            return Self::unavailable("no recent messages");
        }
        Self {
            snippets,
            unavailable_reason: None,
        }
    }

    pub(crate) fn unavailable(reason: &str) -> Self {
        Self {
            snippets: Vec::new(),
            unavailable_reason: Some(reason.to_owned()),
        }
    }
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn read_history_tail(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    let file_len = metadata.len();
    let start = file_len.saturating_sub(SESSION_CONVERSATION_MAX_READ_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let tail_len = file_len - start;
    let mut bytes = Vec::with_capacity(tail_len as usize);
    file.take(SESSION_CONVERSATION_MAX_READ_BYTES)
        .read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    if start > 0
        && let Some((_, remaining)) = text.split_once('\n')
    {
        return Ok(remaining.to_owned());
    }
    Ok(text.into_owned())
}

fn extract_recent_conversation_snippets(text: &str) -> Vec<String> {
    let mut recent_messages = Vec::new();
    let mut latest_user_message = None;
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(message) = conversation_message_from_event(&event) else {
            continue;
        };
        if message.is_user {
            latest_user_message = Some(message.clone());
        }
        recent_messages.push(message);
        if recent_messages.len() > SESSION_CONVERSATION_MAX_SNIPPETS {
            recent_messages.remove(0);
        }
    }
    if !recent_messages.iter().any(|message| message.is_user)
        && let Some(latest_user_message) = latest_user_message
    {
        if recent_messages.len() == SESSION_CONVERSATION_MAX_SNIPPETS {
            recent_messages.remove(0);
        }
        recent_messages.insert(0, latest_user_message);
    }
    recent_messages
        .into_iter()
        .map(|message| message.snippet)
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConversationMessage {
    snippet: String,
    is_user: bool,
}

fn conversation_message_from_event(event: &Value) -> Option<ConversationMessage> {
    if event.get("type").and_then(Value::as_str) != Some("response_item") {
        return None;
    }
    let payload = event.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let role = payload.get("role").and_then(Value::as_str)?;
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    let content = payload.get("content")?;
    let mut fragments = Vec::new();
    collect_text_fragments(content, &mut fragments);
    let text = fragments.join(" ");
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalized.trim();
    if normalized.is_empty() || is_control_conversation_text(normalized) {
        return None;
    }
    Some(ConversationMessage {
        snippet: truncate_end(normalized, SESSION_CONVERSATION_SNIPPET_MAX_CHARS),
        is_user: role == "user",
    })
}

fn collect_text_fragments(value: &Value, fragments: &mut Vec<String>) {
    match value {
        Value::String(text) => fragments.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_text_fragments(item, fragments);
            }
        }
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                fragments.push(text.to_owned());
                return;
            }
            for key in ["content", "output_text"] {
                if let Some(value) = object.get(key) {
                    collect_text_fragments(value, fragments);
                }
            }
        }
        _ => {}
    }
}

fn is_control_conversation_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    text.chars().count() > 5_000
        || lower.contains("agents.md instructions")
        || lower.contains("# agents.md")
        || lower.contains("<instructions>")
        || lower.contains("</instructions>")
        || lower.contains("<hook_prompt")
        || lower.contains("hook_run_id=")
        || lower.contains("<turn_aborted>")
        || lower.contains("<environment_context>")
        || lower.contains("<permissions instructions>")
        || lower.contains("<skills_instructions>")
        || lower.contains("<plugins_instructions>")
        || lower.contains("<subagent_notification>")
        || lower.contains("<user_instructions>")
        || lower.contains("<developer_instructions>")
        || lower.contains("<system_instructions>")
        || lower.contains("<context_summary>")
        || lower.contains("<tool_call>")
        || lower.contains("filesystem sandboxing")
        || lower.contains("available tools and usage guidelines")
        || lower.contains("the following is the codex agent history")
        || lower.contains("tool call arguments")
        || lower.contains(">>> transcript start")
        || lower.contains("transcript start")
        || lower.contains("transcript end")
        || lower.contains("review only p0-p2")
        || lower.contains("read-only implementation review")
        || lower.contains("scope: current uncommitted diff")
        || lower.contains("knowledge cutoff:")
        || lower.contains("you are codex")
        || lower.contains("available skills")
        || lower.contains("response_item")
        || lower.contains("api_key")
        || lower.contains("access_token")
        || lower.contains("refresh_token")
}

fn human_session_row(record: &SessionRecord) -> String {
    let context = record
        .cwd
        .as_deref()
        .map(session_context_from_cwd)
        .unwrap_or_else(|| "-".to_owned());
    format!(
        "{}\n  {}  {}  {}  id={}",
        record.display_title(),
        format_recency_at_ms(record.recency_at_ms),
        record.branch(),
        context,
        short_session_id(&record.session_id)
    )
}

fn session_context_from_cwd(cwd: &str) -> String {
    let leaf = Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(cwd);
    truncate_middle(leaf, SESSION_CONTEXT_MAX_CHARS)
}

fn display_title_from_session_fields(
    name: Option<&str>,
    title: Option<&str>,
    preview: Option<&str>,
    first_user_message: Option<&str>,
) -> Option<String> {
    let explicit_name = name.and_then(normalize_display_title);
    let derived_title = [title, preview, first_user_message]
        .into_iter()
        .flatten()
        .find_map(normalize_display_title);
    match (explicit_name, derived_title) {
        (Some(name), Some(derived_title)) => Some(truncate_end(
            &format!("{name} | {derived_title}"),
            SESSION_TITLE_MAX_CHARS,
        )),
        (Some(name), None) | (None, Some(name)) => Some(name),
        (None, None) => None,
    }
}

fn normalize_display_title(value: &str) -> Option<String> {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return None;
    }
    Some(truncate_end(&compact, SESSION_TITLE_MAX_CHARS))
}

fn truncate_end(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}…", value.chars().take(keep).collect::<String>())
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    let side = max_chars.saturating_sub(1) / 2;
    let prefix = value.chars().take(side).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(side)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}…{suffix}")
}

fn short_session_id(session_id: &str) -> String {
    truncate_end(session_id, 8)
}

fn format_recency_at_ms(recency_at_ms: Option<i64>) -> String {
    let Some(recency_at_ms) = recency_at_ms else {
        return "-".to_owned();
    };
    if recency_at_ms < 0 {
        return "-".to_owned();
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let recency_at_ms = recency_at_ms as u128;
    if now_ms >= recency_at_ms {
        let duration = format_duration_ms(now_ms - recency_at_ms);
        if duration == "now" {
            duration
        } else {
            format!("{duration} ago")
        }
    } else {
        let duration = format_duration_ms(recency_at_ms - now_ms);
        if duration == "now" {
            duration
        } else {
            format!("in {duration}")
        }
    }
}

fn format_duration_ms(duration_ms: u128) -> String {
    let seconds = duration_ms / 1_000;
    if seconds < 60 {
        return "now".to_owned();
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    format!("{days}d")
}

#[cfg(test)]
mod tests {
    use super::RepositoryIdentity;
    use super::RootFilter;
    use super::SESSION_CONVERSATION_MAX_READ_BYTES;
    use super::SessionConversationPreview;
    use super::SessionPickerRecord;
    use super::SessionRecord;
    use super::SessionSearchDocument;
    use super::SessionSearchExpression;
    use super::SessionsCommand;
    use super::SessionsRoot;
    use super::codex_home_from_environment;
    use super::deferred_rollout_source;
    use super::extract_recent_conversation_snippets;
    use super::format_duration_ms;
    use super::live_roots_with_current_checkout_fallback;
    use super::normalize_git_origin_url;
    use super::repository_basename_from_evidence;
    use super::session_belongs_to_repository;
    use super::validated_rollout_path;
    use serde_json::json;
    use sqlx::Execute;
    use std::fs;
    use std::path::PathBuf;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    fn search_consistency_record(
        git_origin_url: Option<&str>,
        first_user_message: Option<&str>,
    ) -> SessionRecord {
        SessionRecord {
            session_id: "thread-search-consistency".to_owned(),
            rollout_path: None,
            display_title: Some("deploy rollback plan".to_owned()),
            cwd: Some("/history/app.impl-search".to_owned()),
            provider: Some("codex-router".to_owned()),
            model: None,
            source: Some("cli".to_owned()),
            thread_source: None,
            git_branch: Some("main".to_owned()),
            git_origin_url: git_origin_url.map(str::to_owned),
            name: None,
            title: None,
            preview: None,
            first_user_message: first_user_message.map(str::to_owned),
            created_at_ms: Some(1),
            updated_at_ms: Some(1),
            recency_at_ms: Some(1),
        }
    }

    #[test]
    fn session_record_pages_use_keyset_order_index_without_offset() {
        let mut first_page_builder = super::session_record_page_query(
            &super::RootFilter::Any,
            &super::ProviderFilter::Any,
            super::SessionsSource::All,
            super::SessionsSort::Updated,
            super::SESSION_RECORD_PAGE_SIZE,
            None,
        );
        let first_page_sql = first_page_builder.build().sql().as_str().to_owned();
        let cursor = super::SessionRecordPageCursor {
            sort_value: Some(42),
            session_id: "thread-cursor".to_owned(),
        };
        let mut later_page_builder = super::session_record_page_query(
            &super::RootFilter::Any,
            &super::ProviderFilter::Any,
            super::SessionsSource::All,
            super::SessionsSort::Updated,
            super::SESSION_RECORD_PAGE_SIZE,
            Some(&cursor),
        );
        let later_page_sql = later_page_builder.build().sql().as_str().to_owned();

        assert!(first_page_sql.contains("INDEXED BY idx_threads_recency_at_ms"));
        assert!(!first_page_sql.contains("OFFSET"));
        assert!(later_page_sql.contains("recency_at_ms <"));
        assert!(later_page_sql.contains("recency_at_ms ="));
        assert!(later_page_sql.contains("id <"));
        assert!(!later_page_sql.contains("OFFSET"));

        let null_cursor = super::SessionRecordPageCursor {
            sort_value: None,
            session_id: "thread-null-cursor".to_owned(),
        };
        let mut created_page_builder = super::session_record_page_query(
            &super::RootFilter::Any,
            &super::ProviderFilter::Any,
            super::SessionsSource::All,
            super::SessionsSort::Created,
            super::SESSION_RECORD_PAGE_SIZE,
            Some(&null_cursor),
        );
        let created_page_sql = created_page_builder.build().sql().as_str().to_owned();
        assert!(created_page_sql.contains("INDEXED BY idx_threads_created_at_ms"));
        assert!(created_page_sql.contains("created_at_ms IS NULL AND id <"));
        assert!(!created_page_sql.contains("OFFSET"));
    }

    #[cfg(unix)]
    #[test]
    fn cwd_query_candidates_preserve_raw_symlink_and_canonical_paths() {
        let fixture_root = std::env::temp_dir().join(format!(
            "codex-router-session-cwd-query-candidates-{}",
            std::process::id()
        ));
        let canonical_checkout = fixture_root.join("canonical-checkout");
        let checkout_alias = fixture_root.join("checkout-alias");
        fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
        std::os::unix::fs::symlink(&canonical_checkout, &checkout_alias)
            .expect("create checkout symlink");

        let candidates = super::path_identity_candidates(&checkout_alias);
        let canonical_path = fs::canonicalize(&canonical_checkout).expect("canonicalize checkout");

        fs::remove_file(&checkout_alias).expect("remove checkout symlink");
        fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
        fs::remove_dir(&fixture_root).expect("remove fixture root");
        assert_eq!(candidates.len(), 2);
        assert!(candidates.contains(&checkout_alias));
        assert!(candidates.contains(&canonical_path));
    }

    #[cfg(unix)]
    #[test]
    fn cwd_scope_defers_all_symlink_spellings_to_the_final_matcher() {
        let fixture_root = std::env::temp_dir().join(format!(
            "codex-router-session-cwd-final-matcher-{}",
            std::process::id()
        ));
        let canonical_parent = fixture_root.join("canonical-parent");
        let canonical_checkout = canonical_parent.join("checkout");
        let first_alias = fixture_root.join("first-alias");
        let second_alias = fixture_root.join("second-alias");
        fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
        std::os::unix::fs::symlink(&canonical_parent, &first_alias).expect("create first alias");
        std::os::unix::fs::symlink(&canonical_parent, &second_alias).expect("create second alias");
        let current_dir = first_alias.join("checkout");
        let persisted_cwd = second_alias.join("checkout");
        let root_filter = super::RootFilter::Cwd(super::path_identity_candidates(&current_dir));
        let mut record = search_consistency_record(None, None);
        record.cwd = Some(persisted_cwd.display().to_string());
        let mut query = super::session_record_page_query(
            &root_filter,
            &super::ProviderFilter::Any,
            super::SessionsSource::All,
            super::SessionsSort::Updated,
            super::SESSION_RECORD_PAGE_SIZE,
            None,
        );
        let query_sql = query.build().sql().as_str().to_owned();
        let record_matches = super::session_record_matches_root(&record, &root_filter);

        fs::remove_file(&first_alias).expect("remove first alias");
        fs::remove_file(&second_alias).expect("remove second alias");
        fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
        fs::remove_dir(&canonical_parent).expect("remove canonical parent");
        fs::remove_dir(&fixture_root).expect("remove fixture root");
        assert!(!query_sql.contains("cwd ="));
        assert!(record_matches);
    }

    #[test]
    fn session_record_candidate_pages_do_not_shrink_to_the_remaining_match_limit() {
        assert_eq!(super::session_record_candidate_page_size(), 250);
    }

    #[test]
    fn git_origin_normalization_compares_common_transport_forms() {
        let expected = Some("github.com/shravan-agent/codex-router".to_owned());

        assert_eq!(
            normalize_git_origin_url("https://github.com/shravan-agent/codex-router.git"),
            expected
        );
        assert_eq!(
            normalize_git_origin_url("git@github.com:shravan-agent/codex-router.git"),
            expected
        );
        assert_eq!(
            normalize_git_origin_url("ssh://git@github.com/shravan-agent/codex-router/"),
            expected
        );
        assert_eq!(
            normalize_git_origin_url(
                "https://user:secret@GitHub.com/shravan-agent/codex-router.git?token=secret#branch",
            ),
            expected
        );
        assert_eq!(
            normalize_git_origin_url("github.com/shravan-agent/codex-router"),
            expected
        );
    }

    #[test]
    fn picker_repository_matching_normalizes_persisted_origin_exactly_once() {
        let raw_origin = "http://gitlab.internal:8443/team/app.git";
        let identity = RepositoryIdentity {
            normalized_origin: normalize_git_origin_url(raw_origin),
            live_roots: vec![PathBuf::from("/dev/app")],
            repository_basename: "app".to_owned(),
            fallback_cwd: None,
        };
        let record = search_consistency_record(Some(raw_origin), None);
        let picker_record = SessionPickerRecord::from_record(&record);

        assert!(session_belongs_to_repository(
            &identity,
            picker_record.git_origin_url.as_deref(),
            std::path::Path::new(picker_record.cwd.as_deref().expect("record cwd")),
        ));
    }

    #[test]
    fn picker_search_uses_the_same_complete_persisted_fields_as_loader_search() {
        let record = search_consistency_record(None, Some("deploy\nrollback plan"));
        let picker_record = SessionPickerRecord::from_record(&record);
        let expression = SessionSearchExpression::parse("\"deploy rollback\"");

        assert_eq!(
            picker_record.matches_search(&expression),
            record.matches_search(&expression)
        );
        assert!(!picker_record.matches_search(&expression));
    }

    #[test]
    fn explicit_session_name_precedes_derived_title_and_is_searchable() {
        let mut record = search_consistency_record(None, None);
        record.name = Some("Website stuff".to_owned());
        record.title = Some("Derived first-message title".to_owned());
        record.display_title = super::display_title_from_session_fields(
            record.name.as_deref(),
            record.title.as_deref(),
            record.preview.as_deref(),
            record.first_user_message.as_deref(),
        );

        let picker_record = SessionPickerRecord::from_record(&record);

        assert_eq!(
            picker_record.title,
            "Website stuff | Derived first-message title"
        );
        for query in ["website stuff", "derived first-message"] {
            let expression = SessionSearchExpression::parse(query);
            assert!(record.matches_search(&expression), "loader search: {query}");
            assert!(
                picker_record.matches_search(&expression),
                "picker search: {query}"
            );
        }
    }

    #[test]
    fn missing_explicit_session_name_keeps_the_previous_display_fallback() {
        let mut record = search_consistency_record(None, Some("fallback first message"));
        record.title = Some("previous title".to_owned());
        record.display_title = super::display_title_from_session_fields(
            record.name.as_deref(),
            record.title.as_deref(),
            record.preview.as_deref(),
            record.first_user_message.as_deref(),
        );
        let picker_record = SessionPickerRecord::from_record(&record);

        assert_eq!(picker_record.title, "previous title");
    }

    #[cfg(unix)]
    #[test]
    fn picker_search_preserves_raw_persisted_cwd_for_loader_parity() {
        let fixture_root = std::env::temp_dir().join(format!(
            "codex-router-session-picker-search-cwd-{}",
            std::process::id()
        ));
        let canonical_checkout = fixture_root.join("canonical-checkout");
        let checkout_alias = fixture_root.join("checkout-alias");
        fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
        std::os::unix::fs::symlink(&canonical_checkout, &checkout_alias)
            .expect("create checkout symlink");
        let mut record = search_consistency_record(None, None);
        record.cwd = Some(checkout_alias.display().to_string());
        let picker_record = SessionPickerRecord::from_record(&record);
        let alias_expression = SessionSearchExpression::parse("checkout-alias");
        let canonical_expression = SessionSearchExpression::parse("canonical-checkout");
        let actual = [
            picker_record.matches_search(&alias_expression),
            picker_record.matches_search(&canonical_expression),
        ];
        let expected = [
            record.matches_search(&alias_expression),
            record.matches_search(&canonical_expression),
        ];
        fs::remove_file(&checkout_alias).expect("remove checkout symlink");
        fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
        fs::remove_dir(&fixture_root).expect("remove fixture root");

        assert_eq!(actual, expected);
    }

    #[test]
    fn picker_search_does_not_match_missing_branch_display_placeholder() {
        let mut record = search_consistency_record(None, None);
        record.git_branch = None;
        let picker_record = SessionPickerRecord::from_record(&record);
        let expression = SessionSearchExpression::parse("b:-");

        assert_eq!(
            picker_record.matches_search(&expression),
            record.matches_search(&expression)
        );
        assert!(!picker_record.matches_search(&expression));
    }

    #[cfg(unix)]
    #[test]
    fn picker_record_normalizes_existing_cwd_before_interactive_matching() {
        let fixture_root = std::env::temp_dir().join(format!(
            "codex-router-session-picker-path-normalization-{}",
            std::process::id()
        ));
        let canonical_checkout = fixture_root.join("canonical-checkout");
        let checkout_alias = fixture_root.join("checkout-alias");
        fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
        std::os::unix::fs::symlink(&canonical_checkout, &checkout_alias)
            .expect("create checkout symlink");
        let mut record = search_consistency_record(None, None);
        record.cwd = Some(checkout_alias.display().to_string());

        let picker_record = SessionPickerRecord::from_record(&record);
        let actual_cwd = picker_record.normalized_cwd;
        let expected_cwd = fs::canonicalize(&canonical_checkout)
            .expect("canonicalize checkout")
            .display()
            .to_string();
        fs::remove_file(&checkout_alias).expect("remove checkout symlink");
        fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
        fs::remove_dir(&fixture_root).expect("remove fixture root");

        assert_eq!(actual_cwd.as_deref(), Some(expected_cwd.as_str()));
    }

    #[test]
    fn repository_membership_uses_origin_precedence_and_bounded_path_fallbacks() {
        let identity = RepositoryIdentity {
            normalized_origin: normalize_git_origin_url(
                "https://github.com/shravan-agent/codex-router.git",
            ),
            live_roots: vec![PathBuf::from("/dev/codex-router.live")],
            repository_basename: "codex-router".to_owned(),
            fallback_cwd: None,
        };

        let cases = [
            (
                "matching origin survives deleted worktree",
                Some("git@github.com:shravan-agent/codex-router.git"),
                "/history/unrelated-name",
                true,
            ),
            (
                "conflicting origin overrides live-root shape",
                Some("https://github.com/other/codex-router.git"),
                "/dev/codex-router.live/src",
                false,
            ),
            (
                "missing origin uses live root",
                None,
                "/dev/codex-router.live/src",
                true,
            ),
            (
                "missing origin uses dotted historical basename",
                None,
                "/history/codex-router.impl-search",
                true,
            ),
            (
                "missing origin rejects a prefixed lookalike",
                None,
                "/history/my-codex-router",
                false,
            ),
        ];

        for (name, row_origin, cwd, expected) in cases {
            assert_eq!(
                session_belongs_to_repository(&identity, row_origin, std::path::Path::new(cwd)),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn unknown_current_origin_never_uses_deleted_path_fallback_for_present_row_origin() {
        let identity = RepositoryIdentity {
            normalized_origin: None,
            live_roots: vec![PathBuf::from("/dev/codex-router.live")],
            repository_basename: "codex-router".to_owned(),
            fallback_cwd: None,
        };

        assert!(session_belongs_to_repository(
            &identity,
            Some("https://github.com/shravan-agent/codex-router.git"),
            std::path::Path::new("/dev/codex-router.live/src"),
        ));
        assert!(!session_belongs_to_repository(
            &identity,
            Some("https://github.com/shravan-agent/codex-router.git"),
            std::path::Path::new("/history/codex-router.impl-old"),
        ));
        assert!(session_belongs_to_repository(
            &identity,
            None,
            std::path::Path::new("/history/codex-router.impl-old"),
        ));
    }

    #[test]
    fn empty_repository_basename_does_not_admit_dot_or_dash_prefixed_paths() {
        let identity = RepositoryIdentity {
            normalized_origin: None,
            live_roots: Vec::new(),
            repository_basename: String::new(),
            fallback_cwd: None,
        };

        assert!(!session_belongs_to_repository(
            &identity,
            None,
            std::path::Path::new("/history/.cache"),
        ));
        assert!(!session_belongs_to_repository(
            &identity,
            None,
            std::path::Path::new("/history/-scratch"),
        ));
    }

    #[test]
    fn repository_scope_without_git_metadata_matches_only_the_exact_cwd() {
        let current_dir = std::env::temp_dir().join(format!(
            "codex-router-non-repository-sessions-scope-{}",
            std::process::id()
        ));
        let identity = RepositoryIdentity::discover(&current_dir);

        assert!(session_belongs_to_repository(&identity, None, &current_dir,));
        assert!(!session_belongs_to_repository(
            &identity,
            None,
            &current_dir.join("nested-repository"),
        ));
    }

    #[test]
    fn repository_scope_with_broken_git_metadata_falls_back_to_the_exact_cwd() {
        let repository_root = std::env::temp_dir().join(format!(
            "codex-router-broken-repository-sessions-scope-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        ));
        let current_dir = repository_root.join("nested");
        fs::create_dir_all(&current_dir).expect("nested test cwd should be created");
        fs::write(repository_root.join(".git"), "gitdir: missing\n")
            .expect("broken git metadata should be created");

        let identity = RepositoryIdentity::discover(&current_dir);
        let context = crate::CliContext::new(Vec::new()).with_current_dir(current_dir.clone());

        assert!(session_belongs_to_repository(&identity, None, &current_dir));
        assert!(!session_belongs_to_repository(
            &identity,
            None,
            &repository_root,
        ));
        assert!(matches!(
            RootFilter::from_query(SessionsRoot::Checkout, &context, None),
            RootFilter::Cwd(_)
        ));

        fs::remove_file(repository_root.join(".git"))
            .expect("broken git metadata should be removed");
        fs::remove_dir(&current_dir).expect("nested test cwd should be removed");
        fs::remove_dir(&repository_root).expect("test repository root should be removed");
    }

    #[test]
    fn partial_git_evidence_retains_the_current_checkout_as_a_live_root() {
        let current_checkout = PathBuf::from("/repo/project");

        assert_eq!(
            live_roots_with_current_checkout_fallback(Vec::new(), &current_checkout, true,),
            vec![current_checkout.clone()]
        );
        assert!(
            live_roots_with_current_checkout_fallback(Vec::new(), &current_checkout, false)
                .is_empty()
        );
    }

    #[test]
    fn repository_basename_prefers_origin_and_git_common_directory_over_invoking_worktree() {
        assert_eq!(
            repository_basename_from_evidence(
                normalize_git_origin_url("git@github.com:shravan-agent/codex-router.git")
                    .as_deref(),
                Some(std::path::Path::new("/dev/codex-router/.git")),
                Some(std::path::Path::new("/dev/codex-router")),
                std::path::Path::new("/dev/codex-router.impl-y"),
            ),
            "codex-router"
        );
        assert_eq!(
            repository_basename_from_evidence(
                None,
                Some(std::path::Path::new("/dev/codex-router/.git")),
                Some(std::path::Path::new("/dev/codex-router.impl-x")),
                std::path::Path::new("/dev/codex-router.impl-y"),
            ),
            "codex-router"
        );
    }

    #[test]
    fn qualified_search_ands_terms_and_keeps_branch_out_of_bare_matching() {
        let document = SessionSearchDocument {
            session_id: "019abc-session",
            name: "Named router session",
            title: "Fix router crash",
            preview: "Investigate deleted worktree sessions",
            first_user_message: "please make search robust",
            branch: "main",
            origin: "github.com/shravan-agent/codex-router",
            cwd: "/dev/codex-router.impl-search",
        };

        assert!(SessionSearchExpression::parse("019abc").matches(&document));
        assert!(SessionSearchExpression::parse("id:019abc").matches(&document));
        assert!(SessionSearchExpression::parse("b:main").matches(&document));
        assert!(SessionSearchExpression::parse("branch:main crash").matches(&document));
        assert!(SessionSearchExpression::parse("repo:codex-router crash").matches(&document));
        assert!(SessionSearchExpression::parse("\"deleted worktree\"").matches(&document));
        assert!(!SessionSearchExpression::parse("main").matches(&document));
        assert!(!SessionSearchExpression::parse("b:feature").matches(&document));
        assert!(!SessionSearchExpression::parse("main crash").matches(&document));
    }

    #[test]
    fn qualified_search_handles_unicode_literals_unknown_prefixes_and_empty_qualifiers() {
        let document = SessionSearchDocument {
            session_id: "thread-percent",
            name: "ÉCHEC named session",
            title: "ÉCHEC 100%_safe\\path",
            preview: "ticket:router",
            first_user_message: "",
            branch: "feature/Échec",
            origin: "github.com/Org/Repo",
            cwd: "/dev/Repo",
        };

        assert!(SessionSearchExpression::parse("échec").matches(&document));
        assert!(SessionSearchExpression::parse("100%_safe\\path").matches(&document));
        assert!(SessionSearchExpression::parse("ticket:router").matches(&document));
        assert!(!SessionSearchExpression::parse("id:").matches(&document));
        assert!(!SessionSearchExpression::parse("b:").matches(&document));
    }

    #[test]
    fn interactive_sessions_default_to_cwd_and_reject_checkout() {
        let command = SessionsCommand::parse(Vec::new()).expect("interactive command should parse");
        assert_eq!(command.root, SessionsRoot::Cwd);

        let error = SessionsCommand::parse(vec!["--checkout".into()])
            .expect_err("interactive checkout cannot be represented by the picker");
        assert!(error.contains("--checkout requires --list"), "{error}");
    }

    #[test]
    fn list_sessions_preserves_default_cwd_and_explicit_checkout() {
        let default_list = SessionsCommand::parse(vec!["--list".into()])
            .expect("default list command should parse");
        assert_eq!(default_list.root, SessionsRoot::Cwd);

        let checkout_list = SessionsCommand::parse(vec!["--checkout".into(), "--list".into()])
            .expect("checkout list command should parse");
        assert_eq!(checkout_list.root, SessionsRoot::Checkout);
    }

    #[test]
    fn positional_session_id_after_an_option_is_rejected_instead_of_forwarded() {
        let error = SessionsCommand::parse(vec![
            "--local".into(),
            "019ff0bb-5993-70d3-b1ba-f56724b94919".into(),
        ])
        .expect_err("a misplaced positional session id must not become a Codex argument");

        assert!(
            error.contains("session UUID must be the first argument or use --id"),
            "{error}"
        );
    }

    #[test]
    fn near_miss_positional_session_id_is_rejected_instead_of_forwarded() {
        for near_miss in [
            "019ff05e-77c0-7831-8f68-40bf182509f",
            "019ff05g-77c0-7831-8f68-40bf182509f6",
            "019ff05e77c078318f6840bf182509f6",
        ] {
            let error = SessionsCommand::parse(vec![near_miss.into()])
                .expect_err("a positional UUID typo must not become a Codex argument");

            assert!(error.contains("complete UUID"), "{near_miss}: {error}");
        }
    }

    #[test]
    fn sessions_command_preserves_escaped_uuid_passthrough_without_resume_mode() {
        let command = SessionsCommand::parse(vec![
            "--new".into(),
            "--".into(),
            "--request-id".into(),
            "11111111-1111-4111-8111-111111111111".into(),
        ])
        .expect("an escaped UUID belongs to Codex passthrough arguments");

        assert!(command.new);
        assert!(command.id.is_none());
        assert_eq!(
            command.codex_args,
            [
                std::ffi::OsString::from("--request-id"),
                std::ffi::OsString::from("11111111-1111-4111-8111-111111111111"),
            ]
        );
    }

    #[test]
    fn positional_session_id_conflicts_with_explicit_id() {
        for arguments in [
            vec![
                "019ff05e-77c0-7831-8f68-40bf182509f6".into(),
                "--id".into(),
                "11111111-1111-4111-8111-111111111111".into(),
            ],
            vec![
                "019ff05e-77c0-7831-8f68-40bf182509f6".into(),
                "--id=11111111-1111-4111-8111-111111111111".into(),
            ],
        ] {
            let error = SessionsCommand::parse(arguments)
                .expect_err("two exact resume IDs must not silently choose one");

            assert_eq!(
                error,
                "positional session UUID cannot be combined with --id"
            );
        }
    }

    #[test]
    fn duration_format_uses_now_without_suffix_for_subminute_values() {
        assert_eq!(format_duration_ms(0), "now");
        assert_eq!(format_duration_ms(59_000), "now");
        assert_eq!(format_duration_ms(60_000), "1m");
    }

    #[test]
    fn conversation_snippets_use_recent_jsonl_user_and_assistant_messages() {
        let jsonl = [
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "please pull main"}]
                }
            })
            .to_string(),
            "not-json".to_owned(),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "checking branch and upstream state"}]
                }
            })
            .to_string(),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "tool_call",
                    "role": "assistant",
                    "content": [{"text": "SECRET_TOOL_OUTPUT"}]
                }
            })
            .to_string(),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "system",
                    "content": [{"text": "system content"}]
                }
            })
            .to_string(),
        ]
        .join("\n");

        let snippets = extract_recent_conversation_snippets(&jsonl);

        assert_eq!(
            snippets,
            vec![
                "please pull main".to_owned(),
                "checking branch and upstream state".to_owned()
            ]
        );
    }

    #[test]
    fn conversation_snippets_keep_ten_messages_and_retain_latest_user_context() {
        let mut events = vec![json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "the user request that explains the work"}]
            }
        })];
        events.extend((1..=11).map(|index| {
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": format!("assistant update {index}")}]
                }
            })
        }));
        let jsonl = events
            .into_iter()
            .map(|event| event.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        let snippets = extract_recent_conversation_snippets(&jsonl);

        assert_eq!(snippets.len(), 10);
        assert_eq!(
            snippets.first().map(String::as_str),
            Some("the user request that explains the work")
        );
        assert_eq!(
            snippets.get(1).map(String::as_str),
            Some("assistant update 3")
        );
        assert_eq!(
            snippets.last().map(String::as_str),
            Some("assistant update 11")
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn conversation_preview_reads_message_beyond_the_previous_tail_limit() {
        let path = std::env::temp_dir().join(format!(
            "codex-router-large-session-history-{}.jsonl",
            std::process::id()
        ));
        let trailing_padding = format!("\n{}", "x\n".repeat(350 * 1024));
        let recent_message = json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "message inside the expanded tail"}]
            }
        })
        .to_string();
        fs::write(&path, format!("{recent_message}{trailing_padding}"))
            .expect("test should write large history fixture");

        let preview = SessionConversationPreview::from_rollout_path(Some(
            path.to_str().expect("temp path should be utf-8"),
        ));
        let _ = fs::remove_file(&path);

        assert_eq!(
            preview.snippets,
            vec!["message inside the expanded tail".to_owned()]
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn conversation_preview_recovers_when_tail_starts_inside_utf8_character() {
        let path = std::env::temp_dir().join(format!(
            "codex-router-split-utf8-session-history-{}.jsonl",
            std::process::id()
        ));
        let recent_message = json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "message after split UTF-8 boundary"}]
            }
        })
        .to_string();
        let mut bytes = "épartial line\n".as_bytes().to_vec();
        bytes.extend_from_slice(recent_message.as_bytes());
        bytes.push(b'\n');
        bytes.resize(
            usize::try_from(SESSION_CONVERSATION_MAX_READ_BYTES)
                .expect("tail bound should fit usize")
                + 1,
            b'x',
        );
        fs::write(&path, bytes).expect("test should write split UTF-8 history fixture");

        let preview = SessionConversationPreview::from_rollout_path(Some(
            path.to_str().expect("temp path should be utf-8"),
        ));
        let _ = fs::remove_file(&path);

        assert_eq!(
            preview.snippets,
            vec!["message after split UTF-8 boundary".to_owned()]
        );
    }

    #[test]
    fn conversation_snippets_skip_control_payloads() {
        let jsonl = [
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "# AGENTS.md instructions\n<INSTRUCTIONS>do not display</INSTRUCTIONS>"}]
                }
            })
            .to_string(),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "<hook_prompt hook_run_id=\"x\">control</hook_prompt>"}]
                }
            })
            .to_string(),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "real assistant reply"}]
                }
            })
            .to_string(),
        ]
        .join("\n");

        assert_eq!(
            extract_recent_conversation_snippets(&jsonl),
            vec!["real assistant reply".to_owned()]
        );
    }

    #[test]
    fn conversation_snippets_skip_review_wrapper_prompts() {
        let jsonl = [
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Read-only implementation review. Scope: current uncommitted diff. Review only P0-P2 findings."}]
                }
            })
            .to_string(),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "actual assistant answer"}]
                }
            })
            .to_string(),
        ]
        .join("\n");

        assert_eq!(
            extract_recent_conversation_snippets(&jsonl),
            vec!["actual assistant answer".to_owned()]
        );
    }

    #[test]
    fn conversation_snippets_skip_codex_transcript_wrappers() {
        let jsonl = [
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": "The following is the Codex agent history for review. It includes tool call arguments.\n>>> TRANSCRIPT START\nuser: keep the working tree clean"
                    }]
                }
            })
            .to_string(),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "actual resumed thread message"}]
                }
            })
            .to_string(),
        ]
        .join("\n");

        assert_eq!(
            extract_recent_conversation_snippets(&jsonl),
            vec!["actual resumed thread message".to_owned()]
        );
    }

    #[test]
    fn codex_home_resolution_uses_real_home_without_debug_redirect() {
        let home = PathBuf::from("/tmp/codex-router-home-policy");
        let explicit_codex_home = PathBuf::from("/tmp/explicit-codex-home");

        assert_eq!(
            codex_home_from_environment(None, Some(home.clone()))
                .expect("HOME should resolve Codex home"),
            home.join(".codex")
        );
        assert_eq!(
            codex_home_from_environment(Some(explicit_codex_home.clone()), Some(home))
                .expect("CODEX_HOME should win"),
            explicit_codex_home
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn conversation_preview_reads_rollout_path_with_fallback() {
        let path = std::env::temp_dir().join(format!(
            "codex-router-session-history-{}.jsonl",
            std::process::id()
        ));
        fs::write(
            &path,
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "real local history"}]
                }
            })
            .to_string(),
        )
        .expect("test should write history fixture");

        let preview = SessionConversationPreview::from_rollout_path(Some(
            path.to_str().expect("temp path should be utf-8"),
        ));
        let _ = fs::remove_file(&path);

        assert_eq!(preview.snippets, vec!["real local history".to_owned()]);
        assert_eq!(preview.unavailable_reason, None);

        let missing = SessionConversationPreview::from_rollout_path(None);
        assert_eq!(missing.snippets, Vec::<String>::new());
        assert_eq!(
            missing.unavailable_reason,
            Some("history unavailable".to_owned())
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn rollout_path_validation_rejects_paths_outside_codex_sessions() {
        let root = std::env::temp_dir().join(format!(
            "codex-router-rollout-validation-{}",
            std::process::id()
        ));
        let codex_home = root.join("codex-home");
        let sessions_dir = codex_home.join("sessions");
        let outside_dir = root.join("outside");
        fs::create_dir_all(&sessions_dir).expect("test should create sessions dir");
        fs::create_dir_all(&outside_dir).expect("test should create outside dir");
        let inside = sessions_dir.join("rollout.jsonl");
        let outside = outside_dir.join("rollout.jsonl");
        fs::write(&inside, "").expect("test should write inside rollout");
        fs::write(&outside, "").expect("test should write outside rollout");

        assert_eq!(
            validated_rollout_path(
                &codex_home,
                Some(inside.to_str().expect("inside path should be utf-8"))
            ),
            Some(
                inside
                    .canonicalize()
                    .expect("inside path should canonicalize")
                    .display()
                    .to_string()
            )
        );
        assert_eq!(
            validated_rollout_path(
                &codex_home,
                Some(outside.to_str().expect("outside path should be utf-8"))
            ),
            None
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn deferred_rollout_source_avoids_filesystem_validation_during_session_load() {
        let root = std::env::temp_dir().join(format!(
            "codex-router-deferred-rollout-{}",
            std::process::id()
        ));
        let codex_home = root.join("codex-home");
        let missing_inside = codex_home.join("sessions").join("missing.jsonl");
        let outside = root.join("outside").join("missing.jsonl");

        assert!(
            deferred_rollout_source(
                &codex_home,
                Some(
                    missing_inside
                        .to_str()
                        .expect("inside path should be utf-8")
                ),
            )
            .is_some(),
            "startup should keep an inside source candidate without canonicalizing every file"
        );
        assert_eq!(
            deferred_rollout_source(
                &codex_home,
                Some(outside.to_str().expect("outside path should be utf-8")),
            ),
            None,
            "startup should still reject lexically outside rollout paths"
        );
        assert_eq!(
            deferred_rollout_source(&codex_home, Some("../sessions/escape.jsonl")),
            None,
            "startup should reject parent-directory escape paths"
        );
    }
}

/// Sessions command failures.
#[derive(Debug, Error)]
pub enum SessionsCommandError {
    /// Checkout scope is intentionally list-only.
    #[error(
        "--checkout requires --list because the interactive picker supports cwd, repo, and all"
    )]
    InteractiveCheckoutUnsupported,
    /// Interactive picker has not landed yet.
    #[error("sessions interactive picker is not implemented yet; use --list --format json")]
    InteractivePickerNotImplemented,
    /// No matching session was found.
    #[error("no Codex sessions matched the requested filters")]
    NoSessionsMatch,
    /// Interactive picker was canceled.
    #[error("sessions picker canceled")]
    PickerCanceled,
    /// Interactive picker failed.
    #[error("sessions picker failed: {0}")]
    Picker(std::io::Error),
    /// Interactive picker cannot run without a terminal.
    #[error("sessions interactive picker requires a terminal; use --list or --last")]
    InteractiveRequiresTerminal,
    /// Interactive picker cannot render inside the current terminal width.
    #[error("sessions interactive picker requires a wider terminal")]
    TerminalTooNarrow,
    /// Codex failed to launch.
    #[error("failed to launch codex resume command: {0}")]
    CodexLaunch(std::io::Error),
    /// Codex exited unsuccessfully.
    #[error("codex resume command exited with {status}")]
    CodexExit {
        /// Exit status string.
        status: String,
    },
    /// Current provider could not be resolved.
    #[error(
        "sessions --provider current could not find model_provider in CODEX_HOME/codex-router.config.toml or CODEX_HOME/config.toml"
    )]
    CurrentProviderUnavailable,
    /// Config read failed.
    #[error("failed to read Codex config {path}: {source}")]
    ConfigRead {
        /// Config path.
        path: PathBuf,
        /// Source error.
        #[source]
        source: std::io::Error,
    },
    /// CODEX_HOME and HOME were both unavailable.
    #[error("could not locate Codex home; set CODEX_HOME or HOME")]
    CodexHomeUnavailable,
    /// Debug app-server socket override is unsafe or ambiguous.
    #[error("invalid debug app-server socket: {0}")]
    AppServerSocket(String),
    /// Failed to initialize async runtime.
    #[error("failed to initialize sessions runtime: {0}")]
    Runtime(std::io::Error),
    /// SQLite access failed.
    #[error("failed to read Codex sessions state: {0}")]
    Sqlx(sqlx::Error),
    /// Session id from Codex state is unsafe to pass to resume.
    #[error("unsafe Codex session id in state database")]
    UnsafeSessionId,
    /// Direct resume ids must be complete UUIDs.
    #[error("--id requires a complete UUID")]
    InvalidResumeSessionId,
    /// JSON rendering failed.
    #[error("failed to render sessions JSON: {0}")]
    Json(serde_json::Error),
    /// stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),
}
