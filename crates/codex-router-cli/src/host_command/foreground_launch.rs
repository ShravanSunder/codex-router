//! Foreground host dependency projection and lifecycle launch.

use std::ffi::OsString;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::sync::Arc;

use codex_router_codex::AppServerCommandSpec;
use codex_router_codex::CodexPaths;
use codex_router_codex::CodexRouterProfile;
use codex_router_host::AppServerLaunchPlan;
use codex_router_host::ChildCommandSpec;
use codex_router_host::HostConfig;
use codex_router_host::HostConfigInputs;
use codex_router_host::HostCoordinationPaths;
use codex_router_host::HostDeadlines;
use codex_router_host::HostInstance;
use codex_router_host::HostRuntime;
use codex_router_host::ManagedChildLaunchPlans;
use codex_router_host::ManagedUpdateInputs;
use codex_router_host::PreExecTelemetry;

use super::HostCommandError;
use crate::CliContext;

struct HostPreExecTelemetry(crate::telemetry::TelemetryShutdownHandle);

impl PreExecTelemetry for HostPreExecTelemetry {
    fn flush_and_shutdown(&self) {
        self.0.flush_and_shutdown();
    }
}

pub(super) async fn run_foreground_host(
    router_root: PathBuf,
    port: u16,
    coordination_paths: HostCoordinationPaths,
    context: &CliContext,
    telemetry: Option<crate::telemetry::TelemetryShutdownHandle>,
) -> Result<(), HostCommandError> {
    tokio::fs::create_dir_all(&router_root).await?;
    let inherited_marker = std::env::var_os(codex_router_host::inherited_lock_environment());
    let instance = match inherited_marker.as_deref() {
        Some(marker) => HostInstance::acquire_inherited(coordination_paths.clone(), marker),
        None => HostInstance::acquire(coordination_paths.clone()),
    }
    .map_err(codex_router_host::HostError::from)?;
    let codex_paths = CodexPaths::from_codex_home(resolve_codex_home(context)?);
    let app_server_socket = crate::app_server_socket_or_default(context, &codex_paths)
        .map_err(|message| HostCommandError::AppServerSocket(message.to_owned()))?;
    let profile = CodexRouterProfile::new(port);
    let app_server_spec = AppServerCommandSpec::new(&codex_paths, &profile, &app_server_socket);
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
        OsString::from(port.to_string()),
        OsString::from("--state-db"),
        router_root.join("state.sqlite").into_os_string(),
        OsString::from("--secret-root"),
        router_root.join("secrets").into_os_string(),
    ]);
    let replacement_command = ChildCommandSpec::new(current_executable).with_arguments([
        OsString::from("host"),
        OsString::from("--router-root"),
        router_root.into_os_string(),
        OsString::from("--port"),
        OsString::from(port.to_string()),
    ]);
    let config = HostConfig::new(HostConfigInputs {
        coordination_paths,
        router_endpoint: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)),
        app_server_socket,
        managed_executable: codex_paths.managed_executable(),
        deadlines: HostDeadlines::production(),
    });
    let child_launch_plans = ManagedChildLaunchPlans::new(Some(router_command), app_server);
    let mut update_inputs =
        ManagedUpdateInputs::production().with_replacement_command(replacement_command);
    if let Some(telemetry) = telemetry {
        update_inputs =
            update_inputs.with_pre_exec_telemetry(Arc::new(HostPreExecTelemetry(telemetry)));
    }
    HostRuntime::run_acquired(config, child_launch_plans, update_inputs, instance).await?;
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
