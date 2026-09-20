//! Opt-in real image generation/edit proof through owned debug Router and app-server children.
#[path = "imagegen_debug_acceptance/audit_proof.rs"]
mod audit_proof;
#[path = "imagegen_debug_acceptance/native_proof.rs"]
mod native_proof;
#[path = "debug_host_acceptance/process_identity_guard.rs"]
mod process_identity_guard;
#[path = "imagegen_debug_acceptance/profile.rs"]
mod profile;

use codex_native_integration::{
    AppServerCommandSpec, CodexPaths, CodexRouterProfile, NativeProtocolConnection,
};
use codex_router_host::{APP_SERVER_SHUTDOWN_TIMEOUT, ProcessGroupChild, ROUTER_SHUTDOWN_TIMEOUT};
use process_identity_guard::capture_production_identity;
use serde::Serialize;
use std::{
    ffi::OsString,
    fs::OpenOptions,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::process::Command;

const DEBUG_PORT: u16 = 18787;

struct Options {
    router_binary: PathBuf,
    artifact_directory: PathBuf,
    port: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceptanceSummary {
    kind: &'static str,
    managed_codex_version: &'static str,
    router_port: u16,
    normal_codex_home: bool,
    remote_control: &'static str,
    hooks: &'static str,
    image_capability: bool,
    model_image_input: bool,
    generated_artifact: PathBuf,
    edited_artifact: PathBuf,
    generation_bytes: u64,
    edit_bytes: u64,
    audit: audit_proof::AuditProof,
    owned_children_exited: bool,
    debug_listeners_closed: bool,
    production_identity_unchanged: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IsolationReceipt<'a> {
    kind: &'static str,
    proof_passed: bool,
    owned_children_exited: bool,
    debug_listeners_closed: bool,
    production_identity_unchanged: bool,
    diagnostics_directory: &'a Path,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(options) = parse_options()? else {
        return Ok(());
    };
    run(options).await
}

async fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    if !cfg!(debug_assertions) || options.port == 0 || options.port == 8787 {
        return Err("A debug build and an unused non-production port are required.".into());
    }
    if !options.router_binary.is_absolute() || !options.router_binary.is_file() {
        return Err("--router-binary must identify an absolute built debug Router binary.".into());
    }
    let endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, options.port));
    let port_guard = std::net::TcpListener::bind(endpoint)
        .map_err(|_| "The requested debug port is occupied; no listener was touched.")?;
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME unavailable")?);
    let codex_home = home.join(".codex").canonicalize()?;
    if let Some(configured) = std::env::var_os("CODEX_HOME")
        && PathBuf::from(configured).canonicalize()? != codex_home
    {
        return Err("Normal Codex home is required; alternate CODEX_HOME is forbidden.".into());
    }
    let debug_root = home.join(".codex-router-debug");
    if !debug_root.join("state.sqlite").is_file() || !debug_root.join("secrets").is_dir() {
        return Err("Existing debug Router state and secrets are required.".into());
    }
    prepare_artifact_directory(&options.artifact_directory)?;
    let run_directory = create_run_directory()?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(run_directory.join("native-socket"))?;
    let native_socket = run_directory.join("native-socket/app-server.sock");
    let audit_file = run_directory.join("router-audit.jsonl");
    let production_before = capture_production_identity().await?;

    let mut router_command = Command::new(&options.router_binary);
    router_command
        .args(["serve", "--port", &options.port.to_string(), "--state-db"])
        .arg(debug_root.join("state.sqlite"))
        .arg("--secret-root")
        .arg(debug_root.join("secrets"))
        .arg("--audit-file")
        .arg(&audit_file)
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .stdout(Stdio::null())
        .stderr(Stdio::from(private_log(&run_directory.join("router.log"))?));
    drop(port_guard);
    let mut router = ProcessGroupChild::spawn(&mut router_command)?;
    let mut app_server = None::<ProcessGroupChild>;

    let proof_result = async {
        wait_for_router(&mut router, endpoint).await?;
        app_server = Some(
            spawn_app_server(&codex_home, &run_directory, &native_socket, options.port).await?,
        );
        let mut native = wait_for_native(
            app_server
                .as_mut()
                .ok_or("Owned app-server was not retained")?,
            &native_socket,
        )
        .await?;
        let native_result = native_proof::run(
            &mut native,
            native_proof::NativeProofInputs {
                artifact_directory: &options.artifact_directory,
                codex_home: &codex_home,
                port: options.port,
            },
        )
        .await?;
        let audit = audit_proof::verify_and_write(&audit_file, &options.artifact_directory)?;
        Ok::<_, Box<dyn std::error::Error>>((native_result, audit))
    }
    .await;

    let app_cleanup = match app_server.as_mut() {
        Some(child) => stop_child(child, APP_SERVER_SHUTDOWN_TIMEOUT).await,
        None => Ok(()),
    };
    let cleanup_result = app_cleanup.and(stop_child(&mut router, ROUTER_SHUTDOWN_TIMEOUT).await);
    let owned_children_exited = cleanup_result.is_ok();
    let debug_listeners_closed = std::net::TcpListener::bind(endpoint).is_ok()
        && std::os::unix::net::UnixStream::connect(&native_socket).is_err();
    let production_identity_unchanged = production_before == capture_production_identity().await?;
    write_json(
        &options.artifact_directory.join("isolation-receipt.json"),
        &IsolationReceipt {
            kind: "imagegenDebugIsolationReceipt",
            proof_passed: proof_result.is_ok(),
            owned_children_exited,
            debug_listeners_closed,
            production_identity_unchanged,
            diagnostics_directory: &run_directory,
        },
    )?;
    cleanup_result?;
    if !debug_listeners_closed || !production_identity_unchanged {
        return Err("Debug cleanup or production identity isolation was not preserved.".into());
    }
    let (native_result, audit) = proof_result?;
    let summary = AcceptanceSummary {
        kind: "imagegenDebugAcceptancePassed",
        managed_codex_version: profile::REQUIRED_CODEX_VERSION,
        router_port: options.port,
        normal_codex_home: true,
        remote_control: "disabled",
        hooks: "disabled",
        image_capability: native_result.image_capability,
        model_image_input: native_result.model_image_input,
        generated_artifact: native_result.generated_artifact,
        edited_artifact: native_result.edited_artifact,
        generation_bytes: native_result.generation_bytes,
        edit_bytes: native_result.edit_bytes,
        audit,
        owned_children_exited,
        debug_listeners_closed,
        production_identity_unchanged,
    };
    write_json(
        &options.artifact_directory.join("acceptance-summary.json"),
        &summary,
    )?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

async fn spawn_app_server(
    codex_home: &Path,
    run_directory: &Path,
    socket: &Path,
    port: u16,
) -> Result<ProcessGroupChild, Box<dyn std::error::Error>> {
    let paths = CodexPaths::from_codex_home(codex_home.to_owned());
    let control_socket =
        codex_native_integration::RouterControlSocketPath::in_collaboration_directory(
            &run_directory.join("unused-control"),
        )?;
    let image_profile = profile::image_profile(codex_home, port)?;
    let spec = AppServerCommandSpec::new(
        &paths,
        &CodexRouterProfile::new(port),
        &control_socket,
        socket,
    )
    .with_debug_profile(&image_profile);
    let version = codex_native_integration::managed_executable_version(&spec.executable()).await?;
    if version != profile::REQUIRED_CODEX_VERSION {
        return Err(format!(
            "Managed Codex must be {}; found {version}.",
            profile::REQUIRED_CODEX_VERSION
        )
        .into());
    }
    let mut arguments = vec![OsString::from("-c"), OsString::from("features.hooks=false")];
    arguments.extend(spec.arguments());
    let mut command = Command::new(spec.executable());
    command
        .args(arguments)
        .envs(spec.environment())
        .env("CODEX_HOME", codex_home)
        .env("CODEX_STOP_REVIEW_RUNNER", "/usr/bin/true")
        .stdout(Stdio::null())
        .stderr(Stdio::from(private_log(
            &run_directory.join("app-server.log"),
        )?));
    ProcessGroupChild::spawn(&mut command).map_err(Into::into)
}

async fn wait_for_router(
    router: &mut ProcessGroupChild,
    endpoint: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(status) = router.try_wait()? {
                return Err(format!("Owned Router exited before readiness: {status}").into());
            }
            if tokio::net::TcpStream::connect(endpoint).await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| "Timed out waiting for the owned debug Router")?
}

async fn wait_for_native(
    app_server: &mut ProcessGroupChild,
    socket: &Path,
) -> Result<NativeProtocolConnection, Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if let Some(status) = app_server.try_wait()? {
                return Err(format!("Owned app-server exited before readiness: {status}").into());
            }
            if socket.exists()
                && let Ok(connection) = NativeProtocolConnection::connect(socket).await
            {
                return Ok(connection);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| "Timed out waiting for the owned debug app-server")?
}

async fn stop_child(
    child: &mut ProcessGroupChild,
    graceful_timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    child.send_terminate()?;
    match tokio::time::timeout(graceful_timeout + Duration::from_secs(5), child.wait()).await {
        Ok(status) => {
            let _status = status?;
        }
        Err(_) => {
            child.send_group_kill()?;
            let _status = tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .map_err(|_| "Owned child did not exit after SIGKILL")??;
        }
    }
    Ok(())
}

fn parse_options() -> Result<Option<Options>, Box<dyn std::error::Error>> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let (mut router_binary, mut artifact_directory, mut port) = (None, None, DEBUG_PORT);
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments
            .get(index)
            .ok_or("Option parser index exceeded the argument list")?;
        if flag == "--help" || flag == "-h" {
            println!(
                "imagegen-debug-acceptance --router-binary /ABSOLUTE/target/debug/codex-router --artifact-directory /ABSOLUTE/repo/tmp/imagegen-debug/NEW-DIRECTORY [--port 18787]"
            );
            return Ok(None);
        }
        let value = arguments.get(index + 1).ok_or("Option value missing")?;
        match flag.to_str() {
            Some("--router-binary") => router_binary = Some(PathBuf::from(value)),
            Some("--artifact-directory") => artifact_directory = Some(PathBuf::from(value)),
            Some("--port") => port = value.to_str().ok_or("Port is not UTF-8")?.parse()?,
            _ => return Err("Unknown option; use --help".into()),
        }
        index += 2;
    }
    Ok(Some(Options {
        router_binary: router_binary.ok_or("--router-binary is required")?,
        artifact_directory: artifact_directory.ok_or("--artifact-directory is required")?,
        port,
    }))
}

fn prepare_artifact_directory(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !path.is_absolute() || path.exists() {
        return Err("--artifact-directory must be a new absolute directory.".into());
    }
    let expected_parent = std::env::current_dir()?.join("tmp/imagegen-debug");
    std::fs::create_dir_all(&expected_parent)?;
    if path
        .parent()
        .ok_or("Artifact parent missing")?
        .canonicalize()?
        != expected_parent.canonicalize()?
    {
        return Err("Artifact directory must be a direct child of repo tmp/imagegen-debug.".into());
    }
    std::fs::DirBuilder::new().mode(0o700).create(path)?;
    Ok(())
}

fn create_run_directory() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path = PathBuf::from("/tmp").join(format!(
        "codex-router-imagegen-{}-{timestamp}",
        std::process::id()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&path)?;
    Ok(path)
}

fn private_log(path: &Path) -> std::io::Result<std::fs::File> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;
    let mut file = private_log(path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
