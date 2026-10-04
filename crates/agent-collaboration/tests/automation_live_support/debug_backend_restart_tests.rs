use super::{
    effective_uid, inspect_process_command, matrix_cli_argv_matches,
    shared_daemon_socket_directory, socket_hash_for_raw_path, upstream_protected_socket_path,
    validate_codex_socket_alias,
};
use std::{
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::UnixListener,
    },
    path::Path,
};
use tokio::process::Command;

#[test]
fn foreground_cli_guard_requires_exact_executable_root_port_and_isolation() {
    let executable = Path::new("/tmp/isolated/codex-router");
    let root = Path::new("/tmp/u6-fixture");
    let valid = format!(
        "{} host --router-root {} --port 43127 --mcp-bind 127.0.0.1:43128 --require-debug-isolation",
        executable.display(),
        root.display()
    );

    assert!(matrix_cli_argv_matches(&valid, executable, root, 43127));
    assert!(!matrix_cli_argv_matches(
        &valid,
        executable,
        Path::new("/tmp/other-fixture"),
        43127
    ));
    assert!(!matrix_cli_argv_matches(&valid, executable, root, 43128));
    assert!(!matrix_cli_argv_matches(
        &valid.replace(" --require-debug-isolation", ""),
        executable,
        root,
        43127
    ));
    assert!(!matrix_cli_argv_matches(
        &valid,
        Path::new("/tmp/other/codex-router"),
        root,
        43127
    ));
}

#[tokio::test]
async fn live_foreign_process_is_rejected_without_an_operator_request() -> super::ProofResult<()> {
    let mut foreign_process = Command::new("/bin/sleep")
        .env_clear()
        .arg("60")
        .kill_on_drop(true)
        .spawn()?;
    let foreign_pid = foreign_process
        .id()
        .ok_or("sleep fixture did not publish its PID")?;
    let observed_command = inspect_process_command(foreign_pid).await;
    foreign_process.kill().await?;
    let _exit = foreign_process.wait().await?;

    let command = observed_command?;
    if matrix_cli_argv_matches(
        &command,
        Path::new("/tmp/isolated/codex-router"),
        Path::new("/tmp/u6-fixture"),
        43127,
    ) {
        return Err("live foreign process matched the owned foreground CLI guard".into());
    }
    Ok(())
}

#[test]
fn upstream_socket_name_uses_sha256_of_raw_canonical_path_bytes() {
    assert_eq!(
        socket_hash_for_raw_path(std::ffi::OsStr::new(
            "/private/tmp/native-socket/app-server.sock"
        )),
        "1507253aeae27c585842beb810da38c00905a06ac97f71ccfce2558d49bdedf8"
    );
}

#[test]
fn shared_daemon_path_uses_canonical_tmp_and_effective_uid() -> super::ProofResult<()> {
    let effective_uid = effective_uid()?;
    let shared_directory = shared_daemon_socket_directory(effective_uid)?;
    let canonical_tmp = Path::new("/tmp").canonicalize()?;
    let expected_name = format!("codex-daemon-{effective_uid}");
    if shared_directory.parent() != Some(canonical_tmp.as_path())
        || shared_directory.file_name().and_then(|name| name.to_str())
            != Some(expected_name.as_str())
    {
        return Err(
            "Codex daemon directory did not use canonical /tmp and the effective UID".into(),
        );
    }
    Ok(())
}

#[test]
fn private_codex_socket_alias_must_resolve_to_its_exact_hash() -> super::ProofResult<()> {
    let effective_uid = effective_uid()?;
    let fixture = SocketFixture::new(effective_uid)?;
    let expected_target =
        upstream_protected_socket_path(&fixture.alias, fixture.daemon_directory.path())?;
    let _expected_socket = bind_owner_socket(&expected_target)?;
    std::os::unix::fs::symlink(&expected_target, &fixture.alias)?;

    let provenance = validate_codex_socket_alias(
        fixture.root.path(),
        &fixture.alias,
        fixture.daemon_directory.path(),
        effective_uid,
    )?;
    let canonical_alias = fixture
        .alias
        .parent()
        .ok_or("fixture rendezvous omitted its parent")?
        .canonicalize()?
        .join(
            fixture
                .alias
                .file_name()
                .ok_or("fixture rendezvous omitted its filename")?,
        );
    let expected_hash = socket_hash_for_raw_path(canonical_alias.as_os_str());
    let expected_daemon_directory = fixture.daemon_directory.path().canonicalize()?;
    if provenance.resolved_socket != expected_target
        || provenance.socket_hash != expected_hash
        || provenance.daemon_directory != expected_daemon_directory
        || provenance.resolved_socket != expected_daemon_directory.join(expected_hash)
        || provenance.effective_uid != effective_uid
    {
        return Err(
            "Codex socket provenance did not match the independently computed path hash".into(),
        );
    }
    Ok(())
}

#[test]
fn private_codex_socket_alias_rejects_a_wrong_physical_target() -> super::ProofResult<()> {
    let effective_uid = effective_uid()?;
    let fixture = SocketFixture::new(effective_uid)?;
    let foreign_target = fixture.root.path().join("foreign-target.sock");
    let _foreign_socket = bind_owner_socket(&foreign_target)?;
    std::os::unix::fs::symlink(&foreign_target, &fixture.alias)?;

    if validate_codex_socket_alias(
        fixture.root.path(),
        &fixture.alias,
        fixture.daemon_directory.path(),
        effective_uid,
    )
    .is_ok()
    {
        return Err(
            "Codex rendezvous accepted a physical socket target outside its hash directory".into(),
        );
    }
    Ok(())
}

#[test]
fn private_codex_socket_alias_rejects_a_valid_socket_with_another_alias_hash()
-> super::ProofResult<()> {
    let effective_uid = effective_uid()?;
    let fixture = SocketFixture::new(effective_uid)?;
    let foreign_alias = fixture.root.path().join("native-socket").join("other.sock");
    let foreign_hash_target =
        upstream_protected_socket_path(&foreign_alias, fixture.daemon_directory.path())?;
    let _foreign_socket = bind_owner_socket(&foreign_hash_target)?;
    std::os::unix::fs::symlink(&foreign_hash_target, &fixture.alias)?;

    if validate_codex_socket_alias(
        fixture.root.path(),
        &fixture.alias,
        fixture.daemon_directory.path(),
        effective_uid,
    )
    .is_ok()
    {
        return Err("Codex rendezvous accepted a valid socket with another alias hash".into());
    }
    Ok(())
}

#[test]
fn private_codex_socket_alias_rejects_an_unsafe_daemon_parent() -> super::ProofResult<()> {
    let effective_uid = effective_uid()?;
    let fixture = SocketFixture::new(effective_uid)?;
    std::fs::set_permissions(
        fixture.daemon_directory.path(),
        std::fs::Permissions::from_mode(0o755),
    )?;

    if validate_codex_socket_alias(
        fixture.root.path(),
        &fixture.alias,
        fixture.daemon_directory.path(),
        effective_uid,
    )
    .is_ok()
    {
        return Err("Codex rendezvous accepted a world-accessible daemon parent".into());
    }
    Ok(())
}

#[test]
fn private_codex_socket_alias_rejects_a_symlinked_daemon_parent() -> super::ProofResult<()> {
    use std::os::unix::fs::symlink;

    let effective_uid = effective_uid()?;
    let fixture = SocketFixture::new(effective_uid)?;
    let expected_target =
        upstream_protected_socket_path(&fixture.alias, fixture.daemon_directory.path())?;
    let _expected_socket = bind_owner_socket(&expected_target)?;
    symlink(
        fixture.daemon_directory.path(),
        fixture.root.path().join("daemon-alias"),
    )?;

    if validate_codex_socket_alias(
        fixture.root.path(),
        &fixture.alias,
        &fixture.root.path().join("daemon-alias"),
        effective_uid,
    )
    .is_ok()
    {
        return Err("Codex rendezvous accepted a symlinked daemon parent".into());
    }
    Ok(())
}

struct SocketFixture {
    root: tempfile::TempDir,
    daemon_directory: tempfile::TempDir,
    alias: std::path::PathBuf,
}

impl SocketFixture {
    fn new(effective_uid: u32) -> super::ProofResult<Self> {
        let root = tempfile::Builder::new().prefix("u6s").tempdir_in("/tmp")?;
        let daemon_directory = tempfile::Builder::new().prefix("u6d").tempdir_in("/tmp")?;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(
            daemon_directory.path(),
            std::fs::Permissions::from_mode(0o700),
        )?;
        let native_directory = root.path().join("native-socket");
        std::fs::create_dir(&native_directory)?;
        std::fs::set_permissions(&native_directory, std::fs::Permissions::from_mode(0o700))?;
        if std::fs::metadata(root.path())?.uid() != effective_uid {
            return Err("temporary socket fixture root has the wrong effective owner".into());
        }
        Ok(Self {
            alias: native_directory.join("app-server.sock"),
            root,
            daemon_directory,
        })
    }
}

fn bind_owner_socket(path: &Path) -> super::ProofResult<UnixListener> {
    let socket = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(socket)
}
