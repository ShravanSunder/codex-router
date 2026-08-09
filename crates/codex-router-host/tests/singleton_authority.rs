use std::fs::OpenOptions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use codex_router_host::HostCoordinationPaths;
use codex_router_host::HostInstance;
use codex_router_host::InstanceAcquireError;
use codex_router_host::inherited_lock_marker;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn live_contender_never_unlinks_and_next_owner_replaces_stale_socket()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("singleton")?;
    let paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let held_lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(paths.instance_lock())?;
    held_lock.try_lock()?;
    let stale_listener = std::os::unix::net::UnixListener::bind(paths.operator_socket())?;
    drop(stale_listener);
    let stale_inode = std::fs::metadata(paths.operator_socket())?.ino();

    let contender = HostInstance::acquire(paths.clone());
    check(
        matches!(contender, Err(InstanceAcquireError::AlreadyRunning)),
        "live singleton contender must be rejected",
    )?;
    check_equal(
        std::fs::metadata(paths.operator_socket())?.ino(),
        stale_inode,
        "contender must not unlink the existing socket",
    )?;

    held_lock.unlock()?;
    drop(held_lock);
    let owner = HostInstance::acquire(paths.clone())?;
    let rebound_inode = std::fs::metadata(paths.operator_socket())?.ino();
    check(
        rebound_inode != stale_inode,
        "new lock owner must replace the stale socket",
    )?;
    check_equal(
        std::fs::metadata(paths.operator_socket())?
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "operator socket must be owner-only",
    )?;
    drop(owner);
    check(
        !paths.operator_socket().exists(),
        "normal owner drop must remove the socket",
    )?;
    check(
        paths.instance_lock().exists(),
        "stable lock artifact must remain inert on disk",
    )?;
    Ok(())
}

#[test]
fn inherited_lock_bootstrap_validates_same_version_and_retains_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("inherited-lock")?;
    let paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let held_lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(paths.instance_lock())?;
    held_lock.try_lock()?;
    let stale_listener = std::os::unix::net::UnixListener::bind(paths.operator_socket())?;
    drop(stale_listener);

    let output = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("inherited_lock_child_entrypoint")
        .arg("--nocapture")
        .stdin(Stdio::from(held_lock.try_clone()?))
        .env("CODEX_ROUTER_HOST_TEST_INHERITED_CHILD", "1")
        .env(
            "CODEX_ROUTER_HOST_TEST_OPERATOR_SOCKET",
            paths.operator_socket(),
        )
        .env(
            "CODEX_ROUTER_HOST_TEST_INSTANCE_LOCK",
            paths.instance_lock(),
        )
        .env(
            "CODEX_ROUTER_HOST_TEST_INHERITED_MARKER",
            inherited_lock_marker(),
        )
        .output()?;

    check(
        output.status.success(),
        &format!(
            "inherited child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    check(
        !paths.operator_socket().exists(),
        "inherited owner must clean up its socket",
    )?;
    Ok(())
}

#[test]
fn prepare_lock_for_exec_uses_stdin_without_unsafe_code() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = TestDirectory::new("prepare-exec")?;
    let paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let output = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("prepare_lock_child_entrypoint")
        .arg("--nocapture")
        .env("CODEX_ROUTER_HOST_TEST_PREPARE_CHILD", "1")
        .env(
            "CODEX_ROUTER_HOST_TEST_OPERATOR_SOCKET",
            paths.operator_socket(),
        )
        .env(
            "CODEX_ROUTER_HOST_TEST_INSTANCE_LOCK",
            paths.instance_lock(),
        )
        .output()?;

    check(
        output.status.success(),
        &format!(
            "prepare child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    Ok(())
}

#[test]
fn failed_exec_releases_prepared_singleton_authority() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("failed-exec-lock")?;
    let paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let output = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("failed_exec_lock_release_child_entrypoint")
        .arg("--nocapture")
        .env("CODEX_ROUTER_HOST_TEST_FAILED_EXEC_CHILD", "1")
        .env(
            "CODEX_ROUTER_HOST_TEST_OPERATOR_SOCKET",
            paths.operator_socket(),
        )
        .env(
            "CODEX_ROUTER_HOST_TEST_INSTANCE_LOCK",
            paths.instance_lock(),
        )
        .output()?;

    check(
        output.status.success(),
        &format!(
            "failed-exec lock child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

#[tokio::test]
async fn inherited_lock_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_TEST_INHERITED_CHILD").is_none() {
        return Ok(());
    }
    let paths = child_coordination_paths()?;
    let marker = std::env::var_os("CODEX_ROUTER_HOST_TEST_INHERITED_MARKER")
        .ok_or("inherited child marker is missing")?;

    let instance = HostInstance::acquire_inherited(paths.clone(), &marker)?;
    check(
        paths.operator_socket().exists(),
        "inherited owner must publish the operator socket",
    )?;
    let status = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("inherited_lock_descriptor_probe_child_entrypoint")
        .arg("--nocapture")
        .env("CODEX_ROUTER_HOST_TEST_INHERITED_DESCRIPTOR_PROBE", "1")
        .status()?;
    check(
        status.success(),
        "ordinary child spawn must not inherit singleton authority",
    )?;
    drop(instance);
    check(
        !paths.operator_socket().exists(),
        "inherited owner drop must remove the operator socket",
    )?;
    Ok(())
}

#[test]
fn inherited_lock_descriptor_probe_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_TEST_INHERITED_DESCRIPTOR_PROBE").is_none() {
        return Ok(());
    }
    let lock_path = std::env::var_os("CODEX_ROUTER_HOST_TEST_INSTANCE_LOCK")
        .ok_or("inherited lock artifact path is missing")?;
    let lock_stat = rustix::fs::stat(Path::new(&lock_path))?;
    if let Ok(stdin_stat) = rustix::fs::fstat(rustix::stdio::stdin()) {
        check(
            stdin_stat.st_dev != lock_stat.st_dev || stdin_stat.st_ino != lock_stat.st_ino,
            "inherited lock descriptor remained open across ordinary child exec",
        )?;
    }
    Ok(())
}

#[tokio::test]
async fn prepare_lock_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_TEST_PREPARE_CHILD").is_none() {
        return Ok(());
    }
    let paths = child_coordination_paths()?;
    let instance = HostInstance::acquire(paths.clone())?;

    instance.prepare_lock_for_exec()?;

    let inherited = rustix::fs::fstat(rustix::stdio::stdin())?;
    let artifact = rustix::fs::stat(paths.instance_lock())?;
    check_equal(
        inherited.st_dev,
        artifact.st_dev,
        "prepared descriptor device must match the lock artifact",
    )?;
    check_equal(
        inherited.st_ino,
        artifact.st_ino,
        "prepared descriptor inode must match the lock artifact",
    )?;
    check(
        !rustix::io::fcntl_getfd(rustix::stdio::stdin())?.contains(rustix::io::FdFlags::CLOEXEC),
        "prepared stdin descriptor must survive the immediate exec",
    )?;
    Ok(())
}

#[tokio::test]
async fn failed_exec_lock_release_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_TEST_FAILED_EXEC_CHILD").is_none() {
        return Ok(());
    }
    let paths = child_coordination_paths()?;
    let instance = HostInstance::acquire(paths.clone())?;
    instance.prepare_lock_for_exec()?;
    instance.release_prepared_lock_after_exec_failure()?;
    drop(instance);

    let next_owner = HostInstance::acquire(paths)?;
    drop(next_owner);
    Ok(())
}

fn child_coordination_paths() -> Result<HostCoordinationPaths, Box<dyn std::error::Error>> {
    let operator_socket = std::env::var_os("CODEX_ROUTER_HOST_TEST_OPERATOR_SOCKET")
        .ok_or("child operator socket is missing")?;
    let instance_lock = std::env::var_os("CODEX_ROUTER_HOST_TEST_INSTANCE_LOCK")
        .ok_or("child instance lock is missing")?;
    Ok(HostCoordinationPaths::new(
        PathBuf::from(operator_socket),
        PathBuf::from(instance_lock),
    ))
}

fn check(condition: bool, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

fn check_equal<TValue>(
    actual: TValue,
    expected: TValue,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>>
where
    TValue: PartialEq,
{
    check(actual == expected, message)
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> std::io::Result<Self> {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("crho-{name}-{}-{counter}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _cleanup_result = std::fs::remove_dir_all(&self.path);
    }
}
