use codex_router_descriptor_boundary::{DescriptorGate, OwnedPipe, OwnedSocket};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn created_pipe_pair_duplicate_and_socket_pair_are_cloexec() -> TestResult {
    let gate = DescriptorGate::global();
    let (reader, writer) = OwnedPipe::pair(gate).await?;
    let (socket, peer) = OwnedSocket::pair(gate).await?;
    let duplicate = socket.duplicate(gate).await?;
    for descriptor in [
        reader.as_fd(),
        writer.as_fd(),
        socket.as_fd(),
        peer.as_fd(),
        duplicate.as_fd(),
    ] {
        if !rustix::io::fcntl_getfd(descriptor)?.contains(rustix::io::FdFlags::CLOEXEC) {
            return Err("created or duplicated fd lacks CLOEXEC".into());
        }
    }
    writer.write_all(b"pipe bytes").await?;
    let mut bytes = [0; 10];
    reader.read_exact(&mut bytes).await?;
    if &bytes != b"pipe bytes" {
        return Err("pipe changed bytes".into());
    }
    Ok(())
}

#[tokio::test]
async fn idle_accept_and_connect_completion_do_not_hold_creation_gate() -> TestResult {
    use tokio::time::{Duration, timeout};
    let gate = DescriptorGate::global();
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let listener = codex_router_descriptor_boundary::OwnedListener::bind_unix(&path, gate).await?;
    let mut accepting = Box::pin(listener.accept(gate));
    tokio::select! { biased; _result=&mut accepting=>return Err("idle accept settled".into()), ()=tokio::task::yield_now()=>{} }
    let _exclusive = timeout(Duration::from_secs(1), gate.spawn()).await?;
    drop(_exclusive);
    let (connected, accepted) = tokio::join!(OwnedSocket::connect_unix(&path, gate), accepting);
    let connected = connected?;
    let accepted = accepted?;
    connected.send(b"hello", &[]).await?;
    let receipt = codex_router_descriptor_boundary::UnixReceipt::new(accepted);
    let mut bytes = [0; 5];
    if timeout(
        Duration::from_secs(1),
        receipt.read(&mut bytes, false, gate),
    )
    .await??
    .bytes
        != 5
        || &bytes != b"hello"
    {
        return Err("gated Unix connect/accept changed bytes".into());
    }
    Ok(())
}

#[test]
#[ignore = "compiled inventory fixture invoked by gated creation/spawn scenario"]
fn inventory_fixture() -> TestResult {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let forbidden: Vec<(u64, u64)> =
        serde_json::from_str(&std::env::var("GATED_FIXTURE_FORBIDDEN")?)?;
    let directory = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    for entry in std::fs::read_dir(directory)? {
        if let Ok(metadata) = std::fs::metadata(entry?.path()) {
            if forbidden.contains(&(metadata.dev(), metadata.ino())) {
                return Err("unrelated socket/pipe authority inherited by child".into());
            }
            if std::env::var_os("GATED_FIXTURE_NO_SOCKETS").is_some()
                && metadata.file_type().is_socket()
            {
                return Err(format!(
                    "socket authority inherited during concurrent creation: dev={} inode={}",
                    metadata.dev(),
                    metadata.ino()
                )
                .into());
            }
        }
    }
    println!("GATED_INVENTORY_OK");
    Ok(())
}
#[tokio::test]
async fn shared_creation_excludes_spawn_and_all_created_sockets_close_at_owner_end() -> TestResult {
    use std::process::Stdio;
    use tokio::time::{Duration, timeout};
    let gate = DescriptorGate::global();
    for _ in 0..8 {
        let (socket, peer) = OwnedSocket::pair(gate).await?;
        let duplicate = socket.duplicate(gate).await?;
        let (pipe_read, pipe_write) = OwnedPipe::pair(gate).await?;
        let mut forbidden = Vec::new();
        for fd in [
            socket.as_fd(),
            duplicate.as_fd(),
            pipe_read.as_fd(),
            pipe_write.as_fd(),
        ] {
            let stat = rustix::fs::fstat(fd)?;
            forbidden.push((stat.st_dev as u64, stat.st_ino as u64));
        }
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command
            .args(["--ignored", "--exact", "inventory_fixture", "--nocapture"])
            .env(
                "GATED_FIXTURE_FORBIDDEN",
                serde_json::to_string(&forbidden)?,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let shared = gate.creation().await;
        let mut spawning = Box::pin(gate.spawn_child(&mut command));
        tokio::select! {biased; _result=&mut spawning=>return Err("spawn crossed live shared creation guard".into()), ()=tokio::task::yield_now()=>{} }
        drop(shared);
        let child = timeout(Duration::from_secs(2), spawning).await??;
        let output = timeout(Duration::from_secs(2), child.wait_with_output()).await??;
        if !output.status.success()
            || !String::from_utf8_lossy(&output.stdout).contains("GATED_INVENTORY_OK")
        {
            return Err(format!(
                "inventory failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        drop(duplicate);
        drop(socket);
        drop(pipe_write);
        if timeout(
            Duration::from_secs(1),
            codex_router_descriptor_boundary::UnixReceipt::new(peer).read(&mut [0], false, gate),
        )
        .await??
        .bytes
            != 0
        {
            return Err("socket still inherited beyond owner end".into());
        }
        if timeout(Duration::from_secs(1), pipe_read.read(&mut [0])).await?? != 0 {
            return Err("pipe inherited beyond owner end".into());
        }
    }
    Ok(())
}

#[tokio::test]
async fn actual_connect_accept_pair_creation_races_exclusive_spawn_without_inherited_socket()
-> TestResult {
    use std::process::Stdio;
    use tokio::time::{Duration, timeout};
    let gate = DescriptorGate::global();
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("race.sock");
    let listener = codex_router_descriptor_boundary::OwnedListener::bind_unix(&path, gate).await?;
    for _ in 0..12 {
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command
            .args(["--ignored", "--exact", "inventory_fixture", "--nocapture"])
            .env("GATED_FIXTURE_FORBIDDEN", "[]")
            .env("GATED_FIXTURE_NO_SOCKETS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let (connecting, accepting, pair, spawning) = tokio::join!(
            OwnedSocket::connect_unix(&path, gate),
            listener.accept(gate),
            OwnedSocket::pair(gate),
            gate.spawn_child(&mut command)
        );
        let socket = connecting?;
        let accepted = accepting?;
        let (created, peer) = pair?;
        let output = timeout(Duration::from_secs(3), spawning?.wait_with_output()).await??;
        if !output.status.success()
            || !String::from_utf8_lossy(&output.stdout).contains("GATED_INVENTORY_OK")
        {
            return Err(format!(
                "creation race inventory failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        drop(accepted);
        drop(created);
        for socket in [socket, peer] {
            if timeout(
                Duration::from_secs(1),
                codex_router_descriptor_boundary::UnixReceipt::new(socket).read(
                    &mut [0],
                    false,
                    gate,
                ),
            )
            .await??
            .bytes
                != 0
            {
                return Err("owner-close failed after connect/accept/pair spawn race".into());
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn synchronous_spawn_wrapper_excludes_creation_and_runs_without_blocking_acquisition_on_runtime()
-> TestResult {
    use std::process::Stdio;
    use tokio::time::{Duration, timeout};
    let gate = DescriptorGate::global();
    let shared = gate.creation().await;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args(["--ignored", "--exact", "inventory_fixture", "--nocapture"])
        .env("GATED_FIXTURE_FORBIDDEN", "[]")
        .env("GATED_FIXTURE_NO_SOCKETS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut spawning = Box::pin(gate.spawn_blocking(move || {
        command
            .spawn()
            .map_err(codex_router_descriptor_boundary::BoundaryError::Io)
    }));
    tokio::select! {biased; _result=&mut spawning=>return Err("syncspawn crossed sharedcreation guard".into()),()=tokio::task::yield_now()=>{}}
    drop(shared);
    let child = timeout(Duration::from_secs(2), spawning).await??;
    let output = timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking(move || child.wait_with_output()),
    )
    .await???;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout).contains("GATED_INVENTORY_OK")
    {
        return Err("syncspawn inventory or execution failed".into());
    }
    Ok(())
}

#[tokio::test]
async fn adopted_listener_duplicate_accepts_and_rejects_wrong_association() -> TestResult {
    use codex_router_descriptor_boundary::OwnedListener;
    let gate = DescriptorGate::global();
    let original = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
    let address = original.tcp_address()?;
    let adopted =
        OwnedListener::from_tcp_owned(gate.duplicate(original.as_fd()).await?, address, gate)
            .await?;
    if !(OwnedListener::from_tcp_owned(
        gate.duplicate(original.as_fd()).await?,
        "127.0.0.1:1".parse()?,
        gate,
    )
    .await
    .is_err())
    {
        return Err("owned_carriers scenario assertion failed".into());
    }
    if !(OwnedListener::from_tcp_owned(std::fs::File::open("/dev/null")?.into(), address, gate)
        .await
        .is_err())
    {
        return Err("owned_carriers scenario assertion failed".into());
    }
    let (peer, accepted) = tokio::join!(
        OwnedSocket::connect_tcp(address, gate),
        adopted.accept(gate)
    );
    let peer = peer?;
    if !(rustix::io::fcntl_getfd(accepted?.as_fd())?.contains(rustix::io::FdFlags::CLOEXEC)) {
        return Err("owned_carriers scenario assertion failed".into());
    }
    drop(adopted);
    drop(peer);
    let (peer, accepted) = tokio::join!(
        OwnedSocket::connect_tcp(address, gate),
        original.accept(gate)
    );
    drop(peer?);
    drop(accepted?);
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("adopt.sock");
    let original = OwnedListener::bind_unix(&path, gate).await?;
    if !(OwnedListener::from_unix_owned(
        gate.duplicate(original.as_fd()).await?,
        &directory.path().join("wrong.sock"),
        gate,
    )
    .await
    .is_err())
    {
        return Err("owned_carriers scenario assertion failed".into());
    }
    let adopted =
        OwnedListener::from_unix_owned(gate.duplicate(original.as_fd()).await?, &path, gate)
            .await?;
    let (peer, accepted) =
        tokio::join!(OwnedSocket::connect_unix(&path, gate), adopted.accept(gate));
    drop(peer?);
    drop(accepted?);
    drop(adopted);
    if !(path.exists()) {
        return Err("owned_carriers scenario assertion failed".into());
    }
    Ok(())
}
