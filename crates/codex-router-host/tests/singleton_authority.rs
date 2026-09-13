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
use codex_router_host::ProcessGroupChild;
use codex_router_host::inherited_lock_environment;
use codex_router_host::inherited_lock_marker;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn inherited_lock_marker_names_the_stable_handoff_protocol() {
    assert_eq!(inherited_lock_marker(), "codex-router-host-handoff/v1");
    assert!(!inherited_lock_marker().contains(env!("CARGO_PKG_VERSION")));
}

#[tokio::test]
async fn inherited_lock_bootstrap_rejects_malformed_marker_without_disturbing_owner()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("malformed-marker")?;
    let paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let owner = HostInstance::acquire(paths.clone())?;
    let owner_socket_inode = std::fs::metadata(paths.operator_socket())?.ino();

    let inherited = HostInstance::acquire_inherited(paths.clone(), "malformed".as_ref());
    check(
        matches!(
            inherited,
            Err(InstanceAcquireError::InheritedMarkerMismatch)
        ),
        "malformed handoff marker must return the typed marker-mismatch error",
    )?;
    check_equal(
        std::fs::metadata(paths.operator_socket())?.ino(),
        owner_socket_inode,
        "malformed handoff must preserve the existing owner socket",
    )?;
    check(
        matches!(
            HostInstance::acquire(paths),
            Err(InstanceAcquireError::AlreadyRunning)
        ),
        "malformed handoff must preserve the existing singleton owner",
    )?;

    drop(owner);
    Ok(())
}

#[tokio::test]
async fn inherited_lock_bootstrap_rejects_wrong_descriptor_without_disturbing_owner()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TestDirectory::new("wrong-descriptor")?;
    let paths = HostCoordinationPaths::new(
        directory.path().join("operator.sock"),
        directory.path().join("instance.lock"),
    );
    let owner = HostInstance::acquire(paths.clone())?;
    let owner_socket_inode = std::fs::metadata(paths.operator_socket())?.ino();
    let output = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("wrong_inherited_lock_descriptor_child_entrypoint")
        .arg("--nocapture")
        .stdin(Stdio::null())
        .env("CODEX_ROUTER_HOST_TEST_WRONG_DESCRIPTOR_CHILD", "1")
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
            "wrong-descriptor child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    check_equal(
        std::fs::metadata(paths.operator_socket())?.ino(),
        owner_socket_inode,
        "wrong descriptor handoff must preserve the existing owner socket",
    )?;
    check(
        matches!(
            HostInstance::acquire(paths),
            Err(InstanceAcquireError::AlreadyRunning)
        ),
        "wrong descriptor handoff must preserve the existing singleton owner",
    )?;

    drop(owner);
    Ok(())
}

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
fn inherited_lock_bootstrap_filters_marker_before_fresh_descendant_acquisition()
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
        .env(inherited_lock_environment(), inherited_lock_marker())
        .env(
            "CODEX_ROUTER_HOST_TEST_FRESH_OPERATOR_SOCKET",
            directory.path().join("fresh-operator.sock"),
        )
        .env(
            "CODEX_ROUTER_HOST_TEST_FRESH_INSTANCE_LOCK",
            directory.path().join("fresh-instance.lock"),
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
    let marker = std::env::var_os(inherited_lock_environment())
        .ok_or("inherited child marker is missing")?;

    let instance = HostInstance::acquire_inherited(paths.clone(), &marker)?;
    check(
        paths.operator_socket().exists(),
        "inherited owner must publish the operator socket",
    )?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .arg("--exact")
        .arg("fresh_descendant_host_child_entrypoint")
        .arg("--nocapture")
        .env("CODEX_ROUTER_HOST_TEST_FRESH_DESCENDANT", "1")
        .env("CODEX_ROUTER_HOST_TEST_RETAINED_ENVIRONMENT", "retained")
        .stdout(Stdio::null());
    let mut descendant = ProcessGroupChild::spawn(&mut command)?;
    let status =
        match tokio::time::timeout(std::time::Duration::from_secs(2), descendant.wait()).await {
            Ok(wait_result) => wait_result?,
            Err(timeout_error) => {
                descendant.send_group_kill()?;
                let _reaped_status = descendant.wait().await?;
                return Err(timeout_error.into());
            }
        };
    check(
        status.success(),
        "ordinary child must use normal singleton acquisition for a fresh root",
    )?;
    drop(instance);
    check(
        !paths.operator_socket().exists(),
        "inherited owner drop must remove the operator socket",
    )?;
    Ok(())
}

#[tokio::test]
async fn fresh_descendant_host_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_TEST_FRESH_DESCENDANT").is_none() {
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
    check_equal(
        std::env::var("CODEX_ROUTER_HOST_TEST_RETAINED_ENVIRONMENT")?,
        "retained".to_owned(),
        "ordinary child must preserve unrelated inherited and explicit environment",
    )?;
    let paths = HostCoordinationPaths::new(
        PathBuf::from(
            std::env::var_os("CODEX_ROUTER_HOST_TEST_FRESH_OPERATOR_SOCKET")
                .ok_or("fresh descendant operator socket is missing")?,
        ),
        PathBuf::from(
            std::env::var_os("CODEX_ROUTER_HOST_TEST_FRESH_INSTANCE_LOCK")
                .ok_or("fresh descendant instance lock is missing")?,
        ),
    );
    let instance = match std::env::var_os(inherited_lock_environment()) {
        Some(marker) => HostInstance::acquire_inherited(paths.clone(), &marker)?,
        None => HostInstance::acquire(paths.clone())?,
    };
    check(
        paths.operator_socket().exists(),
        "fresh descendant must publish its operator socket through normal acquisition",
    )?;
    drop(instance);
    Ok(())
}

#[test]
fn wrong_inherited_lock_descriptor_child_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CODEX_ROUTER_HOST_TEST_WRONG_DESCRIPTOR_CHILD").is_none() {
        return Ok(());
    }
    let inherited = HostInstance::acquire_inherited(
        child_coordination_paths()?,
        inherited_lock_marker().as_ref(),
    );
    check(
        matches!(inherited, Err(InstanceAcquireError::InheritedLockMismatch)),
        "wrong inherited descriptor must return the typed lock-mismatch error",
    )
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
