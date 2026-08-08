//! Safe retained Tokio child and exact Unix process-group signal primitives.

use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::process::Stdio;

use rustix::process::Pid;
use rustix::process::Signal;
use thiserror::Error;
use tokio::process::Child;
use tokio::process::Command;

/// Child stdout/stderr behavior selected by the composition root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildOutput {
    /// Preserve the foreground process's output streams.
    Inherit,
    /// Discard fixture or explicitly quiet child output.
    Null,
}

/// Cloneable exact child command projection for restart/recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildCommandSpec {
    executable: PathBuf,
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    output: ChildOutput,
}

impl ChildCommandSpec {
    /// Creates an exact executable projection without PATH re-resolution.
    #[must_use]
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            arguments: Vec::new(),
            environment: Vec::new(),
            output: ChildOutput::Inherit,
        }
    }

    /// Replaces the complete child argument vector.
    #[must_use]
    pub fn with_arguments<TArguments, TArgument>(mut self, arguments: TArguments) -> Self
    where
        TArguments: IntoIterator<Item = TArgument>,
        TArgument: Into<OsString>,
    {
        self.arguments = arguments.into_iter().map(Into::into).collect();
        self
    }

    /// Adds one explicit environment value required by the child projection.
    #[must_use]
    pub fn with_environment<TKey, TValue>(mut self, key: TKey, value: TValue) -> Self
    where
        TKey: Into<OsString>,
        TValue: AsRef<OsStr>,
    {
        self.environment
            .push((key.into(), value.as_ref().to_os_string()));
        self
    }

    /// Selects inherited or discarded child output.
    #[must_use]
    pub const fn with_output(mut self, output: ChildOutput) -> Self {
        self.output = output;
        self
    }

    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.args(&self.arguments).envs(self.environment.clone());
        if self.output == ChildOutput::Null {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        command
    }

    pub(crate) fn std_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.executable);
        command.args(&self.arguments).envs(self.environment.clone());
        if self.output == ChildOutput::Null {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        command
    }
}

/// Retained child started in a process group isolated from the foreground host.
pub struct ProcessGroupChild {
    child: Child,
    process_id: Pid,
    process_group_id: Pid,
}

impl ProcessGroupChild {
    /// Spawns one child as the leader of its own process group.
    pub fn spawn(command: &mut Command) -> Result<Self, ProcessGroupError> {
        command.process_group(0);
        let child = command.spawn().map_err(ProcessGroupError::Spawn)?;
        let raw_process_id = child.id().ok_or(ProcessGroupError::MissingProcessId)?;
        let signed_process_id = i32::try_from(raw_process_id)
            .map_err(|_error| ProcessGroupError::InvalidProcessId(raw_process_id))?;
        let process_id = Pid::from_raw(signed_process_id)
            .ok_or(ProcessGroupError::InvalidProcessId(raw_process_id))?;
        Ok(Self {
            child,
            process_id,
            process_group_id: process_id,
        })
    }

    /// Returns the exact retained child PID.
    #[must_use]
    pub fn process_id(&self) -> u32 {
        self.process_id.as_raw_nonzero().get().unsigned_abs()
    }

    /// Returns the isolated child process-group ID.
    #[must_use]
    pub fn process_group_id(&self) -> u32 {
        self.process_group_id.as_raw_nonzero().get().unsigned_abs()
    }

    /// Sends SIGTERM to the exact retained child PID.
    pub fn send_terminate(&self) -> Result<(), ProcessGroupError> {
        rustix::process::kill_process(self.process_id, Signal::TERM)
            .map_err(ProcessGroupError::Signal)
    }

    /// Sends SIGKILL to the exact retained child PID.
    pub fn send_kill(&self) -> Result<(), ProcessGroupError> {
        rustix::process::kill_process(self.process_id, Signal::KILL)
            .map_err(ProcessGroupError::Signal)
    }

    /// Sends SIGTERM to the complete isolated child process group.
    pub fn send_group_terminate(&self) -> Result<(), ProcessGroupError> {
        rustix::process::kill_process_group(self.process_group_id, Signal::TERM)
            .map_err(ProcessGroupError::Signal)
    }

    /// Sends SIGKILL to the complete isolated child process group.
    pub fn send_group_kill(&self) -> Result<(), ProcessGroupError> {
        rustix::process::kill_process_group(self.process_group_id, Signal::KILL)
            .map_err(ProcessGroupError::Signal)
    }

    /// Observes and reaps an already-exited child without waiting.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, ProcessGroupError> {
        self.child.try_wait().map_err(ProcessGroupError::Wait)
    }

    /// Asynchronously waits for and reaps the retained child.
    pub async fn wait(&mut self) -> Result<ExitStatus, ProcessGroupError> {
        self.child.wait().await.map_err(ProcessGroupError::Wait)
    }
}

/// Process spawn, signal, or wait failure for an exactly retained child.
#[derive(Debug, Error)]
pub enum ProcessGroupError {
    /// Tokio failed to spawn the child.
    #[error("failed spawning isolated child process: {0}")]
    Spawn(#[source] std::io::Error),
    /// Spawned child did not expose a process identifier.
    #[error("spawned child did not expose a process identifier")]
    MissingProcessId,
    /// Child PID could not be represented by the platform signal API.
    #[error("spawned child process identifier is invalid: {0}")]
    InvalidProcessId(u32),
    /// Exact-PID signal failed.
    #[error("failed signalling retained child: {0}")]
    Signal(#[source] rustix::io::Errno),
    /// Async child observation or reap failed.
    #[error("failed waiting for retained child: {0}")]
    Wait(#[source] std::io::Error),
}
