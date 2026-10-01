//! `agent-sessions --provider claude` list and launch commands.

use super::{
    CliContext, SessionsCommand, SessionsCommandError, SessionsFormat,
    claude_launch_target::ClaudeLaunchTarget,
};
use std::{ffi::OsString, io::Write};

pub(super) fn run_claude_sessions_command<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    context: &CliContext,
) -> Result<(), SessionsCommandError> {
    let launch_target = ClaudeLaunchTarget::resolve(context)?;
    if command.list {
        return write_claude_session_listing(stdout, command, &launch_target);
    }
    let session_id = command.id.as_deref();
    if command.dry_run {
        return write_claude_dry_run(stdout, &launch_target, session_id, &command.codex_args);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(SessionsCommandError::Runtime)?;
    let environment = runtime.block_on(launch_target.routed_environment())?;
    let status = launch_target
        .command(session_id, &command.codex_args, environment)
        .status()
        .map_err(|error| {
            SessionsCommandError::ClaudeLaunch(format!("failed to launch Claude Code: {error}"))
        })?;
    if !status.success() {
        return Err(SessionsCommandError::ClaudeExit {
            status: status.to_string(),
        });
    }
    Ok(())
}

fn write_claude_session_listing<W: Write>(
    stdout: &mut W,
    command: SessionsCommand,
    launch_target: &ClaudeLaunchTarget,
) -> Result<(), SessionsCommandError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(SessionsCommandError::Runtime)?;
    let (records, active_error) = runtime.block_on(launch_target.session_records(command.limit))?;
    if let Some(error) = active_error {
        eprintln!("agent-sessions: active Claude session discovery unavailable: {error}");
    }
    match command.format {
        SessionsFormat::Json => {
            serde_json::to_writer(&mut *stdout, &records).map_err(SessionsCommandError::Json)?;
            writeln!(stdout).map_err(SessionsCommandError::Stdout)
        }
        SessionsFormat::Table => write_claude_session_table(stdout, &records),
    }
}

fn write_claude_session_table<W: Write>(
    stdout: &mut W,
    records: &[serde_json::Value],
) -> Result<(), SessionsCommandError> {
    for (index, record) in records.iter().enumerate() {
        if index > 0 {
            writeln!(stdout).map_err(SessionsCommandError::Stdout)?;
        }
        let session_id = record
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unknown>");
        let source = record
            .get("source")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let detail = record
            .get("name")
            .and_then(serde_json::Value::as_str)
            .or_else(|| record.get("project").and_then(serde_json::Value::as_str))
            .unwrap_or("");
        write!(stdout, "{session_id}  {source}  {detail}").map_err(SessionsCommandError::Stdout)?;
    }
    if !records.is_empty() {
        writeln!(stdout).map_err(SessionsCommandError::Stdout)?;
    }
    Ok(())
}

fn write_claude_dry_run<W: Write>(
    stdout: &mut W,
    launch_target: &ClaudeLaunchTarget,
    session_id: Option<&str>,
    passthrough_arguments: &[OsString],
) -> Result<(), SessionsCommandError> {
    write!(stdout, "claude").map_err(SessionsCommandError::Stdout)?;
    for argument in launch_target.launch_arguments(session_id, passthrough_arguments) {
        write!(stdout, " {}", argument.to_string_lossy()).map_err(SessionsCommandError::Stdout)?;
    }
    writeln!(stdout).map_err(SessionsCommandError::Stdout)
}
