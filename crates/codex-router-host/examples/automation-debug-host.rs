//! Opt-in acceptance Host: normal Codex home, existing debug provider, fresh runtime and Luna only.
#[path = "automation_debug_host/host_replacement.rs"]
mod host_replacement;
#[path = "automation_debug_host/launch_options.rs"]
mod launch_options;
#[path = "automation_debug_host/proof_permissions.rs"]
mod proof_permissions;
#[path = "automation_debug_host/startup_diagnostics.rs"]
mod startup_diagnostics;

use codex_native_integration::{
    AppServerCommandSpec, CodexPaths, CodexRouterProfile, DEFAULT_ROUTER_PORT, DebugCodexProfile,
};
use codex_router_host::{
    AppServerLaunchPlan, ChildCommandSpec, ChildOutput, HostConfig, HostConfigInputs,
    HostDeadlines, HostRuntime, ManagedChildLaunchPlans, ManagedUpdateInputs,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[cfg(test)]
use launch_options::parse_options_from;
use launch_options::{ProviderCommand, RunDirectoryAdmission, parse_options};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DebugHostContext {
    kind: String,
    run_directory: PathBuf,
    service_directory: PathBuf,
    workspace: PathBuf,
    profile: String,
    model: String,
    port: u16,
    host_pid: u32,
    router_root: Option<PathBuf>,
    claude_provider: Option<ProviderCommand>,
    cursor_provider: Option<ProviderCommand>,
    peer_registry_directory: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    startup_diagnostics::enable_startup_diagnostics()?;
    let Some(options) = parse_options()? else {
        return Ok(());
    };
    if !cfg!(debug_assertions) {
        return Err("This acceptance Host must be a debug build.".into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME unavailable")?);
    let codex_home = home.join(".codex").canonicalize()?;
    if let Some(configured) = std::env::var_os("CODEX_HOME")
        && PathBuf::from(configured).canonicalize()? != codex_home
    {
        return Err(
            "Acceptance requires normal Codex home; alternate CODEX_HOME is not allowed.".into(),
        );
    }
    let debug_root = options
        .router_root
        .clone()
        .unwrap_or_else(|| home.join(".codex-router-debug"));
    codex_native_integration::validate_debug_directory(&debug_root, &home.join(".codex-router"))?;
    match options.run_directory_admission {
        RunDirectoryAdmission::Fresh => validate_fresh_run_directory(&options.run_directory)?,
        RunDirectoryAdmission::Resume => {
            validate_resume_run_directory(&options.run_directory, options.port)?;
            let previous = read_existing_context(&options.run_directory)?;
            validate_resume_binding(&previous, &options)?;
        }
    }
    if options.port == DEFAULT_ROUTER_PORT || options.port == 0 {
        return Err(format!(
            "Select an unused debug port, never production {DEFAULT_ROUTER_PORT}."
        )
        .into());
    }
    if !options.router_binary.is_absolute() || !options.router_binary.is_file() {
        return Err(
            "--router-binary requires the absolute path of the locally compiled debug router."
                .into(),
        );
    }
    let endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, options.port));
    // Reject already-occupied ports before preparing a test launch.
    let probe = std::net::TcpListener::bind(endpoint)
        .map_err(|_| "The debug port is already occupied; no existing process was touched.")?;
    let profile = luna_profile(&codex_home, options.port)?;
    if !debug_root.join("state.sqlite").is_file() || !debug_root.join("secrets").is_dir() {
        return Err("Existing debug router state and credentials are required; this harness never copies production credentials.".into());
    }
    let native_directory = options.run_directory.join("native-socket");
    let workspace = options.run_directory.join("agent-workspace");
    if options.run_directory_admission == RunDirectoryAdmission::Fresh {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&options.run_directory)?;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&native_directory)?;
        std::fs::DirBuilder::new().mode(0o700).create(&workspace)?;
    }
    prepare_peer_registry(&options)?;
    let socket = native_directory.join("app-server.sock");
    let paths = CodexPaths::from_codex_home(codex_home.clone());
    let spec = AppServerCommandSpec::new(&paths, &CodexRouterProfile::new(options.port), &socket)
        .with_debug_profile(&profile);
    let executable = codex_native_integration::executable_identity(&spec.executable()).await?;
    let version = codex_native_integration::managed_executable_version(&spec.executable()).await?;
    // Home hooks can inject extra work after a test task ends. Disable them only
    // for this owned acceptance app-server, without editing the home profile.
    let mut native_arguments = vec![OsString::from("-c"), OsString::from("features.hooks=false")];
    native_arguments.extend(proof_permissions::automation_proof_startup_arguments(
        &options.run_directory,
    )?);
    native_arguments.extend(spec.arguments());
    let mut native = ChildCommandSpec::new(spec.executable())
        .with_arguments(native_arguments)
        .with_output(ChildOutput::Telemetry);
    for (key, value) in spec.environment() {
        native = native.with_environment(key, value);
    }
    let collaboration = options.run_directory.join("agent-communication");
    let mut launch = AppServerLaunchPlan::new(native, executable, version)
        .with_schema_directory(collaboration.clone());
    launch.prepare_schema().await;
    let replacement_command = host_replacement::build_host_replacement_command(&options)?;
    let router = options.router_child_command(&debug_root);
    let config = HostConfig::new(HostConfigInputs {
        coordination_paths: options.host_coordination_paths(&debug_root),
        router_endpoint: endpoint,
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        app_server_socket: socket,
        managed_executable: paths.managed_executable(),
        deadlines: HostDeadlines::production(),
    })
    .with_collaboration_directory(collaboration.clone(), codex_home);
    let config = match &options.peer_registry_directory {
        Some(directory) => config.with_collaboration_peer_registry_directory(directory.clone()),
        None => config,
    };
    let config = options.attach_provider(
        config,
        std::env::var_os("CLAUDE_CODE_EXECUTABLE").map(PathBuf::from),
    )?;
    let prepared = DebugHostContext {
        kind: "debugHostPrepared".to_owned(),
        run_directory: options.run_directory.clone(),
        service_directory: collaboration,
        workspace,
        profile: "codex-router-debug".to_owned(),
        model: "gpt-5.6-luna".to_owned(),
        port: options.port,
        host_pid: std::process::id(),
        router_root: options.router_root.clone(),
        claude_provider: options.claude_provider.clone(),
        cursor_provider: options.cursor_provider.clone(),
        peer_registry_directory: options.peer_registry_directory.clone(),
    };
    let instance =
        host_replacement::acquire_acceptance_host_instance(config.coordination_paths().clone())?;
    match options.run_directory_admission {
        RunDirectoryAdmission::Fresh => write_fresh_context(&prepared)?,
        RunDirectoryAdmission::Resume => replace_resumed_context(&prepared)?,
    }
    println!("{}", serde_json::to_string(&prepared)?);
    drop(probe);
    HostRuntime::run_acquired(
        config,
        ManagedChildLaunchPlans::new(Some(router), launch),
        ManagedUpdateInputs::production().with_replacement_command(replacement_command),
        instance,
    )
    .await?;
    Ok(())
}
fn luna_profile(
    codex_home: &Path,
    port: u16,
) -> Result<DebugCodexProfile, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(codex_home.join("codex-router-debug.config.toml"))?;
    let mut text = String::new();
    file.take(65537).read_to_string(&mut text)?;
    luna_profile_from_text(&text, port)
}

fn luna_profile_from_text(
    text: &str,
    selected_port: u16,
) -> Result<DebugCodexProfile, Box<dyn std::error::Error>> {
    let mut profile: toml::Table = toml::from_str(text)?;
    let provider = profile
        .get_mut("model_providers")
        .and_then(toml::Value::as_table_mut)
        .and_then(|providers| providers.get_mut("codex-router-debug"))
        .and_then(toml::Value::as_table_mut)
        .ok_or("debug profile is missing its sole debug provider")?;
    let configured_endpoint = provider
        .get("base_url")
        .and_then(toml::Value::as_str)
        .ok_or("debug profile is missing its debug provider endpoint")?;
    let configured_port = configured_endpoint
        .strip_prefix("http://127.0.0.1:")
        .and_then(|value| value.strip_suffix("/v1"))
        .ok_or("debug profile endpoint is not the expected loopback route")?
        .parse::<u16>()?;
    // Validate the complete saved profile before changing only its in-memory endpoint.
    DebugCodexProfile::parse(text, configured_port)?;
    provider.insert(
        "base_url".into(),
        toml::Value::String(format!("http://127.0.0.1:{selected_port}/v1")),
    );
    profile.insert("model".into(), toml::Value::String("gpt-5.6-luna".into()));
    profile.insert(
        "model_reasoning_effort".into(),
        toml::Value::String("high".into()),
    );
    Ok(DebugCodexProfile::parse(
        &toml::to_string(&profile)?,
        selected_port,
    )?)
}
fn validate_fresh_run_directory(run_directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let parent = run_directory
        .parent()
        .ok_or("run directory parent missing")?
        .canonicalize()?;
    if !run_directory.is_absolute()
        || parent != Path::new("/tmp").canonicalize()?
        || run_directory.exists()
    {
        return Err("--run-directory must be a new direct child of /tmp; existing folders are never reused.".into());
    }
    Ok(())
}

fn validate_resume_run_directory(
    run_directory: &Path,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_private_owned_directory(run_directory, "resume run directory")?;
    let canonical_root = run_directory.canonicalize()?;
    if canonical_root.parent() != Some(Path::new("/tmp").canonicalize()?.as_path()) {
        return Err("--resume-run-directory must be a direct child of /tmp".into());
    }
    let service_directory = run_directory.join("agent-communication");
    let workspace = run_directory.join("agent-workspace");
    let native_directory = run_directory.join("native-socket");
    validate_private_owned_directory(&service_directory, "service directory")?;
    validate_private_owned_directory(&workspace, "workspace directory")?;
    validate_private_owned_directory(&native_directory, "native socket directory")?;
    match std::fs::symlink_metadata(service_directory.join("service.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err("resume refused while the prior Host service is still published".into());
        }
        Err(error) => return Err(error.into()),
    }

    let context = read_existing_context(run_directory)?;
    if context.kind != "debugHostPrepared"
        || context.profile != "codex-router-debug"
        || context.model != "gpt-5.6-luna"
        || context.port != port
        || context.run_directory.canonicalize()? != canonical_root
        || context.service_directory.canonicalize()? != service_directory.canonicalize()?
        || context.workspace.canonicalize()? != workspace.canonicalize()?
    {
        return Err("resume run directory marker does not match the requested debug Host".into());
    }
    if !host_replacement::is_inherited_host_replacement(context.host_pid) {
        confirm_process_absent(context.host_pid)?;
    }
    Ok(())
}

fn validate_private_owned_directory(
    directory: &Path,
    description: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = std::fs::symlink_metadata(directory)?;
    if !directory.is_absolute()
        || !metadata.file_type().is_dir()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(format!("{description} must be an owner-private absolute directory").into());
    }
    Ok(())
}

fn read_existing_context(
    run_directory: &Path,
) -> Result<DebugHostContext, Box<dyn std::error::Error>> {
    let marker_path = run_directory.join("debug-host-context.json");
    let metadata = std::fs::symlink_metadata(&marker_path)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > 4096
    {
        return Err("debug Host context marker must be a small owner-private regular file".into());
    }
    let descriptor = rustix::fs::open(
        &marker_path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )?;
    let marker = File::from(descriptor);
    let opened_metadata = marker.metadata()?;
    if opened_metadata.dev() != metadata.dev()
        || opened_metadata.ino() != metadata.ino()
        || opened_metadata.uid() != rustix::process::getuid().as_raw()
        || opened_metadata.permissions().mode() & 0o077 != 0
    {
        return Err("debug Host context marker changed during validation".into());
    }
    let mut bytes = Vec::new();
    marker.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err("debug Host context marker is too large".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn confirm_process_absent(process_id: u32) -> Result<(), Box<dyn std::error::Error>> {
    let process_id = rustix::process::Pid::from_raw(i32::try_from(process_id)?)
        .ok_or("debug Host context marker has an invalid process ID")?;
    match rustix::process::test_kill_process(process_id) {
        Err(rustix::io::Errno::SRCH) => Ok(()),
        Ok(()) => Err("resume refused because the prior debug Host process is still alive".into()),
        Err(_) => Err("resume could not confirm that the prior debug Host process exited".into()),
    }
}

fn write_fresh_context(context: &DebugHostContext) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = serialize_context(context)?;
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(context.run_directory.join("debug-host-context.json"))?;
    marker.write_all(&bytes)?;
    marker.sync_all()?;
    Ok(())
}

fn replace_resumed_context(context: &DebugHostContext) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = serialize_context(context)?;
    let marker_path = context.run_directory.join("debug-host-context.json");
    let temporary_path = context
        .run_directory
        .join(format!(".debug-host-context-{}.tmp", context.host_pid));
    let descriptor = rustix::fs::open(
        &temporary_path,
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?;
    let result = (|| {
        let mut marker = File::from(descriptor);
        marker.write_all(&bytes)?;
        marker.sync_all()?;
        std::fs::rename(&temporary_path, &marker_path)?;
        let run_directory = rustix::fs::open(
            &context.run_directory,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )?;
        rustix::fs::fsync(&run_directory)?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    if result.is_err() {
        let _cleanup = std::fs::remove_file(temporary_path);
    }
    result
}

fn validate_resume_binding(
    context: &DebugHostContext,
    options: &launch_options::DebugHostOptions,
) -> Result<(), &'static str> {
    if context.router_root != options.router_root
        || context.claude_provider != options.claude_provider
        || context.cursor_provider != options.cursor_provider
        || context.peer_registry_directory != options.peer_registry_directory
    {
        return Err("resume refused: Router root or ACP command changed");
    }
    Ok(())
}

fn serialize_context(context: &DebugHostContext) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec(context)?;
    if bytes.len() > 4096 {
        return Err("debug Host context marker is too large".into());
    }
    Ok(bytes)
}

fn prepare_peer_registry(
    options: &launch_options::DebugHostOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(directory) = &options.peer_registry_directory {
        if directory != &options.run_directory.join("peer-registry") {
            return Err("peer registry does not match the admitted run directory".into());
        }
        if options.run_directory_admission == RunDirectoryAdmission::Fresh {
            std::fs::DirBuilder::new().mode(0o700).create(directory)?;
        }
        validate_private_owned_directory(directory, "peer registry directory")?;
        if directory.canonicalize()?.parent()
            != Some(options.run_directory.canonicalize()?.as_path())
        {
            return Err("peer registry must remain inside the admitted run directory".into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "automation_debug_host/launch_tests.rs"]
mod tests;
