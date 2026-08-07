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
use codex_router_host::HostInstance;
use codex_router_host::HostRuntime;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::PreExecTelemetry;
use codex_router_host::TerminalClassification;
use codex_router_host::UpdateResult;
use thiserror::Error;

use crate::CliContext;

const DEFAULT_HOST_PORT: u16 = 8787;
const STATUS_REQUEST_DEADLINE: Duration = Duration::from_secs(40);
const APP_SERVER_RESTART_DEADLINE: Duration = Duration::from_secs(120);
const ROUTER_RESTART_DEADLINE: Duration = Duration::from_secs(30);
const UPDATE_REQUEST_DEADLINE: Duration = Duration::from_secs(17 * 60);
const REPLACEMENT_CONVERGENCE_DEADLINE: Duration = Duration::from_secs(40);
const REPLACEMENT_RECOVERY_ACTION: &str = "codex-router host";

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
            operator_request_deadline(command.action()),
        )
        .await?;
        if matches!(command.action(), HostAction::Update) {
            let result = complete_update_result(&coordination_paths, frames).await;
            crate::presentation::host::render_update_result(stdout, &result)?;
        } else {
            crate::presentation::host::render_frames(stdout, &frames)?;
        }
        return Ok(());
    }

    tokio::fs::create_dir_all(&router_root).await?;
    let inherited_marker = std::env::var_os(codex_router_host::inherited_lock_environment());
    let instance = match inherited_marker.as_deref() {
        Some(marker) => HostInstance::acquire_inherited(coordination_paths.clone(), marker),
        None => HostInstance::acquire(coordination_paths.clone()),
    }
    .map_err(codex_router_host::HostError::from)?;
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
    HostRuntime::run_acquired(config, dependencies, instance).await?;
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

async fn complete_update_result(
    coordination_paths: &HostCoordinationPaths,
    frames: Vec<OperatorFrame>,
) -> UpdateResult {
    complete_update_result_with_deadline(
        coordination_paths,
        frames,
        REPLACEMENT_CONVERGENCE_DEADLINE,
    )
    .await
}

async fn complete_update_result_with_deadline(
    coordination_paths: &HostCoordinationPaths,
    frames: Vec<OperatorFrame>,
    replacement_deadline: Duration,
) -> UpdateResult {
    let replacement_started = frames
        .iter()
        .any(|frame| matches!(frame, OperatorFrame::Progress(_)));
    if let Some(OperatorFrame::Terminal(response)) = frames.last() {
        if replacement_started {
            return UpdateResult::UpdatedButReplacementFailed {
                message: response.message().to_owned(),
                recovery_action: REPLACEMENT_RECOVERY_ACTION.to_owned(),
            };
        }
        return if response.classification() == TerminalClassification::Succeeded {
            UpdateResult::NoChange
        } else {
            UpdateResult::FailedWithoutRestart {
                message: response.message().to_owned(),
            }
        };
    }
    if !replacement_started {
        return UpdateResult::FailedWithoutRestart {
            message: "shared Codex host update returned no terminal result".to_owned(),
        };
    }

    match codex_router_host::send_operator_request(
        coordination_paths.operator_socket(),
        OperatorRequest::AwaitHostStart,
        replacement_deadline,
    )
    .await
    {
        Ok(replacement) => match replacement.last() {
            Some(OperatorFrame::Terminal(response))
                if matches!(
                    response.classification(),
                    TerminalClassification::Ready
                        | TerminalClassification::LocalReadyRemoteDegraded
                ) =>
            {
                UpdateResult::UpdatedAndHostRestarted {
                    snapshot: response.snapshot().clone(),
                }
            }
            Some(OperatorFrame::Terminal(response)) => UpdateResult::UpdatedButReplacementFailed {
                message: response.message().to_owned(),
                recovery_action: REPLACEMENT_RECOVERY_ACTION.to_owned(),
            },
            _ => UpdateResult::UpdatedButReplacementFailed {
                message: "replacement host returned no terminal readiness".to_owned(),
                recovery_action: REPLACEMENT_RECOVERY_ACTION.to_owned(),
            },
        },
        Err(_) => UpdateResult::UpdatedButReplacementFailed {
            message: "updated Codex but replacement host did not become ready".to_owned(),
            recovery_action: REPLACEMENT_RECOVERY_ACTION.to_owned(),
        },
    }
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

#[cfg(test)]
mod tests {
    use codex_router_host::AppServerCondition;
    use codex_router_host::ExecutableRelation;
    use codex_router_host::HostPhase;
    use codex_router_host::HostProgress;
    use codex_router_host::HostSnapshot;
    use codex_router_host::HostSnapshotDimensions;
    use codex_router_host::HostTerminalResponse;
    use codex_router_host::RecoveryBudget;
    use codex_router_host::RemoteControlCondition;
    use codex_router_host::RouterCondition;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

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
            REPLACEMENT_CONVERGENCE_DEADLINE,
            Duration::from_secs(40),
            "replacement convergence starts only after old-host EOF"
        );
    }

    #[tokio::test]
    async fn update_result_maps_post_change_terminal_failure_to_manual_recovery() {
        let paths = HostCoordinationPaths::new(
            PathBuf::from("/unused/operator.sock"),
            PathBuf::from("/unused/instance.lock"),
        );
        let result = complete_update_result_with_deadline(
            &paths,
            vec![
                OperatorFrame::Progress(HostProgress::ReplacementStarting),
                OperatorFrame::terminal(HostTerminalResponse::new(
                    OperatorRequest::UpdateCodex,
                    TerminalClassification::Failed,
                    ready_snapshot(),
                    "changed update teardown failed".to_owned(),
                )),
            ],
            Duration::from_millis(10),
        )
        .await;
        assert!(matches!(
            result,
            UpdateResult::UpdatedButReplacementFailed {
                recovery_action,
                ..
            } if recovery_action == REPLACEMENT_RECOVERY_ACTION
        ));
    }

    #[tokio::test]
    async fn update_result_maps_missing_replacement_endpoint_to_manual_recovery() {
        let directory =
            std::env::temp_dir().join(format!("codex-router-update-result-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("fixture directory must create");
        let paths = HostCoordinationPaths::new(
            directory.join("missing.sock"),
            directory.join("instance.lock"),
        );
        let result = complete_update_result_with_deadline(
            &paths,
            vec![OperatorFrame::Progress(HostProgress::ReplacementStarting)],
            Duration::from_millis(20),
        )
        .await;
        let _cleanup_result = std::fs::remove_dir(&directory);
        assert!(matches!(
            result,
            UpdateResult::UpdatedButReplacementFailed {
                recovery_action,
                ..
            } if recovery_action == REPLACEMENT_RECOVERY_ACTION
        ));
    }

    #[tokio::test]
    async fn update_result_maps_ready_replacement_to_updated_and_restarted()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::env::temp_dir().join(format!(
            "codex-router-update-success-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory)?;
        let paths = HostCoordinationPaths::new(
            directory.join("operator.sock"),
            directory.join("instance.lock"),
        );
        let listener = tokio::net::UnixListener::bind(paths.operator_socket())?;
        let server = tokio::spawn(async move {
            let (mut stream, _peer) = listener.accept().await?;
            let mut request = Vec::new();
            stream.read_to_end(&mut request).await?;
            let decoded = codex_router_host::decode_operator_request(&request)
                .map_err(std::io::Error::other)?;
            if decoded != OperatorRequest::AwaitHostStart {
                return Err(std::io::Error::other("unexpected replacement request"));
            }
            let response = codex_router_host::encode_operator_frame(&OperatorFrame::terminal(
                HostTerminalResponse::new(
                    OperatorRequest::AwaitHostStart,
                    TerminalClassification::Ready,
                    ready_snapshot(),
                    "replacement ready".to_owned(),
                ),
            ))
            .map_err(std::io::Error::other)?;
            stream.write_all(&response).await?;
            stream.shutdown().await
        });

        let result = complete_update_result_with_deadline(
            &paths,
            vec![OperatorFrame::Progress(HostProgress::ReplacementStarting)],
            Duration::from_secs(1),
        )
        .await;
        server.await??;
        let _socket_cleanup = std::fs::remove_file(paths.operator_socket());
        let _directory_cleanup = std::fs::remove_dir(&directory);
        if !matches!(result, UpdateResult::UpdatedAndHostRestarted { .. }) {
            return Err("ready replacement did not produce updated-and-restarted".into());
        }
        Ok(())
    }

    fn ready_snapshot() -> HostSnapshot {
        HostSnapshot::new(HostSnapshotDimensions {
            phase: HostPhase::Steady,
            router: RouterCondition::OwnedReachable,
            app_server: AppServerCondition::NativeReady {
                running_version: "1.2.3".to_owned(),
            },
            remote_control: RemoteControlCondition::Connected,
            executable_relation: ExecutableRelation::Match,
            recovery_budget: RecoveryBudget::Available,
            last_lifecycle_outcome: None,
        })
    }
}
