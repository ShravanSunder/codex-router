//! Native async composition and operator commands for the foreground shared host.

use std::ffi::OsString;
use std::io::Write;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use clap::Subcommand;
use codex_router_codex::AppServerCommandSpec;
use codex_router_codex::CodexPaths;
use codex_router_codex::CodexRouterProfile;
use codex_router_host::AppServerLaunchPlan;
use codex_router_host::ChildCommandSpec;
use codex_router_host::HostConfig;
use codex_router_host::HostConfigInputs;
use codex_router_host::HostCoordinationPaths;
use codex_router_host::HostDeadlines;
use codex_router_host::HostDependencies;
use codex_router_host::HostDependenciesInputs;
use codex_router_host::HostRuntime;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::PreExecTelemetry;
use thiserror::Error;

use crate::CliContext;

const DEFAULT_HOST_PORT: u16 = 8787;
const OPERATOR_REQUEST_DEADLINE: Duration = Duration::from_secs(40);

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

struct HostPreExecTelemetry(crate::telemetry::TelemetryShutdownHandle);

impl PreExecTelemetry for HostPreExecTelemetry {
    fn flush_and_shutdown(&self) {
        self.0.flush_and_shutdown();
    }
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
    if !command.runs_foreground() {
        let request = match command.action() {
            HostAction::Status => OperatorRequest::Status,
            HostAction::Restart => OperatorRequest::RestartAppServer,
            HostAction::RestartRouter => OperatorRequest::RestartRouter,
            HostAction::Update => OperatorRequest::UpdateCodex,
        };
        let frames = codex_router_host::send_operator_request(
            coordination_paths.operator_socket(),
            request,
            OPERATOR_REQUEST_DEADLINE,
        )
        .await?;
        crate::presentation::host::render_frames(stdout, &frames)?;
        if matches!(command.action(), HostAction::Update)
            && matches!(frames.as_slice(), [OperatorFrame::Progress(_)])
        {
            let replacement = codex_router_host::send_operator_request(
                coordination_paths.operator_socket(),
                OperatorRequest::AwaitHostStart,
                OPERATOR_REQUEST_DEADLINE,
            )
            .await?;
            crate::presentation::host::render_frames(stdout, &replacement)?;
        }
        return Ok(());
    }

    tokio::fs::create_dir_all(&router_root).await?;
    let codex_paths = CodexPaths::from_codex_home(resolve_codex_home(context)?);
    let profile = CodexRouterProfile::new(command.port);
    let app_server_spec = AppServerCommandSpec::new(&codex_paths, &profile);
    let running_identity =
        codex_router_codex::executable_identity(&codex_paths.managed_executable()).await?;
    let running_version =
        codex_router_codex::managed_executable_version(&codex_paths.managed_executable()).await?;
    let app_server = AppServerLaunchPlan::new(
        ChildCommandSpec::new(app_server_spec.executable())
            .with_arguments(app_server_spec.arguments()),
        running_identity,
        running_version,
    );
    let current_executable = std::env::current_exe()?;
    let router_command = ChildCommandSpec::new(current_executable.clone()).with_arguments([
        OsString::from("serve"),
        OsString::from("--port"),
        OsString::from(command.port.to_string()),
        OsString::from("--state-db"),
        router_root.join("state.sqlite").into_os_string(),
        OsString::from("--secret-root"),
        router_root.join("secrets").into_os_string(),
    ]);
    let replacement_command = ChildCommandSpec::new(current_executable).with_arguments([
        OsString::from("host"),
        OsString::from("--router-root"),
        router_root.clone().into_os_string(),
        OsString::from("--port"),
        OsString::from(command.port.to_string()),
    ]);
    let config = HostConfig::new(HostConfigInputs {
        coordination_paths,
        router_endpoint: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, command.port)),
        app_server_socket: codex_paths.app_server_socket(),
        managed_executable: codex_paths.managed_executable(),
        deadlines: HostDeadlines::production(),
    });
    let mut dependencies = HostDependencies::new(HostDependenciesInputs {
        router_command: Some(router_command),
        app_server,
    })
    .with_replacement_command(replacement_command);
    if let Some(telemetry) = telemetry {
        dependencies =
            dependencies.with_pre_exec_telemetry(Arc::new(HostPreExecTelemetry(telemetry)));
    }
    let inherited_marker = std::env::var_os(codex_router_host::inherited_lock_environment());
    if let Some(marker) = inherited_marker {
        HostRuntime::run_inherited(config, dependencies, &marker).await?;
    } else {
        HostRuntime::run(config, dependencies).await?;
    }
    Ok(())
}

fn resolve_codex_home(context: &CliContext) -> Result<PathBuf, HostCommandError> {
    if let Some(codex_home) = context.env_var("CODEX_HOME") {
        return Ok(PathBuf::from(codex_home));
    }
    context
        .env_var("HOME")
        .map(|home| PathBuf::from(home).join(".codex"))
        .ok_or(HostCommandError::CodexHomeUnavailable)
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
    Operator(#[from] codex_router_host::OperatorClientError),
    #[error(transparent)]
    Runtime(#[from] codex_router_host::HostError),
}
