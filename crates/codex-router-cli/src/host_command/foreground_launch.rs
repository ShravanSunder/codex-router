//! Foreground host dependency projection and lifecycle launch.

use std::ffi::OsString;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::sync::Arc;

use codex_native_integration::AppServerCommandSpec;
use codex_native_integration::CodexPaths;
use codex_native_integration::CodexRouterProfile;
use codex_native_integration::DesktopLaunchPolicyCommand;
use codex_router_host::AppServerLaunchPlan;
use codex_router_host::ChildCommandSpec;
use codex_router_host::ChildOutput;
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
    let isolated_debug = cfg!(all(debug_assertions, not(test)))
        && context.env_var(crate::USE_HOME_DEFAULT_ENV).is_none();
    if isolated_debug {
        let home = context
            .env_var("HOME")
            .ok_or(HostCommandError::CodexHomeUnavailable)?;
        codex_native_integration::validate_debug_directory(
            &router_root,
            &PathBuf::from(home).join(".codex-router"),
        )
        .map_err(|message| HostCommandError::RouterRoot(message.to_owned()))?;
    }
    let codex_home = resolve_codex_home(context)?;
    let codex_paths = CodexPaths::from_codex_home(codex_home.clone());
    let app_server_socket = crate::app_server_socket_or_default(context, &codex_paths)
        .map_err(|message| HostCommandError::AppServerSocket(message.to_owned()))?;
    let profile = CodexRouterProfile::new(port);
    let app_server_spec = AppServerCommandSpec::new(&codex_paths, &profile, &app_server_socket);
    let app_server_spec = if isolated_debug {
        app_server_spec.with_debug_profile(&codex_native_integration::DebugCodexProfile::read(
            &codex_home,
            port,
        )?)
    } else {
        app_server_spec
    };
    // Validate the native destination before touching state or launch policy.
    tokio::fs::create_dir_all(&router_root).await?;
    if !isolated_debug {
        DesktopLaunchPolicyCommand::new(launchctl_executable(context)?)
            .apply()
            .await?;
    }
    let inherited_marker = std::env::var_os(codex_router_host::inherited_lock_environment());
    let instance = match inherited_marker.as_deref() {
        Some(marker) => HostInstance::acquire_inherited(coordination_paths.clone(), marker),
        None => HostInstance::acquire(coordination_paths.clone()),
    }
    .map_err(codex_router_host::HostError::from)?;
    let running_identity =
        codex_native_integration::executable_identity(&codex_paths.managed_executable()).await?;
    let running_version =
        codex_native_integration::managed_executable_version(&codex_paths.managed_executable())
            .await?;
    let mut app_server_command = ChildCommandSpec::new(app_server_spec.executable())
        .with_arguments(app_server_spec.arguments())
        .with_output(ChildOutput::Telemetry);
    for (key, value) in app_server_spec.environment() {
        app_server_command = app_server_command.with_environment(key, value);
    }
    let mut app_server =
        AppServerLaunchPlan::new(app_server_command, running_identity, running_version)
            .with_schema_directory(router_root.join("agent-communication"));
    app_server.prepare_schema().await;
    let current_executable = std::env::current_exe()?;
    let communication_directory = router_root.join("agent-communication");
    let otlp_endpoint = crate::telemetry::foreground_host_otlp_endpoint(
        context.env_var("OTEL_EXPORTER_OTLP_ENDPOINT"),
    );
    let router_command = ChildCommandSpec::new(current_executable.clone())
        .with_arguments([
            OsString::from("serve"),
            OsString::from("--port"),
            OsString::from(port.to_string()),
            OsString::from("--state-db"),
            router_root.join("state.sqlite").into_os_string(),
            OsString::from("--secret-root"),
            router_root.join("secrets").into_os_string(),
        ])
        .with_environment("OTEL_EXPORTER_OTLP_ENDPOINT", otlp_endpoint)
        .with_environment("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf");
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
    })
    .with_communication_directory(communication_directory, codex_home);
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

fn launchctl_executable(context: &CliContext) -> Result<PathBuf, HostCommandError> {
    #[cfg(debug_assertions)]
    if let Some(debug_executable) = context.env_var("CODEX_ROUTER_DEBUG_LAUNCHCTL") {
        let debug_executable = PathBuf::from(debug_executable);
        if !debug_executable.is_absolute() {
            return Err(HostCommandError::LaunchctlExecutable);
        }
        return Ok(debug_executable);
    }

    Ok(PathBuf::from("/bin/launchctl"))
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
