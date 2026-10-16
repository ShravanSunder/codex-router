//! Process effects after the picker returns; configured prepared routes never fall back.
use super::{SessionActionSelection, SessionsCommandError, SessionsLaunchTarget};
use std::{ffi::OsString, process::Command};

/// Runs a selected Codex session.
pub(crate) trait SessionsCommandRunner {
    fn run_prepared_source(
        &mut self,
        native_arguments: &[OsString],
        selection: &super::source_ssh_protocol::PreparedSourceSession,
    ) -> Result<(), SessionsCommandError>;

    /// Launches `codex --profile codex-router`.
    fn run_codex_new(&mut self, codex_args: &[OsString]) -> Result<(), SessionsCommandError>;

    /// Launches `codex --profile codex-router resume <session_id>` with the session's
    /// own stored model and reasoning effort.
    fn run_codex_resume(
        &mut self,
        codex_args: &[OsString],
        selection: &SessionActionSelection,
    ) -> Result<(), SessionsCommandError>;

    /// Launches `codex --profile codex-router fork <session_id>` with the session's
    /// own stored model and reasoning effort.
    fn run_codex_fork(
        &mut self,
        codex_args: &[OsString],
        selection: &SessionActionSelection,
    ) -> Result<(), SessionsCommandError>;
}

pub(super) struct ProcessSessionsCommandRunner {
    pub(super) launch_target: SessionsLaunchTarget,
}

impl SessionsCommandRunner for ProcessSessionsCommandRunner {
    fn run_prepared_source(
        &mut self,
        native_arguments: &[OsString],
        selection: &super::source_ssh_protocol::PreparedSourceSession,
    ) -> Result<(), SessionsCommandError> {
        super::source_ssh_transport::handoff_prepared_source(
            &selection.profile,
            &selection.launch,
            native_arguments,
        )
    }

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
        selection: &SessionActionSelection,
    ) -> Result<(), SessionsCommandError> {
        if matches!(
            &selection.source_context,
            Some(crate::presentation::session_picker::PickerSourceContext::ConfiguredHosted(_))
        ) {
            return Err(SessionsCommandError::SessionSourceUnqualified);
        }
        self.launch_target.ensure_profile_allows_remote_resume()?;
        self.launch_target
            .resolve_for_selection(&selection.identity)?;
        let launch = self.launch_target.resume_launch(
            codex_args,
            &selection.session_id(),
            &selection.model_choice,
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

    fn run_codex_fork(
        &mut self,
        codex_args: &[OsString],
        selection: &SessionActionSelection,
    ) -> Result<(), SessionsCommandError> {
        if matches!(
            &selection.source_context,
            Some(crate::presentation::session_picker::PickerSourceContext::ConfiguredHosted(_))
        ) {
            return Err(SessionsCommandError::SessionSourceUnqualified);
        }
        self.launch_target.ensure_profile_allows_remote_resume()?;
        self.launch_target
            .resolve_for_selection(&selection.identity)?;
        let launch = self.launch_target.fork_launch(
            codex_args,
            &selection.session_id(),
            &selection.model_choice,
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
