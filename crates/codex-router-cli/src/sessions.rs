//! Router-owned Codex session picker command contract.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;
use clap::ValueEnum;
use serde::Serialize;
use sqlx::Row;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use thiserror::Error;

use crate::CliContext;

/// Session search scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SessionsScope {
    /// Exact current working directory.
    Cwd,
    /// Current git worktree or repository root.
    Worktree,
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
    pub scope: SessionsScope,
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
}

impl SessionsCommand {
    pub(crate) fn parse(arguments: Vec<OsString>) -> Result<Self, String> {
        let mut argv = Vec::with_capacity(arguments.len() + 1);
        argv.push(OsString::from("sessions"));
        argv.extend(arguments);
        let parsed =
            ClapSessionsCommand::try_parse_from(argv).map_err(|error| error.to_string())?;
        Ok(Self {
            scope: parsed.scope,
            provider: parsed.provider,
            source: parsed.source,
            sort: parsed.sort,
            list: parsed.list,
            format: parsed.format,
            last: parsed.last,
        })
    }
}

#[derive(Debug, Parser)]
#[command(name = "sessions", disable_help_subcommand = true)]
struct ClapSessionsCommand {
    #[arg(long, value_enum, default_value = "worktree")]
    scope: SessionsScope,
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
}

/// Runs the sessions command.
pub fn run_sessions_command<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
) -> Result<(), SessionsCommandError> {
    if !command.list {
        return Err(SessionsCommandError::InteractivePickerNotImplemented);
    }
    match command.format {
        SessionsFormat::Json => write_sessions_json(stdout, command, context),
        SessionsFormat::Table => Err(SessionsCommandError::TableListNotImplemented),
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

async fn load_session_records(
    command: SessionsCommand,
    context: &CliContext,
) -> Result<Vec<SessionRecord>, SessionsCommandError> {
    if command.last {
        return Err(SessionsCommandError::LastNotImplemented);
    }
    if !matches!(command.scope, SessionsScope::Any) {
        return Err(SessionsCommandError::ScopedListNotImplemented);
    }
    if !matches!(command.provider, SessionsProvider::Any) {
        return Err(SessionsCommandError::ProviderFilterNotImplemented);
    }

    let state_database_path = codex_home(context)?.join("state_5.sqlite");
    let options = SqliteConnectOptions::new()
        .filename(&state_database_path)
        .read_only(true)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(SessionsCommandError::Sqlx)?;

    let rows = sqlx::query(
        r#"
        SELECT
            id, cwd, model_provider, model, source, thread_source, git_branch,
            created_at_ms, updated_at_ms, recency_at_ms
        FROM threads
        WHERE archived = 0
        ORDER BY
            CASE ? WHEN 'created' THEN created_at_ms ELSE recency_at_ms END DESC,
            id DESC
        "#,
    )
    .bind(sort_key(command.sort))
    .fetch_all(&pool)
    .await
    .map_err(SessionsCommandError::Sqlx)?;

    let mut records = Vec::new();
    for row in rows {
        let source = row.get::<Option<String>, _>("source");
        let thread_source = row.get::<Option<String>, _>("thread_source");
        if !source_matches(command.source, source.as_deref(), thread_source.as_deref()) {
            continue;
        }
        records.push(SessionRecord {
            session_id: row.get("id"),
            cwd: row.get::<Option<String>, _>("cwd"),
            provider: row.get::<Option<String>, _>("model_provider"),
            model: row.get::<Option<String>, _>("model"),
            source,
            thread_source,
            git_branch: row.get::<Option<String>, _>("git_branch"),
            created_at_ms: row.get::<Option<i64>, _>("created_at_ms"),
            updated_at_ms: row.get::<Option<i64>, _>("updated_at_ms"),
            recency_at_ms: row.get::<Option<i64>, _>("recency_at_ms"),
        });
    }
    pool.close().await;

    Ok(records)
}

fn codex_home(context: &CliContext) -> Result<PathBuf, SessionsCommandError> {
    if let Some(codex_home) = context.env_var("CODEX_HOME") {
        return Ok(PathBuf::from(codex_home));
    }
    let Some(home) = context.env_var("HOME") else {
        return Err(SessionsCommandError::CodexHomeUnavailable);
    };
    Ok(PathBuf::from(home).join(".codex"))
}

fn sort_key(sort: SessionsSort) -> &'static str {
    match sort {
        SessionsSort::Updated => "updated",
        SessionsSort::Created => "created",
    }
}

fn source_matches(
    source_filter: SessionsSource,
    source: Option<&str>,
    thread_source: Option<&str>,
) -> bool {
    match source_filter {
        SessionsSource::All => true,
        SessionsSource::Interactive => {
            matches!(source, Some("cli" | "vscode"))
                && !matches!(thread_source, Some("exec" | "app_server" | "subagent"))
        }
        SessionsSource::Subagents => {
            matches!(source, Some("subagent")) || matches!(thread_source, Some("subagent"))
        }
    }
}

#[derive(Debug, Serialize)]
struct SessionRecord {
    session_id: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recency_at_ms: Option<i64>,
}

/// Sessions command failures.
#[derive(Debug, Error)]
pub enum SessionsCommandError {
    /// Interactive picker has not landed yet.
    #[error("sessions interactive picker is not implemented yet; use --list --format json")]
    InteractivePickerNotImplemented,
    /// Table output has not landed yet.
    #[error("sessions table output is not implemented yet; use --format json")]
    TableListNotImplemented,
    /// Latest-session launch has not landed yet.
    #[error("sessions --last is not implemented yet")]
    LastNotImplemented,
    /// Scoped session filtering has not landed yet.
    #[error("sessions scoped list is not implemented yet; use --scope any")]
    ScopedListNotImplemented,
    /// Provider filtering has not landed yet.
    #[error("sessions provider filter is not implemented yet; use --provider any")]
    ProviderFilterNotImplemented,
    /// CODEX_HOME and HOME were both unavailable.
    #[error("could not locate Codex home; set CODEX_HOME or HOME")]
    CodexHomeUnavailable,
    /// Failed to initialize async runtime.
    #[error("failed to initialize sessions runtime: {0}")]
    Runtime(std::io::Error),
    /// SQLite access failed.
    #[error("failed to read Codex sessions state: {0}")]
    Sqlx(sqlx::Error),
    /// JSON rendering failed.
    #[error("failed to render sessions JSON: {0}")]
    Json(serde_json::Error),
    /// stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),
}
