//! Opt-in acceptance Host: normal Codex home, existing debug provider, fresh runtime and Luna only.
use codex_native_integration::{
    AppServerCommandSpec, CodexPaths, CodexRouterProfile, DebugCodexProfile,
};
use codex_router_host::{
    AppServerLaunchPlan, ChildCommandSpec, ChildOutput, HostConfig, HostConfigInputs,
    HostCoordinationPaths, HostDeadlines, HostInstance, HostRuntime, ManagedChildLaunchPlans,
    ManagedUpdateInputs,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunDirectoryAdmission {
    Fresh,
    Resume,
}

#[derive(Debug)]
struct DebugHostOptions {
    run_directory: PathBuf,
    run_directory_admission: RunDirectoryAdmission,
    router_binary: PathBuf,
    port: u16,
}

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
    match options.run_directory_admission {
        RunDirectoryAdmission::Fresh => validate_fresh_run_directory(&options.run_directory)?,
        RunDirectoryAdmission::Resume => {
            validate_resume_run_directory(&options.run_directory, options.port)?;
        }
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
    let prepared = DebugHostContext {
        kind: "debugHostPrepared".to_owned(),
        run_directory: options.run_directory.clone(),
        service_directory: communication,
        workspace,
        profile: "codex-router-debug".to_owned(),
        model: "gpt-5.6-luna".to_owned(),
        port: options.port,
        host_pid: std::process::id(),
    };
    let instance = HostInstance::acquire(config.coordination_paths().clone())?;
    match options.run_directory_admission {
        RunDirectoryAdmission::Fresh => write_fresh_context(&prepared)?,
        RunDirectoryAdmission::Resume => replace_resumed_context(&prepared)?,
    }
    println!("{}", serde_json::to_string(&prepared)?);
    drop(probe);
    HostRuntime::run_acquired(
        config,
        ManagedChildLaunchPlans::new(Some(router), launch),
        ManagedUpdateInputs::production(),
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
    parse_options_from(std::env::args_os().skip(1))
}

fn parse_options_from(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<DebugHostOptions>, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let mut run_directory = None::<(PathBuf, RunDirectoryAdmission)>;
    let mut router_binary = None;
    let mut port = 18787;
    while let Some(flag) = arguments.next() {
        if flag == "--help" || flag == "-h" {
            println!(
                "automation-debug-host (--run-directory /tmp/NEW-DIRECTORY | --resume-run-directory /tmp/OWNED-DIRECTORY) --router-binary /ABSOLUTE/target/debug/codex-router [--port 18787]\n\nOpt-in real acceptance Host. Uses the existing codex-router-debug profile with an in-memory Luna override. Fresh mode creates runtime/workspace directories. Resume mode reopens only a stopped Host's validated private directory. Starts only its own debug router and app-server and shuts its children down on SIGTERM. Never use production paths or ports. Keep this process running while the acceptance runner connects."
            );
            return Ok(None);
        }
        let value = arguments.next().ok_or("option value missing; use --help")?;
        match flag.to_str() {
            Some("--run-directory") => select_run_directory(
                &mut run_directory,
                PathBuf::from(value),
                RunDirectoryAdmission::Fresh,
            )?,
            Some("--resume-run-directory") => select_run_directory(
                &mut run_directory,
                PathBuf::from(value),
                RunDirectoryAdmission::Resume,
            )?,
            Some("--router-binary") => router_binary = Some(PathBuf::from(value)),
            Some("--port") => port = value.to_str().ok_or("port is not UTF-8")?.parse()?,
            _ => return Err("Unknown option; use --help".into()),
        }
    }
    let (run_directory, run_directory_admission) = run_directory.ok_or(
        "exactly one of --run-directory or --resume-run-directory is required; use --help",
    )?;
    Ok(Some(DebugHostOptions {
        run_directory,
        run_directory_admission,
        router_binary: router_binary.ok_or("--router-binary required; use --help")?,
        port,
    }))
}

fn select_run_directory(
    selected: &mut Option<(PathBuf, RunDirectoryAdmission)>,
    run_directory: PathBuf,
    admission: RunDirectoryAdmission,
) -> Result<(), &'static str> {
    if selected.is_some() {
        return Err("--run-directory and --resume-run-directory are mutually exclusive");
    }
    *selected = Some((run_directory, admission));
    Ok(())
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
    confirm_process_absent(context.host_pid)?;
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
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(context.run_directory.join("debug-host-context.json"))?;
    marker.write_all(&serde_json::to_vec(context)?)?;
    marker.sync_all()?;
    Ok(())
}

fn replace_resumed_context(context: &DebugHostContext) -> Result<(), Box<dyn std::error::Error>> {
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
        marker.write_all(&serde_json::to_vec(context)?)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestRunDirectory(PathBuf);

    impl TestRunDirectory {
        fn create() -> Result<Self, Box<dyn std::error::Error>> {
            let root = PathBuf::from("/tmp").join(format!(
                "automation-debug-host-resume-{}-{}",
                std::process::id(),
                NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::DirBuilder::new().mode(0o700).create(&root)?;
            for child in ["agent-communication", "agent-workspace", "native-socket"] {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(root.join(child))?;
            }
            Ok(Self(root))
        }

        fn write_context(&self, process_id: u32) -> Result<(), Box<dyn std::error::Error>> {
            write_fresh_context(&DebugHostContext {
                kind: "debugHostPrepared".to_owned(),
                run_directory: self.0.clone(),
                service_directory: self.0.join("agent-communication"),
                workspace: self.0.join("agent-workspace"),
                profile: "codex-router-debug".to_owned(),
                model: "gpt-5.6-luna".to_owned(),
                port: 18787,
                host_pid: process_id,
            })
        }
    }

    impl Drop for TestRunDirectory {
        fn drop(&mut self) {
            let _cleanup = std::fs::remove_dir_all(&self.0);
        }
    }

    fn stopped_process_id() -> Result<u32, Box<dyn std::error::Error>> {
        let mut child = std::process::Command::new("/usr/bin/true").spawn()?;
        let process_id = child.id();
        let _status = child.wait()?;
        Ok(process_id)
    }

    #[test]
    fn run_directory_modes_are_mutually_exclusive() {
        let result = parse_options_from([
            OsString::from("--run-directory"),
            OsString::from("/tmp/fresh"),
            OsString::from("--resume-run-directory"),
            OsString::from("/tmp/resume"),
        ]);
        assert!(
            result
                .expect_err("conflicting run-directory modes must fail")
                .to_string()
                .contains("mutually exclusive")
        );
    }

    #[test]
    fn resume_rejects_marker_for_another_root() -> Result<(), Box<dyn std::error::Error>> {
        let run = TestRunDirectory::create()?;
        let foreign_run = TestRunDirectory::create()?;
        run.write_context(stopped_process_id()?)?;
        let marker_path = run.0.join("debug-host-context.json");
        let mut marker: DebugHostContext = serde_json::from_slice(&std::fs::read(&marker_path)?)?;
        marker.run_directory = foreign_run.0.clone();
        std::fs::write(&marker_path, serde_json::to_vec(&marker)?)?;

        let error =
            validate_resume_run_directory(&run.0, 18787).expect_err("foreign marker must fail");
        assert!(error.to_string().contains("does not match"));
        Ok(())
    }

    #[test]
    fn resume_rejects_invalid_context_marker() -> Result<(), Box<dyn std::error::Error>> {
        let run = TestRunDirectory::create()?;
        run.write_context(stopped_process_id()?)?;
        let marker_path = run.0.join("debug-host-context.json");
        let mut marker: DebugHostContext = serde_json::from_slice(&std::fs::read(&marker_path)?)?;
        marker.model = "not-luna".to_owned();
        std::fs::write(&marker_path, serde_json::to_vec(&marker)?)?;

        let error =
            validate_resume_run_directory(&run.0, 18787).expect_err("invalid marker must fail");
        assert!(error.to_string().contains("does not match"));
        Ok(())
    }

    #[test]
    fn resume_rejects_live_prior_host_process() -> Result<(), Box<dyn std::error::Error>> {
        let run = TestRunDirectory::create()?;
        run.write_context(std::process::id())?;

        let error =
            validate_resume_run_directory(&run.0, 18787).expect_err("live marker PID must fail");
        assert!(error.to_string().contains("still alive"));
        Ok(())
    }

    #[test]
    fn resume_admits_stopped_owned_private_root() -> Result<(), Box<dyn std::error::Error>> {
        let run = TestRunDirectory::create()?;
        run.write_context(stopped_process_id()?)?;

        validate_resume_run_directory(&run.0, 18787)?;
        Ok(())
    }

    #[test]
    fn resume_replaces_marker_without_changing_service_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let run = TestRunDirectory::create()?;
        run.write_context(stopped_process_id()?)?;
        let database_path = run.0.join("agent-communication/project-board.sqlite");
        std::fs::write(&database_path, b"preserved-board-state")?;
        let resumed_context = DebugHostContext {
            kind: "debugHostPrepared".to_owned(),
            run_directory: run.0.clone(),
            service_directory: run.0.join("agent-communication"),
            workspace: run.0.join("agent-workspace"),
            profile: "codex-router-debug".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
            port: 18787,
            host_pid: 4242,
        };

        replace_resumed_context(&resumed_context)?;

        let published = read_existing_context(&run.0)?;
        assert_eq!(published.host_pid, 4242);
        assert_eq!(std::fs::read(database_path)?, b"preserved-board-state");
        Ok(())
    }

    #[test]
    fn resume_rejects_symlinked_context_marker() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let run = TestRunDirectory::create()?;
        let target = run.0.join("context-target.json");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&target)?;
        symlink(&target, run.0.join("debug-host-context.json"))?;

        let error =
            validate_resume_run_directory(&run.0, 18787).expect_err("symlink marker must fail");
        assert!(error.to_string().contains("regular file"));
        Ok(())
    }
}
