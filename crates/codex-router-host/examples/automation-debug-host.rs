//! Opt-in acceptance Host: normal Codex home, existing debug provider, fresh runtime and Luna only.
use codex_native_integration::{
    AppServerCommandSpec, CodexPaths, CodexRouterProfile, DebugCodexProfile,
};
use codex_router_host::{
    AppServerLaunchPlan, ChildCommandSpec, ChildOutput, HostConfig, HostConfigInputs,
    HostCoordinationPaths, HostDeadlines, HostRuntime, ManagedChildLaunchPlans,
    ManagedUpdateInputs,
};
use std::{
    ffi::OsString,
    io::Read,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

struct DebugHostOptions {
    run_directory: PathBuf,
    router_binary: PathBuf,
    port: u16,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    let parent = options
        .run_directory
        .parent()
        .ok_or("run directory parent missing")?
        .canonicalize()?;
    if !options.run_directory.is_absolute()
        || parent != Path::new("/tmp").canonicalize()?
        || options.run_directory.exists()
    {
        return Err("--run-directory must be a new direct child of /tmp; existing folders are never reused.".into());
    }
    if options.port == 8787 || options.port == 0 {
        return Err("Select an unused debug port, never production 8787.".into());
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
    let debug_root = home.join(".codex-router-debug");
    if !debug_root.join("state.sqlite").is_file() || !debug_root.join("secrets").is_dir() {
        return Err("Existing debug router state and credentials are required; this harness never copies production credentials.".into());
    }
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&options.run_directory)?;
    let native_directory = options.run_directory.join("native-socket");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&native_directory)?;
    let workspace = options.run_directory.join("agent-workspace");
    std::fs::DirBuilder::new().mode(0o700).create(&workspace)?;
    let socket = native_directory.join("app-server.sock");
    let paths = CodexPaths::from_codex_home(codex_home.clone());
    let spec = AppServerCommandSpec::new(&paths, &CodexRouterProfile::new(options.port), &socket)
        .with_debug_profile(&profile);
    let executable = codex_native_integration::executable_identity(&spec.executable()).await?;
    let version = codex_native_integration::managed_executable_version(&spec.executable()).await?;
    // Home hooks can inject extra work after a test task ends. Disable them only
    // for this owned acceptance app-server, without editing the home profile.
    let mut native_arguments = vec![OsString::from("-c"), OsString::from("features.hooks=false")];
    native_arguments.extend(spec.arguments());
    let mut native = ChildCommandSpec::new(spec.executable())
        .with_arguments(native_arguments)
        .with_output(ChildOutput::Telemetry);
    for (key, value) in spec.environment() {
        native = native.with_environment(key, value);
    }
    let communication = options.run_directory.join("agent-communication");
    let mut launch = AppServerLaunchPlan::new(native, executable, version)
        .with_schema_directory(communication.clone());
    launch.prepare_schema().await;
    let router = ChildCommandSpec::new(options.router_binary)
        .with_arguments([
            OsString::from("serve"),
            OsString::from("--port"),
            OsString::from(options.port.to_string()),
            OsString::from("--state-db"),
            debug_root.join("state.sqlite").into_os_string(),
            OsString::from("--secret-root"),
            debug_root.join("secrets").into_os_string(),
            OsString::from("--audit-file"),
            options
                .run_directory
                .join("router-audit.jsonl")
                .into_os_string(),
        ])
        .with_output(ChildOutput::Telemetry);
    let config = HostConfig::new(HostConfigInputs {
        coordination_paths: HostCoordinationPaths::new(
            options.run_directory.join("host.sock"),
            options.run_directory.join("host.lock"),
        ),
        router_endpoint: endpoint,
        app_server_socket: socket,
        managed_executable: paths.managed_executable(),
        deadlines: HostDeadlines::production(),
    })
    .with_communication_directory(communication.clone(), codex_home);
    let prepared = serde_json::json!({"kind":"debugHostPrepared","runDirectory":options.run_directory,"serviceDirectory":communication,"workspace":workspace,"profile":"codex-router-debug","model":"gpt-5.6-luna","port":options.port,"hostPid":std::process::id()});
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut marker = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(options.run_directory.join("debug-host-context.json"))?;
        marker.write_all(&serde_json::to_vec(&prepared)?)?;
        marker.sync_all()?;
    }
    println!("{prepared}");
    drop(probe);
    HostRuntime::run(
        config,
        ManagedChildLaunchPlans::new(Some(router), launch),
        ManagedUpdateInputs::production(),
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
    // Validate before editing in memory: this cannot redirect a production provider to the test.
    DebugCodexProfile::parse(&text, port)?;
    let mut profile: toml::Table = toml::from_str(&text)?;
    profile.insert("model".into(), toml::Value::String("gpt-5.6-luna".into()));
    profile.insert(
        "model_reasoning_effort".into(),
        toml::Value::String("high".into()),
    );
    Ok(DebugCodexProfile::parse(&toml::to_string(&profile)?, port)?)
}
fn parse_options() -> Result<Option<DebugHostOptions>, Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let mut run_directory = None;
    let mut router_binary = None;
    let mut port = 18787;
    while let Some(flag) = arguments.next() {
        if flag == "--help" || flag == "-h" {
            println!(
                "automation-debug-host --run-directory /tmp/NEW-DIRECTORY --router-binary /ABSOLUTE/target/debug/codex-router [--port 18787]\n\nOpt-in real acceptance Host. Uses the existing codex-router-debug profile with an in-memory Luna override. Creates fresh runtime/workspace directories, starts only its own debug router and app-server, and shuts its children down on SIGTERM. Never use production paths or ports. Keep this process running while the acceptance runner connects."
            );
            return Ok(None);
        }
        let value = arguments.next().ok_or("option value missing; use --help")?;
        match flag.to_str() {
            Some("--run-directory") => run_directory = Some(PathBuf::from(value)),
            Some("--router-binary") => router_binary = Some(PathBuf::from(value)),
            Some("--port") => port = value.to_str().ok_or("port is not UTF-8")?.parse()?,
            _ => return Err("Unknown option; use --help".into()),
        }
    }
    Ok(Some(DebugHostOptions {
        run_directory: run_directory.ok_or("--run-directory required; use --help")?,
        router_binary: router_binary.ok_or("--router-binary required; use --help")?,
        port,
    }))
}
