//! Router-owned Codex session picker command contract.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use clap::Parser;
use clap::ValueEnum;
use comfy_table::Table;
use comfy_table::presets::UTF8_FULL;
use inquire::Select;
use inquire::error::InquireError;
use serde::Serialize;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use thiserror::Error;

use crate::CliContext;

const SESSION_TITLE_MAX_CHARS: usize = 96;
const SESSION_CONTEXT_MAX_CHARS: usize = 32;

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
    /// Print the command that would be launched instead of executing it.
    pub dry_run: bool,
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
            dry_run: parsed.dry_run,
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
    #[arg(long)]
    dry_run: bool,
}

/// Runs the sessions command.
pub fn run_sessions_command<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
) -> Result<(), SessionsCommandError> {
    let mut runner = ProcessSessionsCommandRunner;
    let mut picker = InquireSessionsPicker;
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
    if command.last {
        return run_last_session(stdout, command, context, runner);
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
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(["session"]);
    for record in records {
        table.add_row([human_session_row(&record)]);
    }
    writeln!(stdout, "{table}").map_err(SessionsCommandError::Stdout)?;
    Ok(())
}

async fn load_session_records(
    command: SessionsCommand,
    context: &CliContext,
) -> Result<Vec<SessionRecord>, SessionsCommandError> {
    let scope_filter = ScopeFilter::from_command(command.scope, context);
    let codex_home_path = codex_home(context)?;
    let provider_filter = ProviderFilter::from_command(&command.provider, &codex_home_path)?;

    let state_database_path = codex_home_path.join("state_5.sqlite");
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
            title, preview, first_user_message,
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
        let cwd = row.get::<Option<String>, _>("cwd");
        if !source_matches(command.source, source.as_deref(), thread_source.as_deref()) {
            continue;
        }
        if !provider_filter.matches(row.get::<Option<String>, _>("model_provider").as_deref()) {
            continue;
        }
        if !scope_filter.matches(cwd.as_deref()) {
            continue;
        }
        records.push(SessionRecord {
            session_id: row.get("id"),
            cwd,
            provider: row.get::<Option<String>, _>("model_provider"),
            model: row.get::<Option<String>, _>("model"),
            source,
            thread_source,
            git_branch: row.get::<Option<String>, _>("git_branch"),
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
    }
    pool.close().await;

    Ok(records)
}

fn run_interactive_session(
    command: SessionsCommand,
    context: &CliContext,
    runner: &mut impl SessionsCommandRunner,
    picker: &mut impl SessionsPicker,
) -> Result<(), SessionsCommandError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(SessionsCommandError::Runtime)?;
    let records = runtime.block_on(load_session_records(command, context))?;
    if records.is_empty() {
        return Err(SessionsCommandError::NoSessionsMatch);
    }
    let choices = records
        .into_iter()
        .map(SessionPickerChoice::from_record)
        .collect::<Vec<_>>();
    let Some(session_id) = picker.select_session(choices)? else {
        return Err(SessionsCommandError::PickerCanceled);
    };
    validate_resume_session_id(&session_id)?;
    runner.run_codex_resume(&session_id)
}

fn run_last_session<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
    runner: &mut impl SessionsCommandRunner,
) -> Result<(), SessionsCommandError> {
    let dry_run = command.dry_run;
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
        writeln!(
            stdout,
            "codex --profile codex-router resume -- {}",
            record.session_id
        )
        .map_err(SessionsCommandError::Stdout)?;
        return Ok(());
    }

    runner.run_codex_resume(&record.session_id)
}

/// Interactive session picker.
pub(crate) trait SessionsPicker {
    /// Selects one session id, or `None` when the picker was canceled.
    fn select_session(
        &mut self,
        choices: Vec<SessionPickerChoice>,
    ) -> Result<Option<String>, SessionsCommandError>;
}

struct InquireSessionsPicker;

impl SessionsPicker for InquireSessionsPicker {
    fn select_session(
        &mut self,
        choices: Vec<SessionPickerChoice>,
    ) -> Result<Option<String>, SessionsCommandError> {
        Select::new("Resume Codex session", choices)
            .prompt_skippable()
            .map(|choice| choice.map(|choice| choice.session_id().to_owned()))
            .map_err(SessionsCommandError::Picker)
    }
}

/// Runs a selected Codex session.
pub(crate) trait SessionsCommandRunner {
    /// Launches `codex --profile codex-router resume <session_id>`.
    fn run_codex_resume(&mut self, session_id: &str) -> Result<(), SessionsCommandError>;
}

struct ProcessSessionsCommandRunner;

impl SessionsCommandRunner for ProcessSessionsCommandRunner {
    fn run_codex_resume(&mut self, session_id: &str) -> Result<(), SessionsCommandError> {
        let status = Command::new("codex")
            .arg("--profile")
            .arg("codex-router")
            .arg("resume")
            .arg("--")
            .arg(session_id)
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

    fn matches(&self, provider: Option<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::Id(expected_provider) => provider == Some(expected_provider.as_str()),
        }
    }
}

#[derive(Debug)]
enum ScopeFilter {
    Any,
    Cwd(PathBuf),
    Worktree(PathBuf),
}

impl ScopeFilter {
    fn from_command(scope: SessionsScope, context: &CliContext) -> Self {
        match scope {
            SessionsScope::Any => Self::Any,
            SessionsScope::Cwd => Self::Cwd(normalize_path(context.current_dir())),
            SessionsScope::Worktree => Self::Worktree(
                find_worktree_root(context.current_dir())
                    .unwrap_or_else(|| normalize_path(context.current_dir())),
            ),
        }
    }

    fn matches(&self, cwd: Option<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::Cwd(current_dir) => cwd
                .map(|session_cwd| normalize_path(Path::new(session_cwd)) == *current_dir)
                .unwrap_or(false),
            Self::Worktree(worktree_root) => cwd
                .map(|session_cwd| {
                    path_is_equal_or_child(&normalize_path(Path::new(session_cwd)), worktree_root)
                })
                .unwrap_or(false),
        }
    }
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
        return Some(value[1..value.len() - 1].to_owned());
    }
    None
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
            source_indicates_subagent(source) || matches!(thread_source, Some("subagent"))
        }
    }
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

fn source_indicates_subagent(source: Option<&str>) -> bool {
    let Some(source) = source else {
        return false;
    };
    if source == "subagent" {
        return true;
    }
    let trimmed = source.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return false;
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .is_some_and(json_mentions_subagent_source)
}

fn json_mentions_subagent_source(value: Value) -> bool {
    match value {
        Value::String(value) => value == "subagent",
        Value::Array(values) => values.into_iter().any(json_mentions_subagent_source),
        Value::Object(object) => object.into_iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "subagent" | "parent_agent_id" | "parent_session_id" | "parent_thread_id"
            ) || json_mentions_subagent_source(value)
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
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

fn path_is_equal_or_child(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

#[derive(Debug, Serialize)]
struct SessionRecord {
    session_id: String,
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
pub(crate) struct SessionPickerChoice {
    session_id: String,
    label: String,
}

impl SessionPickerChoice {
    fn from_record(record: SessionRecord) -> Self {
        Self {
            session_id: record.session_id.clone(),
            label: human_session_row(&record),
        }
    }

    /// Returns the session id represented by this picker row.
    #[must_use]
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }
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
    use super::format_duration_ms;

    #[test]
    fn duration_format_uses_now_without_suffix_for_subminute_values() {
        assert_eq!(format_duration_ms(0), "now");
        assert_eq!(format_duration_ms(59_000), "now");
        assert_eq!(format_duration_ms(60_000), "1m");
    }
}

impl fmt::Display for SessionPickerChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label)
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
    Picker(InquireError),
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
