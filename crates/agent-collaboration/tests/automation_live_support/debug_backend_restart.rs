//! Restart only a source-verified debug Host's native child through the real CLI.
#[path = "debug_backend_restart_tests.rs"]
#[cfg(test)]
mod tests;

use super::proof_context::{ProofContext, ProofResult};
use codex_native_integration::NativeProtocolConnection;
use collaboration_client::protocol::ChannelDescription;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;

const PRIVATE_MARKER_MAX_BYTES: u64 = 4096;
const MATRIX_ROUTER_PORT: u16 = 43127;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostProcessKind {
    ForegroundCli,
    AutomationExample,
}

struct VerifiedRestartTarget {
    root: PathBuf,
    process_id: u32,
    port: u16,
    kind: HostProcessKind,
    process_command: String,
    effective_uid: u32,
    native_socket: Option<NativeSocketProvenance>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeSocketProvenance {
    alias: PathBuf,
    resolved_socket: PathBuf,
    daemon_directory: PathBuf,
    effective_uid: u32,
    socket_hash: String,
}

pub async fn restart(proof: &mut ProofContext) -> ProofResult<()> {
    let target = verify_restart_target(&proof.root).await?;
    let operator_cli = foreground_cli_binary()?.canonicalize()?;
    let previous_generation = proof.generation.clone();
    let control_service_id = proof.client.identity().service_id.clone();
    if let Some(provenance) = target.native_socket.as_ref() {
        proof.record(
            "nativeSocketProvenanceVerified",
            json!({
                "alias":provenance.alias,
                "resolvedSocket":provenance.resolved_socket,
                "daemonDirectory":provenance.daemon_directory,
                "effectiveUid":provenance.effective_uid,
                "sha256":provenance.socket_hash,
            }),
        )?;
    }
    proof.record(
        "hostOperatorAndControlSocketsVerified",
        json!({
            "operatorSocket":target.root.join("host.sock"),
            "controlSocket":target.root.join("agent-communication/control.sock"),
            "effectiveUid":target.effective_uid,
        }),
    )?;
    proof.record(
        "ownedBackendRestartRequested",
        json!({
            "operation":"host app-server restart",
            "hostPid":target.process_id,
            "routerRoot":target.root,
            "port":target.port,
            "generation":previous_generation,
        }),
    )?;

    let mut command = Command::new(operator_cli);
    command
        .args(["host", "app-server", "restart", "--router-root"])
        .arg(&target.root)
        .args([
            "--port",
            &target.port.to_string(),
            "--require-debug-isolation",
        ])
        .kill_on_drop(true);
    if target.kind == HostProcessKind::ForegroundCli {
        command
            .env_clear()
            .env("PATH", "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", target.root.join("home"))
            .env("CODEX_HOME", target.root.join("codex-home"))
            .env(
                "CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET",
                target.root.join("native-socket/app-server.sock"),
            );
        if let Some(user) = std::env::var_os("USER") {
            command.env("USER", user);
        }
    }
    let output = tokio::time::timeout(Duration::from_secs(130), command.output()).await??;
    let succeeded = output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains("result: succeeded");
    proof.record(
        "ownedBackendRestartResult",
        json!({
            "operation":"host app-server restart",
            "exitCode":output.status.code(),
            "succeeded":succeeded,
        }),
    )?;
    if !succeeded {
        return Err("Owned native app-server restart did not report success".into());
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    loop {
        interval.tick().await;
        let inventory = proof.client.list_endpoints().await?;
        let replacement = inventory
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.endpoint == proof.endpoint)
            .flat_map(|endpoint| &endpoint.channels)
            .find_map(|channel| match channel {
                ChannelDescription::NativeCodex {
                    generation: Some(generation),
                    schema_digest: Some(digest),
                    ..
                } if generation != &previous_generation => {
                    Some((generation.clone(), digest.clone()))
                }
                _ => None,
            });
        if let Some((generation, digest)) = replacement {
            if String::from(digest) != proof.schemas.schema_digest() {
                return Err("Native schema changed during app-server restart".into());
            }
            let command_after = inspect_process_command(target.process_id).await?;
            if command_after != target.process_command {
                return Err(
                    "Foreground Host process identity changed during native restart".into(),
                );
            }
            let native_socket_after = if target.kind == HostProcessKind::ForegroundCli {
                Some(validate_native_socket(&target.root, target.effective_uid)?)
            } else {
                None
            };
            if native_socket_after != target.native_socket {
                return Err(
                    "Codex native socket provenance changed during app-server restart".into(),
                );
            }
            validate_operator_and_control_sockets(&target.root, target.effective_uid)?;
            if proof.client.identity().service_id != control_service_id {
                return Err("Host control service identity changed during native restart".into());
            }
            proof.generation = generation;
            proof.native = NativeProtocolConnection::connect(
                &target.root.join("native-socket/app-server.sock"),
            )
            .await?;
            proof.record(
                "ownedBackendReplacementReady",
                json!({
                    "before":previous_generation,
                    "after":proof.generation,
                    "hostPid":target.process_id,
                    "controlServiceId":control_service_id,
                    "hostProcessIdentityPreserved":true,
                    "nativeSocketProvenancePreserved":native_socket_after.as_ref().map(|provenance| json!({
                        "alias":provenance.alias,
                        "resolvedSocket":provenance.resolved_socket,
                        "daemonDirectory":provenance.daemon_directory,
                        "effectiveUid":provenance.effective_uid,
                        "sha256":provenance.socket_hash,
                    })),
                }),
            )?;
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Native app-server replacement did not publish a new generation".into());
        }
    }
}

async fn verify_restart_target(root: &Path) -> ProofResult<VerifiedRestartTarget> {
    let effective_uid = effective_uid()?;
    let canonical_root = validate_private_root(root, effective_uid)?;
    let marker_path = canonical_root.join("debug-host-context.json");
    let marker = read_private_marker(&marker_path, effective_uid)?;
    let run_directory = required_string(&marker, "runDirectory")?;
    if Path::new(run_directory).canonicalize()? != canonical_root {
        return Err("Debug Host marker names a different root".into());
    }
    let process_id = marker
        .get("hostPid")
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .ok_or("Owned foreground Host PID missing")?;
    let port = marker
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0 && *port != 8787)
        .ok_or("Isolated debug provider port missing")?;
    let (kind, native_socket) = match marker.get("kind").and_then(Value::as_str) {
        Some("isolatedDeliveryMatrix") => {
            if port != MATRIX_ROUTER_PORT {
                return Err("Foreground matrix Host uses an unexpected Router port".into());
            }
            let executable = required_string(&marker, "cliExecutable")?;
            let executable = Path::new(executable).canonicalize()?;
            let expected_executable = foreground_cli_binary()?.canonicalize()?;
            if executable != expected_executable || !executable.is_file() {
                return Err(
                    "Foreground matrix Host executable does not match the compiled CLI".into(),
                );
            }
            let command = inspect_process_command(process_id).await?;
            if !matrix_cli_argv_matches(&command, &executable, &canonical_root, port) {
                return Err("PID does not identify this isolated foreground CLI Host".into());
            }
            let native_socket = validate_native_socket(&canonical_root, effective_uid)?;
            validate_operator_and_control_sockets(&canonical_root, effective_uid)?;
            (HostProcessKind::ForegroundCli, Some(native_socket))
        }
        Some("debugHostPrepared") => {
            let command = inspect_process_command(process_id).await?;
            if !automation_debug_host_argv_matches(&command, run_directory, &canonical_root) {
                return Err(
                    "PID does not identify the existing automation-debug-host caller".into(),
                );
            }
            validate_operator_and_control_sockets(&canonical_root, effective_uid)?;
            (HostProcessKind::AutomationExample, None)
        }
        _ => return Err("Debug Host marker kind is not an authorized restart fixture".into()),
    };
    let process_command = inspect_process_command(process_id).await?;
    Ok(VerifiedRestartTarget {
        root: canonical_root,
        process_id,
        port,
        kind,
        process_command,
        effective_uid,
        native_socket,
    })
}

fn validate_private_root(root: &Path, owner_uid: u32) -> ProofResult<PathBuf> {
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != owner_uid
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("Debug Host root must be an owner-private real directory".into());
    }
    let canonical_root = root.canonicalize()?;
    if canonical_root.parent() != Some(Path::new("/tmp").canonicalize()?.as_path()) {
        return Err("Debug Host root must be a direct private child of /tmp".into());
    }
    Ok(canonical_root)
}

fn read_private_marker(path: &Path, owner_uid: u32) -> ProofResult<Value> {
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file()
        || before.uid() != owner_uid
        || before.permissions().mode() & 0o077 != 0
        || before.len() > PRIVATE_MARKER_MAX_BYTES
    {
        return Err("Debug Host marker must be a small owner-private regular file".into());
    }
    let mut marker_file = OpenOptions::new().read(true).open(path)?;
    let opened = marker_file.metadata()?;
    if opened.dev() != before.dev()
        || opened.ino() != before.ino()
        || opened.uid() != owner_uid
        || opened.permissions().mode() & 0o077 != 0
    {
        return Err("Debug Host marker changed during identity validation".into());
    }
    let mut bytes = Vec::new();
    marker_file
        .by_ref()
        .take(PRIVATE_MARKER_MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > PRIVATE_MARKER_MAX_BYTES {
        return Err("Debug Host marker exceeds its size bound".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn validate_native_socket(root: &Path, effective_uid: u32) -> ProofResult<NativeSocketProvenance> {
    let canonical_root = validate_private_root(root, effective_uid)?;
    let socket_directory = root.join("native-socket");
    validate_private_directory(&socket_directory, effective_uid)?;
    let rendezvous = socket_directory.join("app-server.sock");
    let daemon_directory = shared_daemon_socket_directory(effective_uid)?;
    validate_codex_socket_alias(
        &canonical_root,
        &rendezvous,
        &daemon_directory,
        effective_uid,
    )
}

fn shared_daemon_socket_directory(effective_uid: u32) -> ProofResult<PathBuf> {
    Ok(Path::new("/tmp")
        .canonicalize()?
        .join(format!("codex-daemon-{effective_uid}")))
}

fn validate_codex_socket_alias(
    root: &Path,
    rendezvous: &Path,
    daemon_directory: &Path,
    effective_uid: u32,
) -> ProofResult<NativeSocketProvenance> {
    let canonical_root = validate_private_root(root, effective_uid)?;
    let native_directory = canonical_root.join("native-socket");
    validate_private_directory(&native_directory, effective_uid)?;
    let rendezvous_parent = rendezvous
        .parent()
        .ok_or("Codex native rendezvous omitted its parent")?
        .canonicalize()?;
    if rendezvous_parent != native_directory
        || rendezvous.file_name() != Some(std::ffi::OsStr::new("app-server.sock"))
    {
        return Err("Codex native rendezvous does not match the private fixture alias".into());
    }

    let daemon_metadata = fs::symlink_metadata(daemon_directory)?;
    let canonical_daemon_directory = daemon_directory.canonicalize()?;
    let canonical_tmp = Path::new("/tmp").canonicalize()?;
    if !daemon_metadata.file_type().is_dir()
        || daemon_metadata.file_type().is_symlink()
        || daemon_metadata.uid() != effective_uid
        || daemon_metadata.permissions().mode() & 0o777 != 0o700
        || canonical_daemon_directory.parent() != Some(canonical_tmp.as_path())
    {
        return Err(
            "Codex daemon socket parent must be a nonsymlink owner-private direct /tmp directory"
                .into(),
        );
    }

    let rendezvous_metadata = fs::symlink_metadata(rendezvous)?;
    if !rendezvous_metadata.file_type().is_symlink() || rendezvous_metadata.uid() != effective_uid {
        return Err("Codex native rendezvous must be its owner-owned symlink alias".into());
    }
    let expected_socket = upstream_protected_socket_path(rendezvous, &canonical_daemon_directory)?;
    let resolved_socket = rendezvous.canonicalize()?;
    if resolved_socket != expected_socket {
        return Err("Codex rendezvous does not resolve to its exact upstream socket hash".into());
    }
    let socket_metadata = fs::symlink_metadata(&resolved_socket)?;
    if !socket_metadata.file_type().is_socket()
        || socket_metadata.file_type().is_symlink()
        || socket_metadata.uid() != effective_uid
        || socket_metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err("Resolved Codex target is not the owner-owned private Unix socket".into());
    }
    let socket_hash = resolved_socket
        .file_name()
        .ok_or("Resolved Codex socket omitted its hash filename")?
        .to_string_lossy()
        .into_owned();
    Ok(NativeSocketProvenance {
        alias: rendezvous.to_owned(),
        resolved_socket,
        daemon_directory: canonical_daemon_directory,
        effective_uid,
        socket_hash,
    })
}

fn upstream_protected_socket_path(
    rendezvous: &Path,
    daemon_directory: &Path,
) -> ProofResult<PathBuf> {
    let requested_parent = rendezvous
        .parent()
        .ok_or("Codex rendezvous omitted its parent")?;
    let requested_name = rendezvous
        .file_name()
        .ok_or("Codex rendezvous omitted its filename")?;
    let canonical_requested_path = fs::canonicalize(requested_parent)?.join(requested_name);
    let hash = socket_hash_for_raw_path(canonical_requested_path.as_os_str());
    Ok(daemon_directory.canonicalize()?.join(hash))
}

fn validate_private_directory(path: &Path, effective_uid: u32) -> ProofResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != effective_uid
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("Fixture socket parent must be a nonsymlink owner-private directory".into());
    }
    Ok(())
}

fn validate_operator_and_control_sockets(root: &Path, effective_uid: u32) -> ProofResult<()> {
    let canonical_root = validate_private_root(root, effective_uid)?;
    let collaboration_directory = canonical_root.join("agent-communication");
    validate_private_directory(&collaboration_directory, effective_uid)?;
    validate_owner_socket(
        &canonical_root,
        &canonical_root.join("host.sock"),
        effective_uid,
    )?;
    validate_owner_socket(
        &canonical_root,
        &collaboration_directory.join("control.sock"),
        effective_uid,
    )
}

fn validate_owner_socket(root: &Path, socket_path: &Path, effective_uid: u32) -> ProofResult<()> {
    let metadata = fs::symlink_metadata(socket_path)?;
    if !metadata.file_type().is_socket()
        || metadata.file_type().is_symlink()
        || metadata.uid() != effective_uid
        || metadata.permissions().mode() & 0o777 != 0o600
        || !socket_path.canonicalize()?.starts_with(root)
    {
        return Err(
            "Host operator/control target must be an in-root owner-only Unix socket".into(),
        );
    }
    Ok(())
}

fn effective_uid() -> ProofResult<u32> {
    let output = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .env_clear()
        .output()?;
    if !output.status.success() {
        return Err("Could not read the current effective UID".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}

fn socket_hash_for_raw_path(path: &std::ffi::OsStr) -> String {
    format!("{:x}", Sha256::digest(path.as_bytes()))
}

fn required_string<'a>(value: &'a Value, key: &str) -> ProofResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Debug Host marker omitted {key}").into())
}

fn foreground_cli_binary() -> ProofResult<PathBuf> {
    let test_binary = PathBuf::from(env!("CARGO_BIN_EXE_agent-collaboration"));
    let cli_binary = test_binary.with_file_name("codex-router");
    if !cli_binary.is_file() {
        return Err("Adjacent compiled codex-router CLI binary is missing".into());
    }
    Ok(cli_binary)
}

fn matrix_cli_argv_matches(command: &str, executable: &Path, root: &Path, port: u16) -> bool {
    let expected = [
        executable.as_os_str().to_string_lossy().into_owned(),
        "host".to_owned(),
        "--router-root".to_owned(),
        root.as_os_str().to_string_lossy().into_owned(),
        "--port".to_owned(),
        port.to_string(),
        "--mcp-bind".to_owned(),
        "127.0.0.1:43128".to_owned(),
        "--require-debug-isolation".to_owned(),
    ];
    command
        .split_whitespace()
        .eq(expected.iter().map(String::as_str))
}

fn automation_debug_host_argv_matches(
    command: &str,
    marker_run_directory: &str,
    canonical_root: &Path,
) -> bool {
    if !command.contains("automation-debug-host") {
        return false;
    }
    let marker_argument = format!("--run-directory {marker_run_directory}");
    let canonical_argument = format!("--run-directory {}", canonical_root.display());
    let has_bounded_argument = |argument: &str| {
        command.match_indices(argument).any(|(start, _)| {
            let end = start + argument.len();
            let starts_at_boundary = start == 0
                || command
                    .as_bytes()
                    .get(start - 1)
                    .is_some_and(u8::is_ascii_whitespace);
            let ends_at_boundary = end == command.len()
                || command
                    .as_bytes()
                    .get(end)
                    .is_some_and(u8::is_ascii_whitespace);
            starts_at_boundary && ends_at_boundary
        })
    };
    has_bounded_argument(&marker_argument) || has_bounded_argument(&canonical_argument)
}

async fn inspect_process_command(process_id: u32) -> ProofResult<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("/bin/ps")
            .args(["-ww", "-p", &process_id.to_string(), "-o", "command="])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    let command = String::from_utf8(output.stdout)?;
    if !output.status.success() || command.trim().is_empty() {
        return Err("Owned Host process is not present in the process table".into());
    }
    Ok(command.trim().to_owned())
}
