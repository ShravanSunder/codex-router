//! Native async composition and operator commands for the foreground shared host.

use std::ffi::OsString;
use std::io::Write;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use clap::Subcommand;
use codex_router_host::HostCoordinationPaths;
use codex_router_host::OperatorRequest;
use thiserror::Error;

use crate::CliContext;
use operator_client::OperatorClientError;
use operator_client::send_operator_request;

mod foreground_launch;
pub(crate) mod operator_client;
mod update_outcome;

const DEFAULT_HOST_PORT: u16 = 8787;
const STATUS_REQUEST_DEADLINE: Duration = Duration::from_secs(40);
const APP_SERVER_RESTART_DEADLINE: Duration = Duration::from_secs(120);
const ROUTER_RESTART_DEADLINE: Duration = Duration::from_secs(30);
const UPDATE_REQUEST_DEADLINE: Duration = Duration::from_secs(17 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub(crate) enum HostAction {
    Status,
    Restart,
    RestartRouter,
    Update,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostCommand {
    action: Option<HostAction>,
    router_root: Option<PathBuf>,
    port: u16,
}

impl HostCommand {
    pub(crate) fn parse(arguments: Vec<OsString>) -> Result<Self, String> {
        let mut argv = vec![OsString::from("host")];
        argv.extend(arguments);
        let parsed = ClapHostCommand::try_parse_from(argv).map_err(|error| error.to_string())?;
        Ok(Self {
            action: parsed.action,
            router_root: parsed.router_root,
            port: parsed.port,
        })
    }

    pub(crate) const fn action(&self) -> HostAction {
        match self.action {
            Some(action) => action,
            None => HostAction::Status,
        }
    }

    #[cfg(test)]
    pub(crate) fn router_root(&self) -> Option<&Path> {
        self.router_root.as_deref()
    }

    pub(crate) const fn runs_foreground(&self) -> bool {
        self.action.is_none()
    }
}

#[derive(Debug, Parser)]
#[command(name = "host", disable_help_subcommand = true)]
struct ClapHostCommand {
    #[command(subcommand)]
    action: Option<HostAction>,
    #[arg(long, global = true)]
    router_root: Option<PathBuf>,
    #[arg(long, default_value_t = DEFAULT_HOST_PORT, global = true)]
    port: u16,
}

pub(crate) async fn run_host_command<W: Write>(
    stdout: &mut W,
    command: HostCommand,
    context: &CliContext,
    telemetry: Option<crate::telemetry::TelemetryShutdownHandle>,
) -> Result<(), HostCommandError> {
    let router_root = crate::router_root_or_default(command.router_root.clone())
        .map_err(|error| HostCommandError::RouterRoot(error.to_string()))?;
    let coordination_paths =
        HostCoordinationPaths::new(router_root.join("host.sock"), router_root.join("host.lock"));
    if command.runs_foreground() {
        return foreground_launch::run_foreground_host(
            router_root,
            command.port,
            coordination_paths,
            context,
            telemetry,
        )
        .await;
    }

    let request = match command.action() {
        HostAction::Status => OperatorRequest::Status,
        HostAction::Restart => OperatorRequest::RestartAppServer,
        HostAction::RestartRouter => OperatorRequest::RestartRouter,
        HostAction::Update => OperatorRequest::UpdateCodex,
    };
    let frames = send_operator_request(
        coordination_paths.operator_socket(),
        request,
        operator_request_deadline(command.action()),
    )
    .await?;
    if matches!(command.action(), HostAction::Update) {
        let result = update_outcome::complete_update_result(&coordination_paths, frames).await;
        crate::presentation::host::render_update_result(stdout, &result)?;
    } else {
        crate::presentation::host::render_frames(stdout, &frames)?;
    }
    Ok(())
}

const fn operator_request_deadline(action: HostAction) -> Duration {
    match action {
        HostAction::Status => STATUS_REQUEST_DEADLINE,
        HostAction::Restart => APP_SERVER_RESTART_DEADLINE,
        HostAction::RestartRouter => ROUTER_RESTART_DEADLINE,
        HostAction::Update => UPDATE_REQUEST_DEADLINE,
    }
}

#[derive(Debug, Error)]
pub enum HostCommandError {
    #[error("failed resolving host router root: {0}")]
    RouterRoot(String),
    #[error("HOME and CODEX_HOME are unavailable")]
    CodexHomeUnavailable,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Codex(#[from] codex_router_codex::ExecutableIdentityError),
    #[error(transparent)]
    Operator(#[from] OperatorClientError),
    #[error(transparent)]
    Runtime(#[from] codex_router_host::HostError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_deadlines_cover_their_owned_lifecycle_bounds() {
        assert!(
            operator_request_deadline(HostAction::Restart) > Duration::from_secs(70),
            "app-server restart must outlive upstream's complete shutdown bound"
        );
        assert!(
            operator_request_deadline(HostAction::Update) > Duration::from_secs(15 * 60),
            "update transport must outlive the updater's own deadline"
        );
        assert_eq!(
            update_outcome::REPLACEMENT_CONVERGENCE_DEADLINE,
            Duration::from_secs(40),
            "replacement convergence starts only after old-host EOF"
        );
    }
}
