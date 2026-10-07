use codex_router_descriptor_boundary::{DescriptorGate, OwnedSocket};
use codex_router_keeper::{ListenerRegistry, RegistryError, SingletonAuthority};
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    process::Stdio,
};
use tokio::time::{Duration, timeout};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[tokio::test]
#[ignore = "compiled contender entrypoint actually invoked by permanent singleton scenario"]
async fn singleton_contender_fixture() -> TestResult {
    let lock = PathBuf::from(std::env::var("KEEPER_LOCK")?);
    match SingletonAuthority::acquire(&lock).await {
        Err(RegistryError::AlreadyRunning) => println!("KEEPER_BUSY_NO_ENDPOINT_EFFECT"),
        Ok(authority) => {
            let mut registry = ListenerRegistry::new(authority);
            registry
                .bind_operator(lock.parent().ok_or("lock parent absent")?.join("host.sock"))
                .await?;
            println!("KEEPER_ACQUIRED_AND_BOUND");
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
async fn contender(lock: &std::path::Path, expected: &str) -> TestResult {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "singleton_contender_fixture",
            "--nocapture",
        ])
        .env("KEEPER_LOCK", lock)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = DescriptorGate::global().spawn_child(&mut command).await?;
    let output = timeout(Duration::from_secs(3), child.wait_with_output()).await??;
    if !output.status.success() || !String::from_utf8_lossy(&output.stdout).contains(expected) {
        return Err(format!(
            "contender failed: {:?} {} {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}
#[tokio::test]
async fn real_contender_cannot_touch_endpoint_until_registry_authority_drops() -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let lock = directory.path().join("host.lock");
    std::fs::write(&lock, b"stable artifact content")?;
    let mut registry = ListenerRegistry::new(SingletonAuthority::acquire(&lock).await?);
    if (std::fs::metadata(&lock)?.permissions().mode() & 0o777) != (0o600) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    let path = directory.path().join("host.sock");
    registry.bind_operator(path.clone()).await?;
    let before = std::fs::metadata(&path)?;
    let lock_before = std::fs::metadata(&lock)?;
    contender(&lock, "KEEPER_BUSY_NO_ENDPOINT_EFFECT").await?;
    let after = std::fs::metadata(&path)?;
    if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    if (std::fs::read(&lock)?) != (b"stable artifact content") {
        return Err("singleton_processes scenario assertion failed".into());
    }
    let gate = DescriptorGate::global();
    let (peer, accepted) = tokio::join!(
        OwnedSocket::connect_unix(&path, gate),
        registry.operator_listener()?.accept(gate)
    );
    drop(peer?);
    drop(accepted?);
    drop(registry);
    if !(!path.exists()) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    contender(&lock, "KEEPER_ACQUIRED_AND_BOUND").await?;
    if (std::fs::metadata(&lock)?.ino()) != (lock_before.ino()) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    if (std::fs::read(&lock)?) != (b"stable artifact content") {
        return Err("singleton_processes scenario assertion failed".into());
    }
    Ok(())
}
#[tokio::test]
async fn stale_operator_is_reclaimed_only_after_authority_and_foreign_live_node_is_retained()
-> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let lock = directory.path().join("host.lock");
    let path = directory.path().join("host.sock");
    let stale = std::os::unix::net::UnixListener::bind(&path)?;
    let stale_inode = std::fs::metadata(&path)?.ino();
    drop(stale);
    let mut registry = ListenerRegistry::new(SingletonAuthority::acquire(&lock).await?);
    registry.bind_operator(path.clone()).await?;
    if (std::fs::metadata(&path)?.ino()) == (stale_inode) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    if (std::fs::metadata(&path)?.permissions().mode() & 0o777) != (0o600) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    if !(matches!(
        registry
            .bind_operator(directory.path().join("other.sock"))
            .await,
        Err(RegistryError::OperatorPathMismatch)
    )) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    drop(registry);
    let live = std::os::unix::net::UnixListener::bind(&path)?;
    let before = std::fs::metadata(&path)?;
    let mut registry = ListenerRegistry::new(SingletonAuthority::acquire(&lock).await?);
    if !(matches!(
        registry.bind_operator(path.clone()).await,
        Err(RegistryError::OperatorOccupied)
    )) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    drop(registry);
    if (std::fs::metadata(&path)?.ino()) != (before.ino()) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    drop(live);
    std::fs::remove_file(&path)?;
    std::fs::write(&path, b"foreign regular file")?;
    let mut registry = ListenerRegistry::new(SingletonAuthority::acquire(&lock).await?);
    if !(matches!(
        registry.bind_operator(path.clone()).await,
        Err(RegistryError::OperatorOccupied)
    )) {
        return Err("singleton_processes scenario assertion failed".into());
    }
    drop(registry);
    if (std::fs::read(&path)?) != (b"foreign regular file") {
        return Err("singleton_processes scenario assertion failed".into());
    }
    Ok(())
}
