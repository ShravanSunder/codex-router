//! Standalone Sessions catalog selection and native launch dispatch.

use std::{
    ffi::OsString,
    io::{IsTerminal, Write},
    process::Command,
};

use crate::CliContext;
use crate::presentation::session_picker::SessionsPickerDataQuery;
use crate::presentation::session_picker::SessionsPickerOutcome;
use crate::presentation::session_picker::SessionsPickerRecordLoader;
use crate::presentation::session_picker::SessionsPickerRequest;
use crate::presentation::session_picker::SessionsPickerRoot;
use crate::presentation::session_picker::run_sessions_picker;

#[path = "session_commands/session_launch_selection.rs"]
mod session_launch_selection;
use session_launch_selection::SessionsLaunchTarget;
#[cfg(test)]
use session_launch_selection::session_profile_for_environment;
use session_launch_selection::sessions_launch_target;

pub(crate) use collaboration_client::session_catalog::SessionSearchExpression;

#[path = "session_commands/session_command_options.rs"]
mod session_command_options;
use session_command_options::validate_exact_uuid_session_id;
pub(crate) use session_command_options::{
    SessionsCommand, SessionsFormat, SessionsProvider, SessionsRoot, SessionsSort, SessionsSource,
    command_help,
};
#[path = "session_commands/session_command_failures.rs"]
mod session_command_failures;
pub(crate) use collaboration_client::session_catalog::{
    RepositoryIdentity, normalized_paths_resolve_to_same_location,
    repository_contains_session as session_belongs_to_repository,
};
use collaboration_client::session_catalog::{discover_repository_identity, normalize_path};
pub use session_command_failures::SessionsCommandError;
#[path = "session_commands/session_display_text.rs"]
mod session_display_text;
#[cfg(test)]
use session_display_text::format_duration_ms;
use session_display_text::{
    display_title_from_session_fields, format_recency_at_ms, human_session_row,
    session_context_from_cwd, truncate_end,
};
#[path = "session_commands/session_catalog_records.rs"]
mod session_catalog_records;
use session_catalog_records::SessionRecord;
pub(crate) use session_catalog_records::{
    SessionConversationPreview, SessionConversationSource, SessionPickerRecord,
};
#[path = "session_commands/session_catalog_query.rs"]
mod session_catalog_query;
use codex_native_integration::ResumeModelChoice;
#[cfg(test)]
use session_catalog_query::codex_home_from_environment;
use session_catalog_query::{
    SessionRecordQuery, codex_home, current_provider_for_picker, load_session_records,
    load_session_records_for_query_with_identity,
};

#[path = "session_commands/picker_runtime_inventory.rs"]
mod picker_runtime_inventory;

const SESSION_TITLE_MAX_CHARS: usize = 96;
const SESSION_CONTEXT_MAX_CHARS: usize = 32;
const SESSION_CONVERSATION_SNIPPET_MAX_CHARS: usize = 180;
const DEFAULT_SESSION_RECORD_LIMIT: usize = 100;

/// Runs the sessions command.
pub fn run_sessions_command<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
) -> Result<(), SessionsCommandError> {
    if command.list {
        return run_session_listing(stdout, command, context);
    }
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
    if command.list {
        return run_session_listing(stdout, command, context);
    }
    let launch_target = sessions_launch_target(&command, context)?;
    if let Some(session_id) = command.id.as_deref() {
        return run_id_session(
            stdout,
            &command,
            context,
            &launch_target,
            runner,
            session_id,
        );
    }
    if command.new {
        return run_new_session(stdout, command, &launch_target, runner);
    }
    if command.last {
        return run_last_session(stdout, command, context, &launch_target, runner);
    }
    run_interactive_session(command, context, &launch_target, runner, picker)
}

fn run_session_listing<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
) -> Result<(), SessionsCommandError> {
    match command.format {
        SessionsFormat::Json => write_sessions_json(stdout, command, context),
        SessionsFormat::Table => write_sessions_table(stdout, command, context),
    }
}

fn run_id_session<W: Write>(
    stdout: &mut W,
    command: &SessionsCommand,
    context: &CliContext,
    launch_target: &SessionsLaunchTarget,
    runner: &mut impl SessionsCommandRunner,
    session_id: &str,
) -> Result<(), SessionsCommandError> {
    if validate_exact_uuid_session_id(session_id).is_err() {
        return Err(SessionsCommandError::InvalidResumeSessionId);
    }
    let model_choice = stored_model_choice_for_session(context, session_id);
    if command.dry_run {
        write_codex_resume_dry_run(
            stdout,
            launch_target,
            &command.codex_args,
            session_id,
            &model_choice,
        )?;
        return Ok(());
    }
    runner.run_codex_resume(&command.codex_args, session_id, &model_choice)
}

/// Reads the model and reasoning effort one stored session last ran with.
///
/// A session that is no longer in the catalog resumes exactly as it did before.
fn stored_model_choice_for_session(context: &CliContext, session_id: &str) -> ResumeModelChoice {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ResumeModelChoice::default();
    };
    let records = runtime.block_on(load_session_records_for_query_with_identity(
        SessionRecordQuery::for_session_id(session_id),
        context,
        None,
    ));
    let Ok(records) = records else {
        return ResumeModelChoice::default();
    };
    records
        .iter()
        .find(|record| record.session_id == session_id)
        .map_or_else(ResumeModelChoice::default, stored_model_choice_from_record)
}

/// Projects one catalog record into the launch-time model choice, warning once when a
/// stored value cannot be quoted as a TOML basic string.
fn stored_model_choice_from_record(record: &SessionRecord) -> ResumeModelChoice {
    let model = record.model.as_deref();
    let reasoning_effort = record.reasoning_effort.as_deref();
    if ResumeModelChoice::rejects_stored_value(model, reasoning_effort) {
        eprintln!(
            "agent-session: stored model or reasoning effort for session {} contains characters that cannot be passed to Codex; resuming without it",
            record.session_id
        );
    }
    ResumeModelChoice::from_stored_values(model, reasoning_effort)
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

fn run_interactive_session(
    command: SessionsCommand,
    context: &CliContext,
    launch_target: &SessionsLaunchTarget,
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
    let repository_identity = discover_repository_identity(context.current_dir());
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
    let service_directory = match launch_target {
        SessionsLaunchTarget::Hosted {
            service_directory, ..
        } => Some(service_directory.clone()),
        SessionsLaunchTarget::Local { .. } => None,
    };
    let record_loader =
        session_picker_record_loader(context.clone(), repository_identity, service_directory);
    let Some(outcome) = picker.select_session(request, Some(record_loader))? else {
        return Err(SessionsCommandError::PickerCanceled);
    };
    match outcome {
        SessionsPickerOutcome::ResumeSession(session_id) => {
            validate_resume_session_id(&session_id)?;
            let model_choice = stored_model_choice_for_session(context, &session_id);
            runner.run_codex_resume(&command.codex_args, &session_id, &model_choice)
        }
        SessionsPickerOutcome::ForkSession(session_id) => {
            validate_resume_session_id(&session_id)?;
            let model_choice = stored_model_choice_for_session(context, &session_id);
            runner.run_codex_fork(&command.codex_args, &session_id, &model_choice)
        }
        SessionsPickerOutcome::StartNewSession => runner.run_codex_new(&command.codex_args),
        SessionsPickerOutcome::TerminalTooNarrow => Err(SessionsCommandError::TerminalTooNarrow),
    }
}

fn session_picker_record_loader(
    context: CliContext,
    repository_identity: RepositoryIdentity,
    service_directory: Option<std::path::PathBuf>,
) -> SessionsPickerRecordLoader {
    let runtime_inventory =
        std::sync::Mutex::new(picker_runtime_inventory::PickerRuntimeInventory::default());
    std::sync::Arc::new(move |query| {
        let record_query = SessionRecordQuery::from_picker_query(query);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let mut inventory = runtime_inventory
            .lock()
            .map_err(|_| "runtime inventory lock failed".to_owned())?;
        runtime.block_on(async {
            let records = load_session_records_for_query_with_identity(
                record_query,
                &context,
                Some(repository_identity.clone()),
            )
            .await
            .map_err(|error| error.to_string())?;
            let stored = records
                .iter()
                .map(SessionPickerRecord::from_record)
                .collect();
            Ok(inventory
                .refresh(service_directory.as_deref(), stored)
                .await)
        })
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
    let model_choice = stored_model_choice_from_record(&record);

    if dry_run {
        write_codex_resume_dry_run(
            stdout,
            launch_target,
            &codex_args,
            &record.session_id,
            &model_choice,
        )?;
        return Ok(());
    }

    runner.run_codex_resume(&codex_args, &record.session_id, &model_choice)
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
    model_choice: &ResumeModelChoice,
) -> Result<(), SessionsCommandError> {
    write!(stdout, "codex").map_err(SessionsCommandError::Stdout)?;
    write_codex_args(
        stdout,
        &launch_target
            .resume_launch(codex_args, session_id, model_choice)
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

    /// Launches `codex --profile codex-router resume <session_id>` with the session's
    /// own stored model and reasoning effort.
    fn run_codex_resume(
        &mut self,
        codex_args: &[OsString],
        session_id: &str,
        model_choice: &ResumeModelChoice,
    ) -> Result<(), SessionsCommandError>;

    /// Launches `codex --profile codex-router fork <session_id>` with the session's
    /// own stored model and reasoning effort.
    fn run_codex_fork(
        &mut self,
        codex_args: &[OsString],
        session_id: &str,
        model_choice: &ResumeModelChoice,
    ) -> Result<(), SessionsCommandError>;
}

struct ProcessSessionsCommandRunner {
    launch_target: SessionsLaunchTarget,
}

impl SessionsCommandRunner for ProcessSessionsCommandRunner {
    fn run_codex_new(&mut self, codex_args: &[OsString]) -> Result<(), SessionsCommandError> {
        self.launch_target.resolve_for_launch()?;
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
        model_choice: &ResumeModelChoice,
    ) -> Result<(), SessionsCommandError> {
        self.launch_target.resolve_for_launch()?;
        let launch = self
            .launch_target
            .resume_launch(codex_args, session_id, model_choice);
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

    fn run_codex_fork(
        &mut self,
        codex_args: &[OsString],
        session_id: &str,
        model_choice: &ResumeModelChoice,
    ) -> Result<(), SessionsCommandError> {
        self.launch_target.resolve_for_launch()?;
        let launch = self
            .launch_target
            .fork_launch(codex_args, session_id, model_choice);
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

#[cfg(test)]
#[path = "session_commands/session_projection_tests.rs"]
mod tests;
