//! Foreground host dependency projection and lifecycle launch.

use std::ffi::OsString;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::path::{Component, Path};
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

pub(super) struct ForegroundHostInputs {
    pub owner_human_id: Option<message_board::HumanId>,
    pub router_root: PathBuf,
    pub launch_mode: HostLaunchMode,
    pub owner_home: Option<PathBuf>,
    pub port: u16,
    pub mcp_bind: SocketAddr,
    pub provider_operation_retention_days: std::num::NonZeroU32,
    pub coordination_paths: HostCoordinationPaths,
    pub external_provider_launches: Vec<codex_router_host::ExternalProviderLaunchBinding>,
}

pub(super) async fn run_foreground_host(
    inputs: ForegroundHostInputs,
    context: &CliContext,
    telemetry: Option<crate::telemetry::TelemetryShutdownHandle>,
) -> Result<(), HostCommandError> {
    let ForegroundHostInputs {
        owner_human_id,
        router_root,
        launch_mode,
        owner_home,
        port,
        mcp_bind,
        provider_operation_retention_days,
        coordination_paths,
        external_provider_launches,
    } = inputs;
    let launch_started_at = std::time::Instant::now();
    let codex_home = resolve_codex_home(context)?;
    let isolated_debug = launch_mode.is_isolated();
    let debug_profile = if isolated_debug {
        let path = codex_home.join("codex-router-debug.config.toml");
        let profile = codex_native_integration::DebugCodexProfile::read(&codex_home, port)
            .map_err(|source| HostCommandError::IsolatedDebugProfile { path, source })?;
        Some(profile)
    } else {
        None
    };
    if isolated_debug {
        let protected_root = owner_home
            .ok_or(HostCommandError::OwnerHomeUnavailable)?
            .join(".codex-router");
        codex_native_integration::validate_debug_directory(&router_root, &protected_root)
            .map_err(|message| HostCommandError::RouterRoot(message.to_owned()))?;
    }
    let codex_paths = CodexPaths::from_codex_home(codex_home.clone());
    let app_server_socket =
        crate::app_server_socket_or_default(context, &codex_paths, isolated_debug)
            .map_err(|message| HostCommandError::AppServerSocket(message.to_owned()))?;
    let collaboration_directory = router_root.join("agent-communication");
    let profile = CodexRouterProfile::new(port);
    let app_server_spec = AppServerCommandSpec::new(&codex_paths, &profile, &app_server_socket);
    let app_server_spec = match debug_profile.as_ref() {
        Some(profile) => app_server_spec.with_debug_profile(profile),
        None => app_server_spec,
    };
    codex_router_host::record_debug_readiness_timing("profileAndSpec", launch_started_at);
    // Validate the native destination before touching state or launch policy.
    tokio::fs::create_dir_all(&router_root).await?;
    codex_router_host::record_debug_readiness_timing("routerRootReady", launch_started_at);
    apply_launch_policy(launch_mode, context).await?;
    let inherited_marker = std::env::var_os(codex_router_host::inherited_lock_environment());
    let instance = match inherited_marker.as_deref() {
        Some(marker) => HostInstance::acquire_inherited(coordination_paths.clone(), marker),
        None => HostInstance::acquire(coordination_paths.clone()),
    }
    .map_err(codex_router_host::HostError::from)?;
    codex_router_host::record_debug_readiness_timing("singletonAcquired", launch_started_at);
    let identity_started_at = std::time::Instant::now();
    let running_identity =
        codex_native_integration::executable_identity(&codex_paths.managed_executable()).await?;
    codex_router_host::record_debug_readiness_timing("executableIdentity", identity_started_at);
    let version_started_at = std::time::Instant::now();
    let running_version =
        codex_native_integration::managed_executable_version(&codex_paths.managed_executable())
            .await?;
    codex_router_host::record_debug_readiness_timing(
        "managedExecutableVersion",
        version_started_at,
    );
    let mut app_server_command = ChildCommandSpec::new(app_server_spec.executable())
        .with_arguments(app_server_spec.arguments())
        .with_output(ChildOutput::Telemetry);
    for (key, value) in app_server_spec.environment() {
        app_server_command = app_server_command.with_environment(key, value);
    }
    let app_server =
        AppServerLaunchPlan::new(app_server_command, running_identity, running_version)
            .with_schema_directory(router_root.join("agent-communication"));
    codex_router_host::record_debug_readiness_timing("appServerPlanBuilt", launch_started_at);
    // Schema export is optional raw-native enrichment; do not delay app-server
    // socket startup on this best-effort operation.
    let current_executable = std::env::current_exe()?;
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
        .with_environment("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .with_output(ChildOutput::Telemetry);
    let replacement_command = host_replacement_command(HostReplacementCommandInputs {
        executable: current_executable,
        router_root: router_root.clone(),
        port,
        mcp_bind,
        provider_operation_retention_days,
        owner_human_id: owner_human_id.as_ref(),
        external_provider_launches: &external_provider_launches,
        launch_mode,
    });
    let provider_startups = super::provider_launch_configuration::configured_provider_startups(
        &router_root,
        &external_provider_launches,
    )?;
    let mut config = HostConfig::new(HostConfigInputs {
        coordination_paths,
        router_endpoint: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)),
        mcp_bind,
        app_server_socket,
        managed_executable: codex_paths.managed_executable(),
        deadlines: HostDeadlines::production(),
    })
    .with_provider_operation_retention_days(provider_operation_retention_days)
    .with_collaboration_directory(collaboration_directory, codex_home);
    if let Some(owner_human_id) = owner_human_id {
        config = config.with_owner_human_id(owner_human_id);
    }
    for provider_startup in provider_startups {
        config = config.with_external_provider_startup(provider_startup);
    }
    let child_launch_plans = ManagedChildLaunchPlans::new(Some(router_command), app_server);
    let mut update_inputs =
        ManagedUpdateInputs::production().with_replacement_command(replacement_command);
    if let Some(telemetry) = telemetry {
        update_inputs =
            update_inputs.with_pre_exec_telemetry(Arc::new(HostPreExecTelemetry(telemetry)));
    }
    HostRuntime::run_acquired_with_progress(
        config,
        child_launch_plans,
        update_inputs,
        instance,
        None,
    )
    .await?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HostLaunchMode {
    OwnerProduction,
    IsolatedDebug,
}

impl HostLaunchMode {
    pub(super) fn resolve(
        router_root: &Path,
        context: &CliContext,
        require_debug_isolation: bool,
        owner_home: Option<&Path>,
    ) -> Self {
        let Some(owner_home) = owner_home else {
            return Self::IsolatedDebug;
        };
        let Some(environment_home) = context.env_var("HOME") else {
            return Self::IsolatedDebug;
        };
        if require_debug_isolation
            || normalized_path(Path::new(environment_home)) != normalized_path(owner_home)
            || normalized_path(router_root) != normalized_path(&owner_home.join(".codex-router"))
        {
            Self::IsolatedDebug
        } else {
            Self::OwnerProduction
        }
    }

    pub(super) const fn is_isolated(self) -> bool {
        matches!(self, Self::IsolatedDebug)
    }

    pub(super) const fn default_port(self) -> u16 {
        match self {
            Self::OwnerProduction => super::DEFAULT_HOST_PORT,
            Self::IsolatedDebug => 18787,
        }
    }
}

fn normalized_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

struct HostReplacementCommandInputs<'a> {
    executable: PathBuf,
    router_root: PathBuf,
    port: u16,
    mcp_bind: SocketAddr,
    provider_operation_retention_days: std::num::NonZeroU32,
    owner_human_id: Option<&'a message_board::HumanId>,
    external_provider_launches: &'a [codex_router_host::ExternalProviderLaunchBinding],
    launch_mode: HostLaunchMode,
}

fn host_replacement_command(inputs: HostReplacementCommandInputs<'_>) -> ChildCommandSpec {
    let HostReplacementCommandInputs {
        executable,
        router_root,
        port,
        mcp_bind,
        provider_operation_retention_days,
        owner_human_id,
        external_provider_launches,
        launch_mode,
    } = inputs;
    let mut arguments = vec![
        OsString::from("host"),
        OsString::from("--router-root"),
        router_root.into_os_string(),
        OsString::from("--port"),
        OsString::from(port.to_string()),
        OsString::from("--mcp-bind"),
        OsString::from(mcp_bind.to_string()),
        OsString::from("--provider-operation-retention-days"),
        OsString::from(provider_operation_retention_days.to_string()),
    ];
    if let Some(owner_human_id) = owner_human_id {
        arguments.push(OsString::from("--owner-human-id"));
        arguments.push(OsString::from(owner_human_id.as_str()));
    }
    if launch_mode.is_isolated() {
        arguments.push(OsString::from("--require-debug-isolation"));
    }
    for binding in external_provider_launches {
        let (executable_flag, argument_flag) = binding.command_flags();
        arguments.push(OsString::from(executable_flag));
        arguments.push(binding.launch.executable.clone().into_os_string());
        for argument in &binding.launch.arguments {
            arguments.push(OsString::from(argument_flag));
            arguments.push(OsString::from(argument));
        }
    }
    ChildCommandSpec::new(executable).with_arguments(arguments)
}

fn launchctl_executable(context: &CliContext) -> Result<PathBuf, HostCommandError> {
    #[cfg(not(debug_assertions))]
    let _ = context;

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

async fn apply_launch_policy(
    launch_mode: HostLaunchMode,
    context: &CliContext,
) -> Result<(), HostCommandError> {
    if launch_mode == HostLaunchMode::OwnerProduction {
        let started_at = std::time::Instant::now();
        DesktopLaunchPolicyCommand::new(launchctl_executable(context)?)
            .apply()
            .await?;
        codex_router_host::record_debug_readiness_timing("launchctlPolicy", started_at);
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

#[cfg(test)]
mod tests {
    use super::{
        HostLaunchMode, HostReplacementCommandInputs, apply_launch_policy, host_replacement_command,
    };
    use crate::CliContext;
    use codex_router_host::ChildCommandSpec;
    use std::{ffi::OsString, net::SocketAddr, path::PathBuf};

    #[test]
    fn forged_home_and_private_root_select_isolation() {
        let context = CliContext::new(vec![
            ("HOME".to_owned(), "/tmp/fixture-owner".to_owned()),
            ("CODEX_ROUTER_USE_HOME_DEFAULT".to_owned(), "1".to_owned()),
        ]);
        let owner_root = PathBuf::from("/tmp/fixture-owner/.codex-router");
        let owner_home = PathBuf::from("/Users/actual-owner");
        assert_eq!(
            HostLaunchMode::resolve(&owner_root, &context, false, Some(&owner_home)),
            HostLaunchMode::IsolatedDebug
        );
        assert_eq!(
            HostLaunchMode::resolve(
                &PathBuf::from("/tmp/fixture-owner/private-router"),
                &context,
                false,
                Some(&owner_home),
            ),
            HostLaunchMode::IsolatedDebug
        );
        assert_eq!(
            HostLaunchMode::resolve(&owner_root, &context, true, Some(&owner_home)),
            HostLaunchMode::IsolatedDebug
        );
        assert_eq!(
            HostLaunchMode::resolve(&owner_root, &context, false, None),
            HostLaunchMode::IsolatedDebug
        );
        let owner_context = CliContext::new(vec![
            ("HOME".to_owned(), owner_home.to_string_lossy().into_owned()),
            ("CODEX_HOME".to_owned(), "/tmp/custom-codex-home".to_owned()),
        ]);
        let owner_root = owner_home.join(".codex-router");
        assert_eq!(
            HostLaunchMode::resolve(&owner_root, &owner_context, false, Some(&owner_home)),
            HostLaunchMode::OwnerProduction,
        );
        assert_eq!(HostLaunchMode::OwnerProduction.default_port(), 8787);
        assert_eq!(HostLaunchMode::IsolatedDebug.default_port(), 18787);
    }

    #[tokio::test]
    async fn owner_mode_calls_launchctl_while_isolated_mode_skips_it() {
        let directory = tempfile::tempdir().expect("fixture directory");
        let fake_launchctl = directory.path().join("launchctl");
        std::fs::write(&fake_launchctl, "#!/bin/sh\nexit 1\n").expect("fixture executable");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_launchctl, std::fs::Permissions::from_mode(0o700))
            .expect("executable permission");
        let context = CliContext::new(vec![(
            "CODEX_ROUTER_DEBUG_LAUNCHCTL".to_owned(),
            fake_launchctl.to_string_lossy().into_owned(),
        )]);
        assert!(
            apply_launch_policy(HostLaunchMode::OwnerProduction, &context)
                .await
                .is_err()
        );
        assert!(
            apply_launch_policy(HostLaunchMode::IsolatedDebug, &context)
                .await
                .is_ok()
        );
    }

    #[test]
    fn replacement_command_preserves_explicit_mcp_bind() {
        let executable = PathBuf::from("/tmp/codex-router");
        let router_root = PathBuf::from("/tmp/router-root");
        let mcp_bind = SocketAddr::from(([127, 0, 0, 1], 19088));
        assert_eq!(
            host_replacement_command(HostReplacementCommandInputs {
                executable: executable.clone(),
                router_root: router_root.clone(),
                port: 19087,
                mcp_bind,
                provider_operation_retention_days: std::num::NonZeroU32::new(60).expect("positive"),
                owner_human_id: None,
                external_provider_launches: &[],
                launch_mode: HostLaunchMode::OwnerProduction,
            }),
            ChildCommandSpec::new(executable).with_arguments([
                OsString::from("host"),
                OsString::from("--router-root"),
                router_root.into_os_string(),
                OsString::from("--port"),
                OsString::from("19087"),
                OsString::from("--mcp-bind"),
                OsString::from("127.0.0.1:19088"),
                OsString::from("--provider-operation-retention-days"),
                OsString::from("60"),
            ])
        );
    }

    #[test]
    fn replacement_command_preserves_external_provider_launches() {
        let executable = PathBuf::from("/tmp/codex-router");
        let router_root = PathBuf::from("/tmp/router-root");
        let mcp_bind = SocketAddr::from(([127, 0, 0, 1], 19088));
        let provider = codex_router_host::ExternalProviderLaunchBinding::cursor(
            PathBuf::from("/tmp/agent"),
            vec!["acp".to_owned()],
        )
        .expect("cursor binding");
        let command = host_replacement_command(HostReplacementCommandInputs {
            executable: executable.clone(),
            router_root: router_root.clone(),
            port: 19087,
            mcp_bind,
            provider_operation_retention_days: std::num::NonZeroU32::new(60).expect("positive"),
            owner_human_id: None,
            external_provider_launches: &[provider],
            launch_mode: HostLaunchMode::OwnerProduction,
        });
        assert_eq!(
            command,
            ChildCommandSpec::new(executable).with_arguments([
                OsString::from("host"),
                OsString::from("--router-root"),
                router_root.into_os_string(),
                OsString::from("--port"),
                OsString::from("19087"),
                OsString::from("--mcp-bind"),
                OsString::from("127.0.0.1:19088"),
                OsString::from("--provider-operation-retention-days"),
                OsString::from("60"),
                OsString::from("--cursor-acp-executable"),
                OsString::from("/tmp/agent"),
                OsString::from("--cursor-acp-arguments"),
                OsString::from("acp"),
            ])
        );
    }

    #[test]
    fn replacement_command_preserves_owner_override() {
        let owner = message_board::HumanId::try_from("chosen-owner".to_owned())
            .expect("valid owner identity");
        let executable = PathBuf::from("/tmp/codex-router");
        let router_root = PathBuf::from("/tmp/router-root");
        let command = host_replacement_command(HostReplacementCommandInputs {
            executable: executable.clone(),
            router_root: router_root.clone(),
            port: 19087,
            mcp_bind: SocketAddr::from(([127, 0, 0, 1], 19088)),
            provider_operation_retention_days: std::num::NonZeroU32::new(60).expect("positive"),
            owner_human_id: Some(&owner),
            external_provider_launches: &[],
            launch_mode: HostLaunchMode::OwnerProduction,
        });
        assert_eq!(
            command,
            ChildCommandSpec::new(executable).with_arguments([
                OsString::from("host"),
                OsString::from("--router-root"),
                router_root.into_os_string(),
                OsString::from("--port"),
                OsString::from("19087"),
                OsString::from("--mcp-bind"),
                OsString::from("127.0.0.1:19088"),
                OsString::from("--provider-operation-retention-days"),
                OsString::from("60"),
                OsString::from("--owner-human-id"),
                OsString::from("chosen-owner"),
            ]),
        );
    }

    #[test]
    fn replacement_command_carries_resolved_isolation() {
        let command = host_replacement_command(HostReplacementCommandInputs {
            executable: PathBuf::from("/tmp/codex-router"),
            router_root: PathBuf::from("/tmp/private-router"),
            port: 18787,
            mcp_bind: SocketAddr::from(([127, 0, 0, 1], 18788)),
            provider_operation_retention_days: std::num::NonZeroU32::new(60).expect("positive"),
            owner_human_id: None,
            external_provider_launches: &[],
            launch_mode: HostLaunchMode::IsolatedDebug,
        });
        assert_eq!(
            command,
            ChildCommandSpec::new(PathBuf::from("/tmp/codex-router")).with_arguments([
                OsString::from("host"),
                OsString::from("--router-root"),
                OsString::from("/tmp/private-router"),
                OsString::from("--port"),
                OsString::from("18787"),
                OsString::from("--mcp-bind"),
                OsString::from("127.0.0.1:18788"),
                OsString::from("--provider-operation-retention-days"),
                OsString::from("60"),
                OsString::from("--require-debug-isolation"),
            ])
        );
    }
}
