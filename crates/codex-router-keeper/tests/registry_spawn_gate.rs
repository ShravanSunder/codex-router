use codex_router_descriptor_boundary::{DescriptorGate, OwnedSocket, UnixReceipt};
use codex_router_keeper::{ListenerAddress, ListenerRegistry, SingletonAuthority};
use codex_router_keeper_protocol::ListenerKind;
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    process::Stdio,
};
use tokio::time::{Duration, timeout};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[test]
#[ignore = "compiled inventory entrypoint invoked by permanent registry gate scenario"]
fn registry_inventory_fixture() -> TestResult {
    let forbidden: Vec<(u64, u64)> = serde_json::from_str(&std::env::var("REGISTRY_FORBIDDEN")?)?;
    let directory = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    for entry in std::fs::read_dir(directory)? {
        if let Ok(metadata) = std::fs::metadata(entry?.path())
            && (forbidden.contains(&(metadata.dev(), metadata.ino()))
                || metadata.file_type().is_socket())
        {
            return Err("unrelated registry authority inherited by child".into());
        }
    }
    println!("REGISTRY_INVENTORY_CLEAN");
    Ok(())
}
#[tokio::test]
async fn registry_creation_dup_and_accept_obey_spawn_gate_and_owner_close_reaches_peer()
-> TestResult {
    let gate = DescriptorGate::global();
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let authority = SingletonAuthority::acquire(&directory.path().join("host.lock")).await?;
    let lock_stat = rustix::fs::fstat(authority.as_fd())?;
    let mut registry = ListenerRegistry::new(authority);
    let path = directory.path().join("race.sock");
    let exclusive = gate.spawn().await;
    let mut binding = Box::pin(registry.bind(
        ListenerKind::CollaborationControl,
        ListenerAddress::unix(path.clone())?,
    ));
    tokio::select! {biased; result=&mut binding=>return Err(format!("registry crossed exclusive spawn guard: {result:?}").into()), ()=tokio::task::yield_now()=>{}}
    if !(!path.exists()) {
        return Err("registry_spawn_gate scenario assertion failed".into());
    }
    drop(exclusive);
    timeout(Duration::from_secs(2), binding).await??;
    for _ in 0..8 {
        let exclusive = gate.spawn().await;
        let mut duplicating = Box::pin(registry.duplicate(&ListenerKind::CollaborationControl));
        tokio::select! {biased; _result=&mut duplicating=>return Err("registry duplication crossed spawn guard".into()), ()=tokio::task::yield_now()=>{}}
        drop(exclusive);
        let grant = timeout(Duration::from_secs(2), duplicating).await??;
        let listener_stat = rustix::fs::fstat(grant.descriptor())?;
        let (_, _, descriptor) = grant.into_parts();
        let adopted = codex_router_descriptor_boundary::OwnedListener::from_unix_owned(
            descriptor, &path, gate,
        )
        .await?;
        let mut accepting = Box::pin(adopted.accept(gate));
        tokio::select! {biased; _result=&mut accepting=>return Err("idle accept unexpectedly completed".into()), ()=tokio::task::yield_now()=>{}}
        // Idle accept must release the shared guard, so a compiled child can actually spawn.
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command
            .args([
                "--ignored",
                "--exact",
                "registry_inventory_fixture",
                "--nocapture",
            ])
            .env(
                "REGISTRY_FORBIDDEN",
                serde_json::to_string(&[
                    (lock_stat.st_dev as u64, lock_stat.st_ino as u64),
                    (listener_stat.st_dev as u64, listener_stat.st_ino as u64),
                ])?,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let shared = gate.creation().await;
        let mut spawning = Box::pin(gate.spawn_child(&mut command));
        tokio::select! {biased; _result=&mut spawning=>return Err("spawn crossed live shared guard".into()), ()=tokio::task::yield_now()=>{}}
        drop(shared);
        let (connecting, accepted, child) =
            tokio::join!(OwnedSocket::connect_unix(&path, gate), accepting, spawning);
        let peer = connecting?;
        let accepted = accepted?;
        let output = timeout(Duration::from_secs(3), child?.wait_with_output()).await??;
        if !output.status.success()
            || !String::from_utf8_lossy(&output.stdout).contains("REGISTRY_INVENTORY_CLEAN")
        {
            return Err(format!(
                "registry child inventory failed: {:?} {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        drop(accepted);
        let receipt = UnixReceipt::new(peer);
        if (timeout(Duration::from_secs(2), receipt.read(&mut [0], false, gate))
            .await??
            .bytes)
            != (0)
        {
            return Err("registry_spawn_gate scenario assertion failed".into());
        }
        drop(adopted);
    }
    drop(registry);
    if !(!path.exists()) {
        return Err("registry_spawn_gate scenario assertion failed".into());
    }
    Ok(())
}
