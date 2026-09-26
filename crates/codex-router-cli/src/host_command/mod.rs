//! Native async composition and operator commands for the foreground shared host.

use std::ffi::OsString;
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
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

mod foreground_launch;
pub(crate) mod operator_client;
mod provider_launch_configuration;
pub(crate) mod replacement_outcome;

const DEFAULT_HOST_PORT: u16 = 8787;
const STATUS_REQUEST_DEADLINE: Duration = Duration::from_secs(40);
const APP_SERVER_RESTART_DEADLINE: Duration = Duration::from_secs(40);
const ROUTER_RESTART_DEADLINE: Duration = Duration::from_secs(30);
const UPDATE_REQUEST_DEADLINE: Duration = Duration::from_secs(17 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub(crate) enum HostAction {
    /// Observe the running Host and managed Codex.
    Status,
    /// Replace the whole Host with this installed CLI and wait for readiness.
    Restart,
    /// Restart the router child when owned by this Host.
    Router {
        #[command(subcommand)]
        action: RouterAction,
    },
    /// Restart or update the managed Codex app-server.
    AppServer {
        #[command(subcommand)]
        action: AppServerAction,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub(crate) enum AppServerAction {
    /// Restart managed Codex without updating it or replacing the Host.
    Restart,
    /// Update managed Codex and activate it if changed.
    Update,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub(crate) enum RouterAction {
    /// Restart the router child when owned by this Host.
    Restart,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostCommand {
    action: Option<HostAction>,
    router_root: Option<PathBuf>,
    port: Option<u16>,
    mcp_bind: Option<SocketAddr>,
    provider_operation_retention_days: std::num::NonZeroU32,
    claude_acp_executable: Option<PathBuf>,
    claude_acp_arguments: Vec<String>,
    cursor_acp_executable: Option<PathBuf>,
    cursor_acp_arguments: Vec<String>,
    require_debug_isolation: bool,
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
            mcp_bind: parsed.mcp_bind,
            provider_operation_retention_days: parsed.provider_operation_retention_days,
            claude_acp_executable: parsed.claude_acp_executable,
            claude_acp_arguments: parsed.claude_acp_arguments,
            cursor_acp_executable: parsed.cursor_acp_executable,
            cursor_acp_arguments: parsed.cursor_acp_arguments,
            require_debug_isolation: parsed.require_debug_isolation,
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
    #[cfg(test)]
    pub(crate) const fn mcp_bind(&self) -> Option<SocketAddr> {
        self.mcp_bind
    }
    #[cfg(test)]
    pub(crate) const fn provider_operation_retention_days(&self) -> std::num::NonZeroU32 {
        self.provider_operation_retention_days
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
    #[arg(
        long,
        global = true,
        help = "Provider port (debug default: 18787; installed default: 8787)"
    )]
    port: Option<u16>,
    #[arg(long, global = true, value_parser = parse_loopback_mcp_bind)]
    mcp_bind: Option<SocketAddr>,
    #[arg(long, global = true, default_value = "60")]
    provider_operation_retention_days: std::num::NonZeroU32,
    #[arg(long, global = true)]
    claude_acp_executable: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        allow_hyphen_values = true,
        requires = "claude_acp_executable"
    )]
    claude_acp_arguments: Vec<String>,
    #[arg(long, global = true)]
    cursor_acp_executable: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        allow_hyphen_values = true,
        requires = "cursor_acp_executable"
    )]
    cursor_acp_arguments: Vec<String>,
    #[arg(long, global = true, hide = true)]
    require_debug_isolation: bool,
}

pub(crate) async fn run_host_command<W: Write + Send>(
    stdout: &mut W,
    command: HostCommand,
    context: &CliContext,
    telemetry: Option<crate::telemetry::TelemetryShutdownHandle>,
) -> Result<(), HostCommandError> {
    if context
        .env_var("CODEX_ROUTER_DEBUG_RUNNING_VERSION")
        .is_some()
        && (!cfg!(debug_assertions) || !command.require_debug_isolation)
    {
        return Err(HostCommandError::RouterRoot(
            "debug running-version override requires --require-debug-isolation in a debug build"
                .to_owned(),
        ));
    }
    if command.require_debug_isolation
        && (!cfg!(all(debug_assertions, not(test)))
            || context.env_var(crate::USE_HOME_DEFAULT_ENV).is_some())
    {
        return Err(HostCommandError::RouterRoot(
            "debug isolation requires a debug build without home-default mode".to_owned(),
        ));
    }
    let router_root = crate::router_root_or_default(command.router_root.clone())
        .map_err(|error| HostCommandError::RouterRoot(error.to_string()))?;
    let coordination_paths =
        HostCoordinationPaths::new(router_root.join("host.sock"), router_root.join("host.lock"));
    if command.runs_foreground() {
        let provider_launches = external_provider_launches(&command)?;
        return foreground_launch::run_foreground_host(
            foreground_launch::ForegroundHostInputs {
                router_root,
                port: command.port.unwrap_or_else(|| {
                    if cfg!(all(debug_assertions, not(test)))
                        && context.env_var(crate::USE_HOME_DEFAULT_ENV).is_none()
                    {
                        18787
                    } else {
                        DEFAULT_HOST_PORT
                    }
                }),
                mcp_bind: command.mcp_bind.unwrap_or_else(|| {
                    default_mcp_bind(
                        cfg!(all(debug_assertions, not(test)))
                            && context.env_var(crate::USE_HOME_DEFAULT_ENV).is_none(),
                    )
                }),
                provider_operation_retention_days: command.provider_operation_retention_days,
                coordination_paths,
                external_provider_launches: provider_launches,
            },
            context,
            telemetry,
        )
        .await;
    }

    let request = match command.action() {
        HostAction::Status => OperatorRequest::Status,
        HostAction::Restart => OperatorRequest::RestartHost {
            executable: std::env::current_exe()?,
        },
        HostAction::Router {
            action: RouterAction::Restart,
        } => OperatorRequest::RestartRouter,
        HostAction::AppServer {
            action: AppServerAction::Restart,
        } => OperatorRequest::RestartAppServer,
        HostAction::AppServer {
            action: AppServerAction::Update,
        } => OperatorRequest::UpdateCodex,
    };
    let mut progress_presenter =
        crate::presentation::host::HostProgressPresenter::new(context.stdout_is_terminal());
    let frames = operator_client::send_operator_request_streaming(
        coordination_paths.operator_socket(),
        request,
        operator_request_deadline(command.action()),
        |frame| { let _ = progress_presenter.accept(stdout, frame); },
    )
    .await
    .map_err(|error| {
        if command.action() == HostAction::Restart
            && matches!(error, OperatorClientError::MissingTerminal)
        {
            HostCommandError::RestartFailed(
                "Host returned no restart result; it may predate whole-Host restart support. For the first upgrade, stop the foreground Host in its owning terminal, wait for exit, then start the installed `codex-router host`. No restart was retried.".to_owned(),
            )
        } else {
            HostCommandError::Operator(error)
        }
    })?;
    if matches!(
        command.action(),
        HostAction::AppServer {
            action: AppServerAction::Update
        }
    ) {
        let result = replacement_outcome::complete_update_result_with_progress(
            &coordination_paths,
            frames,
            |frame| {
                let _ = progress_presenter.accept(stdout, frame);
            },
        )
        .await;
        crate::presentation::host::render_update_result(stdout, &result)?;
        if let codex_router_host::UpdateResult::FailedWithoutRestart { message }
        | codex_router_host::UpdateResult::UpdatedButReplacementFailed { message, .. } = result
        {
            return Err(HostCommandError::OperationFailed(message));
        }
    } else if command.action() == HostAction::Restart {
        let result = replacement_outcome::complete_restart_result_with_progress(
            &coordination_paths,
            frames,
            |frame| {
                let _ = progress_presenter.accept(stdout, frame);
            },
        )
        .await;
        crate::presentation::host::render_restart_result(stdout, &result)?;
        if let Some(message) = result.failure_message() {
            return Err(HostCommandError::RestartFailed(message.to_owned()));
        }
    } else {
        crate::presentation::host::render_terminal_frame(stdout, &frames)?;
        if command.action() != HostAction::Status
            && let Some(codex_router_host::OperatorFrame::Terminal(response)) = frames.last()
            && matches!(
                response.classification(),
                codex_router_host::TerminalClassification::Failed
                    | codex_router_host::TerminalClassification::Busy
                    | codex_router_host::TerminalClassification::Unavailable
            )
        {
            return Err(HostCommandError::OperationFailed(
                response.message().to_owned(),
            ));
        }
    }
    Ok(())
}

fn external_provider_launches(
    command: &HostCommand,
) -> Result<Vec<codex_router_host::ExternalProviderLaunchBinding>, HostCommandError> {
    let mut launches = Vec::new();
    if let Some(executable) = command.claude_acp_executable.clone() {
        launches.push(
            codex_router_host::ExternalProviderLaunchBinding::claude(
                executable,
                command.claude_acp_arguments.clone(),
            )
            .map_err(|error| HostCommandError::RouterRoot(error.to_owned()))?,
        );
    }
    if let Some(executable) = command.cursor_acp_executable.clone() {
        launches.push(
            codex_router_host::ExternalProviderLaunchBinding::cursor(
                executable,
                command.cursor_acp_arguments.clone(),
            )
            .map_err(|error| HostCommandError::RouterRoot(error.to_owned()))?,
        );
    }
    Ok(launches)
}

pub(crate) const fn default_mcp_bind(isolated_debug: bool) -> SocketAddr {
    if isolated_debug {
        SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 18788)
    } else {
        SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8788)
    }
}

fn parse_loopback_mcp_bind(value: &str) -> Result<SocketAddr, String> {
    let address: SocketAddr = value
        .parse()
        .map_err(|_| "--mcp-bind requires HOST:PORT".to_owned())?;
    if !matches!(address.ip(), IpAddr::V4(ip) if ip.is_loopback())
        && !matches!(address.ip(), IpAddr::V6(ip) if ip.is_loopback())
    {
        return Err("--mcp-bind requires a loopback address".to_owned());
    }
    Ok(address)
}

const fn operator_request_deadline(action: HostAction) -> Duration {
    match action {
        HostAction::Status => STATUS_REQUEST_DEADLINE,
        HostAction::Restart => Duration::from_secs(150),
        HostAction::AppServer {
            action: AppServerAction::Restart,
        } => APP_SERVER_RESTART_DEADLINE,
        HostAction::Router {
            action: RouterAction::Restart,
        } => ROUTER_RESTART_DEADLINE,
        HostAction::AppServer {
            action: AppServerAction::Update,
        } => UPDATE_REQUEST_DEADLINE,
    }
}

#[derive(Debug, Error)]
pub enum HostCommandError {
    #[error("Host restart failed: {0}")]
    RestartFailed(String),
    #[error("Host operation failed: {0}")]
    OperationFailed(String),
    #[error(transparent)]
    DebugProfile(#[from] codex_native_integration::DebugProfileError),
    #[error("failed resolving host router root: {0}")]
    RouterRoot(String),
    #[error("HOME and CODEX_HOME are unavailable")]
    CodexHomeUnavailable,
    #[error("invalid debug app-server socket: {0}")]
    AppServerSocket(String),
    #[error("CODEX_ROUTER_DEBUG_LAUNCHCTL must be an absolute path")]
    LaunchctlExecutable,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Codex(#[from] codex_native_integration::ExecutableIdentityError),
    #[error(transparent)]
    DesktopLaunchPolicy(#[from] codex_native_integration::DesktopLaunchPolicyError),
    #[error(transparent)]
    Operator(#[from] OperatorClientError),
    #[error(transparent)]
    ControlSocket(#[from] codex_native_integration::RouterControlSocketError),
    #[error(transparent)]
    Runtime(#[from] codex_router_host::HostError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_deadlines_cover_their_owned_lifecycle_bounds() {
        assert!(
            operator_request_deadline(HostAction::Restart)
                > codex_router_host::APP_SERVER_SHUTDOWN_TIMEOUT,
            "whole-Host restart must outlive the app-server shutdown bound"
        );
        assert!(
            operator_request_deadline(HostAction::AppServer {
                action: AppServerAction::Restart
            }) > codex_router_host::APP_SERVER_SHUTDOWN_TIMEOUT,
            "app-server restart must outlive its complete shutdown bound"
        );
        assert!(
            operator_request_deadline(HostAction::AppServer {
                action: AppServerAction::Update
            }) > Duration::from_secs(15 * 60),
            "update transport must outlive the updater's own deadline"
        );
        assert_eq!(
            replacement_outcome::REPLACEMENT_CONVERGENCE_DEADLINE,
            Duration::from_secs(40),
            "replacement convergence starts only after old-host EOF"
        );
    }

    #[test]
    fn provider_launch_arguments_require_and_retain_explicit_executables() {
        let command = HostCommand::parse(vec![
            OsString::from("--claude-acp-executable"),
            OsString::from("/tmp/claude-agent-acp"),
            OsString::from("--claude-acp-arguments"),
            OsString::from("--permission-mode"),
            OsString::from("--cursor-acp-executable"),
            OsString::from("/tmp/agent"),
            OsString::from("--cursor-acp-arguments"),
            OsString::from("acp"),
        ])
        .expect("provider launch arguments");

        let launches = external_provider_launches(&command).expect("validated launches");
        assert_eq!(launches.len(), 2);
        assert_eq!(launches[0].launch.arguments, vec!["--permission-mode"]);
        assert_eq!(launches[1].launch.arguments, vec!["acp"]);
    }

    #[test]
    fn provider_operation_retention_is_positive_configurable_and_defaults_to_sixty_days() {
        let default = HostCommand::parse(Vec::new()).expect("default host command");
        assert_eq!(default.provider_operation_retention_days().get(), 60);
        let custom = HostCommand::parse(vec![
            OsString::from("--provider-operation-retention-days"),
            OsString::from("7"),
        ])
        .expect("custom retention");
        assert_eq!(custom.provider_operation_retention_days().get(), 7);
        assert!(
            HostCommand::parse(vec![
                OsString::from("--provider-operation-retention-days"),
                OsString::from("0"),
            ])
            .is_err()
        );
    }
}
