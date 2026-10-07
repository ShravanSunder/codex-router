use codex_router_descriptor_boundary::{BoundaryError, DescriptorGate, OwnedSocket, UnixReceipt};
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
async fn receive_literal_message(
    receipt: &UnixReceipt,
    expected: &[u8],
    failure_description: &str,
) -> TestResult {
    let mut received = vec![0; expected.len()];
    let mut offset = 0;
    while offset < received.len() {
        let Some(remaining) = received.get_mut(offset..) else {
            return Err(
                format!("{failure_description}: read offset exceeded the message length").into(),
            );
        };
        let read = timeout(
            Duration::from_secs(1),
            receipt.read(remaining, false, DescriptorGate::global()),
        )
        .await;
        let read = match read {
            Ok(Ok(read)) => read,
            Ok(Err(error)) => {
                return Err(format!("{failure_description}: read failed: {error}").into());
            }
            Err(_) => {
                return Err(format!("{failure_description}: timed out while reading").into());
            }
        };
        if read.bytes == 0 {
            return Err(
                format!("{failure_description}: peer closed before the message completed").into(),
            );
        }
        offset += read.bytes;
    }
    if received.as_slice() != expected {
        return Err(
            format!("{failure_description}: received {received:?}, expected {expected:?}").into(),
        );
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
    drop(stale);

    match timeout(
        Duration::from_secs(1),
        OwnedSocket::connect_unix(&path, DescriptorGate::global()),
    )
    .await
    {
        Ok(Err(BoundaryError::Io(error)))
            if error.kind() == std::io::ErrorKind::ConnectionRefused => {}
        Ok(Err(error)) => {
            return Err(format!(
                "closed stale operator socket returned {error}, expected connection refused"
            )
            .into());
        }
        Ok(Ok(_)) => {
            return Err(
                "closed stale operator socket accepted a connection before registry bind".into(),
            );
        }
        Err(_) => {
            return Err(
                "closed stale operator socket did not refuse a connection within one second".into(),
            );
        }
    }

    let mut registry = ListenerRegistry::new(SingletonAuthority::acquire(&lock).await?);
    registry.bind_operator(path.clone()).await?;
    if (std::fs::metadata(&path)?.permissions().mode() & 0o777) != (0o600) {
        return Err("registry did not set the reclaimed operator socket mode to 0600".into());
    }

    const OPERATOR_REQUEST: &[u8] = b"keeper operator request";
    const OPERATOR_REPLY: &[u8] = b"keeper operator reply";
    {
        let gate = DescriptorGate::global();
        let client = match timeout(
            Duration::from_secs(1),
            OwnedSocket::connect_unix(&path, gate),
        )
        .await
        {
            Ok(Ok(client)) => client,
            Ok(Err(error)) => {
                return Err(format!(
                    "newly bound operator socket rejected a client connection: {error}"
                )
                .into());
            }
            Err(_) => {
                return Err("connecting to the newly bound operator socket timed out".into());
            }
        };
        let listener = registry.operator_listener()?;
        let accepted = match timeout(Duration::from_secs(1), listener.accept(gate)).await {
            Ok(Ok(accepted)) => accepted,
            Ok(Err(error)) => {
                return Err(format!(
                    "registered operator listener failed to accept its client: {error}"
                )
                .into());
            }
            Err(_) => {
                return Err(
                    "registered operator listener did not accept its client within one second"
                        .into(),
                );
            }
        };

        let client_receipt = UnixReceipt::new(client.duplicate(gate).await?);
        let client_writer = client.duplicate(gate).await?.into_writer();
        let listener_receipt = UnixReceipt::new(accepted.duplicate(gate).await?);
        let listener_writer = accepted.duplicate(gate).await?.into_writer();

        match timeout(
            Duration::from_secs(1),
            client_writer.write_all(OPERATOR_REQUEST),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(format!(
                    "client failed to write the literal operator request: {error}"
                )
                .into());
            }
            Err(_) => return Err("client timed out writing the literal operator request".into()),
        }
        receive_literal_message(
            &listener_receipt,
            OPERATOR_REQUEST,
            "registered operator listener did not receive the literal first request",
        )
        .await?;

        match timeout(
            Duration::from_secs(1),
            listener_writer.write_all(OPERATOR_REPLY),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(format!(
                    "registered operator listener failed to write its literal reply: {error}"
                )
                .into());
            }
            Err(_) => {
                return Err(
                    "registered operator listener timed out writing its literal reply".into(),
                );
            }
        }
        receive_literal_message(
            &client_receipt,
            OPERATOR_REPLY,
            "client did not receive the literal reply from the registered operator listener",
        )
        .await?;
    }
    if !(matches!(
        registry
            .bind_operator(directory.path().join("other.sock"))
            .await,
        Err(RegistryError::OperatorPathMismatch)
    )) {
        return Err(
            "registry did not reject a second operator path as OperatorPathMismatch".into(),
        );
    }
    drop(registry);
    let live = std::os::unix::net::UnixListener::bind(&path)?;
    let before = std::fs::metadata(&path)?;
    let mut registry = ListenerRegistry::new(SingletonAuthority::acquire(&lock).await?);
    if !(matches!(
        registry.bind_operator(path.clone()).await,
        Err(RegistryError::OperatorOccupied)
    )) {
        return Err(
            "registry did not retain a foreign live operator socket as OperatorOccupied".into(),
        );
    }
    drop(registry);
    if (std::fs::metadata(&path)?.ino()) != (before.ino()) {
        return Err("registry changed the inode of a foreign live operator socket".into());
    }
    drop(live);
    std::fs::remove_file(&path)?;
    std::fs::write(&path, b"foreign regular file")?;
    let mut registry = ListenerRegistry::new(SingletonAuthority::acquire(&lock).await?);
    if !(matches!(
        registry.bind_operator(path.clone()).await,
        Err(RegistryError::OperatorOccupied)
    )) {
        return Err("registry did not retain a foreign regular file as OperatorOccupied".into());
    }
    drop(registry);
    if (std::fs::read(&path)?) != (b"foreign regular file") {
        return Err("registry changed the contents of a foreign regular file".into());
    }
    Ok(())
}
