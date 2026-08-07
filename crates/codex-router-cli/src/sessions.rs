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
use crate::presentation::session_picker::run_sessions_picker;

const SESSION_TITLE_MAX_CHARS: usize = 96;
const SESSION_CONTEXT_MAX_CHARS: usize = 32;
const SESSION_CONVERSATION_MAX_READ_BYTES: u64 = 256 * 1024;
const SESSION_CONVERSATION_MAX_SNIPPETS: usize = 4;
const SESSION_CONVERSATION_SNIPPET_MAX_CHARS: usize = 180;
const DEFAULT_SESSION_RECORD_LIMIT: usize = 100;
const SESSION_RECORD_PAGE_SIZE: usize = 250;

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
    /// Launch a new Codex session instead of resuming one.
    pub new: bool,
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
            root: query.root,
            provider: query.provider,
            source: query.source,
            sort: query.sort,
            last: false,
            limit: DEFAULT_SESSION_RECORD_LIMIT,
            search: query.search,
        }
    }
}

impl SessionsCommand {
    pub(crate) fn parse(arguments: Vec<OsString>) -> Result<Self, String> {
        let mut argv = Vec::with_capacity(arguments.len() + 1);
        argv.push(OsString::from("sessions"));
        argv.extend(arguments);
        let parsed =
            ClapSessionsCommand::try_parse_from(argv).map_err(|error| error.to_string())?;
        reject_legacy_router_options(&parsed.codex_args)?;
        reject_interactive_limit(&parsed)?;
        Ok(Self {
            root: parsed.root()?,
            provider: parsed.provider,
            source: parsed.source,
            sort: parsed.sort,
            list: parsed.list,
            format: parsed.format,
            last: parsed.last,
            new: parsed.new,
            limit: parsed.limit.unwrap_or(DEFAULT_SESSION_RECORD_LIMIT),
            dry_run: parsed.dry_run,
            codex_args: parsed.codex_args,
        })
    }
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

fn reject_interactive_limit(command: &ClapSessionsCommand) -> Result<(), String> {
    if command.limit.is_some() && !command.list {
        return Err("--limit only applies with --list".to_owned());
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
    #[arg(long, conflicts_with_all = ["list", "last"])]
    new: bool,
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
    let app_server_socket =
        codex_router_codex::CodexPaths::from_codex_home(codex_home(context)?).app_server_socket();
    let mut runner = ProcessSessionsCommandRunner { app_server_socket };
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
    let app_server_socket =
        codex_router_codex::CodexPaths::from_codex_home(codex_home(context)?).app_server_socket();
    if command.new {
        return run_new_session(stdout, command, &app_server_socket, runner);
    }
    if command.last {
        return run_last_session(stdout, command, context, &app_server_socket, runner);
    }
    if !command.list {
        return run_interactive_session(command, context, runner, picker);
    }
    match command.format {
        SessionsFormat::Json => write_sessions_json(stdout, command, context),
        SessionsFormat::Table => write_sessions_table(stdout, command, context),
    }
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
    let root_filter = RootFilter::from_command(query.root, context);
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
    let target_limit = if query.last { 1 } else { query.limit };
    let mut offset = 0_i64;
    while target_limit == 0 || records.len() < target_limit {
        let page_size = target_limit
            .checked_sub(records.len())
            .filter(|remaining| *remaining > 0)
            .map_or(SESSION_RECORD_PAGE_SIZE, |remaining| {
                remaining.min(SESSION_RECORD_PAGE_SIZE)
            });
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
                SELECT
                    id, rollout_path, cwd, model_provider, model, source, thread_source, git_branch,
                    title, preview, first_user_message,
                    created_at_ms, updated_at_ms, recency_at_ms
                FROM threads
                WHERE archived = 0
                "#,
        );
        append_session_record_filters(
            &mut builder,
            &root_filter,
            &provider_filter,
            query.source,
            &query.search,
        );
        match query.sort {
            SessionsSort::Created => {
                builder.push(" ORDER BY created_at_ms DESC, id DESC");
            }
            SessionsSort::Updated => {
                builder.push(" ORDER BY recency_at_ms DESC, id DESC");
            }
        }
        builder
            .push(" LIMIT ")
            .push_bind(i64::try_from(page_size).unwrap_or(i64::MAX))
            .push(" OFFSET ")
            .push_bind(offset);
        let rows = builder
            .build()
            .fetch_all(&pool)
            .await
            .map_err(SessionsCommandError::Sqlx)?;

        if rows.is_empty() {
            break;
        }
        offset = offset.saturating_add(i64::try_from(rows.len()).unwrap_or(i64::MAX));

        for row in rows {
            let source = row.get::<Option<String>, _>("source");
            let thread_source = row.get::<Option<String>, _>("thread_source");
            let cwd = row.get::<Option<String>, _>("cwd");
            records.push(SessionRecord {
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
                preview: row.get::<Option<String>, _>("preview"),
                display_title: display_title_from_session_fields(
                    row.get::<Option<String>, _>("title").as_deref(),
                    row.get::<Option<String>, _>("preview").as_deref(),
                    row.get::<Option<String>, _>("first_user_message")
                        .as_deref(),
                ),
                created_at_ms: row.get::<Option<i64>, _>("created_at_ms"),
                updated_at_ms: row.get::<Option<i64>, _>("updated_at_ms"),
                recency_at_ms: row.get::<Option<i64>, _>("recency_at_ms"),
            });
            if target_limit != 0 && records.len() >= target_limit {
                break;
            }
        }
    }
    pool.close().await;

    Ok(records)
}

fn append_session_record_filters(
    builder: &mut QueryBuilder<Sqlite>,
    root_filter: &RootFilter,
    provider_filter: &ProviderFilter,
    source: SessionsSource,
    search: &str,
) {
    append_root_filter(builder, root_filter);
    append_provider_filter(builder, provider_filter);
    append_source_filter(builder, source);
    append_search_filter(builder, search);
}

fn append_root_filter(builder: &mut QueryBuilder<Sqlite>, root_filter: &RootFilter) {
    match root_filter {
        RootFilter::Any => {}
        RootFilter::Cwd(current_dir) => {
            builder.push(" AND (");
            append_path_exact_filter(builder, current_dir);
            builder.push(")");
        }
        RootFilter::Checkout(checkout_root) => {
            builder.push(" AND (");
            append_path_scope_filter(builder, checkout_root);
            builder.push(")");
        }
        RootFilter::Repo(repo_roots) => {
            if repo_roots.is_empty() {
                builder.push(" AND 0 = 1");
                return;
            }
            builder.push(" AND (");
            for (index, repo_root) in repo_roots.iter().enumerate() {
                if index > 0 {
                    builder.push(" OR ");
                }
                append_path_scope_filter(builder, repo_root);
            }
            builder.push(")");
        }
    }
}

fn append_path_exact_filter(builder: &mut QueryBuilder<Sqlite>, path: &Path) {
    for (index, path_value) in path_sql_values(path).into_iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder.push("cwd = ").push_bind(path_value);
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

fn append_search_filter(builder: &mut QueryBuilder<Sqlite>, search: &str) {
    let search = search.trim().to_lowercase();
    if search.is_empty() {
        return;
    }
    let pattern = format!("%{}%", escape_like(&search));
    builder.push(" AND (lower(id) LIKE ");
    append_like_bind(builder, &pattern);
    builder.push(" OR lower(coalesce(title, '')) LIKE ");
    append_like_bind(builder, &pattern);
    builder.push(" OR lower(coalesce(preview, '')) LIKE ");
    append_like_bind(builder, &pattern);
    builder.push(" OR lower(coalesce(first_user_message, '')) LIKE ");
    append_like_bind(builder, &pattern);
    builder.push(" OR lower(coalesce(model_provider, '')) LIKE ");
    append_like_bind(builder, &pattern);
    builder.push(")");
}

fn append_like_bind(builder: &mut QueryBuilder<Sqlite>, pattern: &str) {
    builder.push_bind(pattern.to_owned()).push(" ESCAPE '\\'");
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
    let picker_root = command.root;
    let picker_provider = command.provider.clone();
    let picker_source = command.source;
    let picker_sort = command.sort;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(SessionsCommandError::Runtime)?;
    let records = runtime.block_on(load_session_records(command.clone(), context))?;
    let request = SessionsPickerRequest {
        root: picker_root,
        provider: picker_provider,
        source: picker_source,
        sort: picker_sort,
        current_dir: normalize_path(context.current_dir()),
        checkout_root: checkout_root(context.current_dir()),
        repo_roots: repo_roots(context.current_dir()),
        current_provider: current_provider_for_picker(context),
        new_session_args_display: codex_args_display(&command.codex_args),
        records: records
            .iter()
            .map(SessionPickerRecord::from_record)
            .collect(),
    };
    let record_loader = session_picker_record_loader(context.clone());
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

fn session_picker_record_loader(context: CliContext) -> SessionsPickerRecordLoader {
    std::sync::Arc::new(move |query| {
        let record_query = SessionRecordQuery::from_picker_query(query);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime
            .block_on(load_session_records_for_query(record_query, &context))
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
    app_server_socket: &Path,
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
        write_codex_resume_dry_run(stdout, app_server_socket, &codex_args, &record.session_id)?;
        return Ok(());
    }

    runner.run_codex_resume(&codex_args, &record.session_id)
}

fn run_new_session<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    app_server_socket: &Path,
    runner: &mut impl SessionsCommandRunner,
) -> Result<(), SessionsCommandError> {
    if command.dry_run {
        write_codex_new_dry_run(stdout, app_server_socket, &command.codex_args)?;
        return Ok(());
    }

    runner.run_codex_new(&command.codex_args)
}

fn write_codex_new_dry_run<W: Write>(
    stdout: &mut W,
    app_server_socket: &Path,
    codex_args: &[OsString],
) -> Result<(), SessionsCommandError> {
    write!(stdout, "codex").map_err(SessionsCommandError::Stdout)?;
    write_codex_args(
        stdout,
        &codex_router_codex::SessionLaunch::new(app_server_socket, codex_args).arguments(),
    )?;
    writeln!(stdout).map_err(SessionsCommandError::Stdout)
}

fn write_codex_resume_dry_run<W: Write>(
    stdout: &mut W,
    app_server_socket: &Path,
    codex_args: &[OsString],
    session_id: &str,
) -> Result<(), SessionsCommandError> {
    write!(stdout, "codex").map_err(SessionsCommandError::Stdout)?;
    write_codex_args(
        stdout,
        &codex_router_codex::SessionLaunch::resume(app_server_socket, codex_args, session_id)
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
    app_server_socket: PathBuf,
}

impl SessionsCommandRunner for ProcessSessionsCommandRunner {
    fn run_codex_new(&mut self, codex_args: &[OsString]) -> Result<(), SessionsCommandError> {
        let launch = codex_router_codex::SessionLaunch::new(&self.app_server_socket, codex_args);
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
        let launch = codex_router_codex::SessionLaunch::resume(
            &self.app_server_socket,
            codex_args,
            session_id,
        );
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
    Cwd(PathBuf),
    Checkout(PathBuf),
    Repo(Vec<PathBuf>),
}

impl RootFilter {
    fn from_command(root: SessionsRoot, context: &CliContext) -> Self {
        match root {
            SessionsRoot::Any => Self::Any,
            SessionsRoot::Cwd => Self::Cwd(normalize_path(context.current_dir())),
            SessionsRoot::Checkout => Self::Checkout(checkout_root(context.current_dir())),
            SessionsRoot::Repo => Self::Repo(repo_roots(context.current_dir())),
        }
    }
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
    vec![checkout_root(current_dir)]
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
    preview: Option<String>,
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
}

/// Picker display row for one session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionPickerRecord {
    pub(crate) session_id: String,
    pub(crate) title: String,
    pub(crate) recency: String,
    pub(crate) created: String,
    pub(crate) recency_at_ms: Option<i64>,
    pub(crate) created_at_ms: Option<i64>,
    pub(crate) branch: String,
    pub(crate) context: String,
    pub(crate) cwd: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) preview: Option<String>,
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
    fn from_record(record: &SessionRecord) -> Self {
        Self {
            session_id: record.session_id.clone(),
            title: record.display_title().to_owned(),
            recency: format_recency_at_ms(record.recency_at_ms),
            created: format_recency_at_ms(record.created_at_ms),
            recency_at_ms: record.recency_at_ms,
            created_at_ms: record.created_at_ms,
            branch: record.branch().to_owned(),
            context: record
                .cwd
                .as_deref()
                .map(session_context_from_cwd)
                .unwrap_or_else(|| "-".to_owned()),
            cwd: record.cwd.clone(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            preview: record
                .preview
                .clone()
                .or_else(|| Some(record.display_title().to_owned())),
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
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    if start > 0
        && let Some((_, remaining)) = text.split_once('\n')
    {
        return Ok(remaining.to_owned());
    }
    Ok(text)
}

fn extract_recent_conversation_snippets(text: &str) -> Vec<String> {
    let mut snippets = Vec::new();
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(snippet) = conversation_snippet_from_event(&event) else {
            continue;
        };
        snippets.push(snippet);
        if snippets.len() > SESSION_CONVERSATION_MAX_SNIPPETS {
            snippets.remove(0);
        }
    }
    snippets
}

fn conversation_snippet_from_event(event: &Value) -> Option<String> {
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
    Some(truncate_end(
        normalized,
        SESSION_CONVERSATION_SNIPPET_MAX_CHARS,
    ))
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
    title: Option<&str>,
    preview: Option<&str>,
    first_user_message: Option<&str>,
) -> Option<String> {
    [title, preview, first_user_message]
        .into_iter()
        .flatten()
        .find_map(normalize_display_title)
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
    use super::SessionConversationPreview;
    use super::codex_home_from_environment;
    use super::deferred_rollout_source;
    use super::extract_recent_conversation_snippets;
    use super::format_duration_ms;
    use super::validated_rollout_path;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

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
    /// Failed to initialize async runtime.
    #[error("failed to initialize sessions runtime: {0}")]
    Runtime(std::io::Error),
    /// SQLite access failed.
    #[error("failed to read Codex sessions state: {0}")]
    Sqlx(sqlx::Error),
    /// Session id from Codex state is unsafe to pass to resume.
    #[error("unsafe Codex session id in state database")]
    UnsafeSessionId,
    /// JSON rendering failed.
    #[error("failed to render sessions JSON: {0}")]
    Json(serde_json::Error),
    /// stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),
}
