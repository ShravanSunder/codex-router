use codex_router_descriptor_boundary::{
    DescriptorGate, OwnedListener, OwnedPipe, OwnedSocket, PipeReader, PipeWriter, UnixReceipt,
};
use codex_router_keeper::{ListenerAddress, ListenerRegistry, SingletonAuthority};
use codex_router_keeper_protocol::{
    ChildGrantExpectation, ChildGrantFrame, ChildLaunchContext, DescriptorSpec, GrantReceiver,
    GrantSender, ListenerKind,
};
use std::{os::unix::fs::PermissionsExt, process::Stdio};
use tokio::time::{Duration, timeout};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const REQUEST: &[u8] = b"keeper request";
const REPLY: &[u8] = b"keeper reply";
const CHILD_LAUNCH_LITERAL: &str = r#"{"role":"agentProxyServices","image":{"retainedPath":"/fixture/codex-router","fileSha256":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"device":1,"inode":2},"fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
const BOOTSTRAP_PROXY_HTTP_LITERAL: &str = r#"{"type":"bootstrap","launch":{"role":"agentProxyServices","image":{"retainedPath":"/fixture/codex-router","fileSha256":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"device":1,"inode":2},"fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"},"listeners":[{"kind":"proxyHttp"}]}"#;
const BOOTSTRAP_NATIVE_RELAY_LITERAL: &str = r#"{"type":"bootstrap","launch":{"role":"agentProxyServices","image":{"retainedPath":"/fixture/codex-router","fileSha256":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"device":1,"inode":2},"fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"},"listeners":[{"kind":"nativeRelay"}]}"#;
const LISTENER_GRANT_PROXY_HTTP_LITERAL: &str =
    r#"{"type":"listenerGrant","listeners":[{"kind":"proxyHttp"}]}"#;
async fn receive_exact(socket: OwnedSocket, expected: &[u8]) -> TestResult {
    let receipt = UnixReceipt::new(socket);
    let mut bytes = vec![0; expected.len()];
    let mut offset = 0;
    while let Some(remaining) = bytes.get_mut(offset..).filter(|part| !part.is_empty()) {
        let count = timeout(
            Duration::from_secs(3),
            receipt.read(remaining, false, DescriptorGate::global()),
        )
        .await??
        .bytes;
        if count == 0 {
            return Err("unexpected endpoint EOF".into());
        }
        offset += count;
    }
    if bytes != expected {
        return Err("literal request/response changed".into());
    }
    Ok(())
}
#[tokio::test]
#[ignore = "compiled consumer entrypoint actually invoked by permanent parent scenarios"]
async fn keeper_listener_fixture() -> TestResult {
    let gate = DescriptorGate::global();
    let (spec, expected_kind, later): (DescriptorSpec, ListenerKind, bool) =
        serde_json::from_str(&std::env::var("KEEPER_EXPECTED")?)?;
    let mut receiver = GrantReceiver::new(OwnedSocket::inherit_stdin(gate).await?);
    let mut expected_descriptors = vec![DescriptorSpec::PipeRead, DescriptorSpec::PipeWrite];
    let expected_bootstrap_listeners = if later {
        Vec::new()
    } else {
        vec![expected_kind.clone()]
    };
    if !later {
        expected_descriptors.push(spec.clone());
    }
    let expected_bootstrap =
        ChildGrantExpectation::bootstrap(expected_bootstrap_listeners, expected_descriptors)?;
    let bootstrap = receiver.receive(&expected_bootstrap, gate).await?;
    let (_, descriptors) = bootstrap.into_parts();
    let mut rights = descriptors.into_iter();
    let commands =
        PipeReader::from_owned(rights.next().ok_or("command-read absent")?, gate).await?;
    let events = PipeWriter::from_owned(rights.next().ok_or("event-write absent")?, gate).await?;
    events.write_all(b"boot").await?;
    let descriptor = if later {
        let expected_later =
            ChildGrantExpectation::listener_grant(vec![expected_kind], vec![spec.clone()])?;
        receiver
            .receive(&expected_later, gate)
            .await?
            .into_parts()
            .1
            .into_iter()
            .next()
            .ok_or("later listener absent")?
    } else {
        rights.next().ok_or("bootstrap listener absent")?
    };
    if !rustix::io::fcntl_getfd(&descriptor)?.contains(rustix::io::FdFlags::CLOEXEC) {
        return Err("received listener lacks CLOEXEC".into());
    }
    let listener = match &spec {
        DescriptorSpec::UnixListener { path } => {
            OwnedListener::from_unix_owned(descriptor, path, gate).await?
        }
        DescriptorSpec::TcpListener { address } => {
            OwnedListener::from_tcp_owned(descriptor, *address, gate).await?
        }
        _ => return Err("fixture expected listener".into()),
    };
    let mut command = [0; 6];
    commands.read_exact(&mut command).await?;
    if &command != b"accept" {
        return Err("wrong accept admission bytes".into());
    }
    let connection = timeout(Duration::from_secs(3), listener.accept(gate)).await??;
    let read = connection.duplicate(gate).await?;
    receive_exact(read, REQUEST).await?;
    connection.into_writer().write_all(REPLY).await?;
    events.write_all(b"done").await?;
    drop(listener);
    println!("KEEPER_CHILD_ACCEPT_REPLY_CLOSED");
    Ok(())
}
async fn connect(
    spec: &DescriptorSpec,
) -> Result<OwnedSocket, Box<dyn std::error::Error + Send + Sync>> {
    let gate = DescriptorGate::global();
    Ok(match spec {
        DescriptorSpec::UnixListener { path } => OwnedSocket::connect_unix(path, gate).await?,
        DescriptorSpec::TcpListener { address } => OwnedSocket::connect_tcp(*address, gate).await?,
        _ => return Err("not a listener address".into()),
    })
}
async fn child_accept(
    registry: &ListenerRegistry,
    kind: &ListenerKind,
    later: bool,
    queued: Option<OwnedSocket>,
) -> TestResult {
    let gate = DescriptorGate::global();
    let grant = registry.duplicate(kind).await?;
    let spec = grant.descriptor_spec();
    let (kind, _, listener) = grant.into_parts();
    let (read_command, commands) = OwnedPipe::pair(gate).await?;
    let (events, write_event) = OwnedPipe::pair(gate).await?;
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "keeper_listener_fixture",
            "--nocapture",
        ])
        .env(
            "KEEPER_EXPECTED",
            serde_json::to_string(&(spec.clone(), kind.clone(), later))?,
        )
        .stdin(Stdio::from(receive.into_owned()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = gate.spawn_child(&mut command).await?;
    let mut sender = GrantSender::new(send);
    let mut rights = vec![read_command.into_owned(), write_event.into_owned()];
    let mut specs = vec![DescriptorSpec::PipeRead, DescriptorSpec::PipeWrite];
    let mut kinds = Vec::new();
    let mut later_right = Some(listener);
    if !later {
        rights.push(later_right.take().ok_or("initial listener absent")?);
        specs.push(spec.clone());
        kinds.push(kind.clone());
    }
    let launch = serde_json::from_str::<ChildLaunchContext>(CHILD_LAUNCH_LITERAL)?;
    let frame = ChildGrantFrame::bootstrap(launch, kinds.clone())?;
    sender.send(&frame, &specs, &rights).await?;
    drop(rights);
    let mut boot = [0; 4];
    timeout(Duration::from_secs(3), events.read_exact(&mut boot)).await??;
    if &boot != b"boot" {
        return Err("bootstrap was not validated before later grant".into());
    }
    if later {
        let frame = ChildGrantFrame::listener_grant(vec![kind])?;
        let listener_specs = vec![spec.clone()];
        sender
            .send(
                &frame,
                &listener_specs,
                &[later_right.take().ok_or("later listener absent")?],
            )
            .await?;
    }
    let peer = match queued {
        Some(peer) => peer,
        None => {
            let peer = timeout(Duration::from_secs(3), connect(&spec)).await??;
            peer.send(REQUEST, &[]).await?;
            peer
        }
    };
    commands.write_all(b"accept").await?;
    receive_exact(peer, REPLY).await?;
    let mut done = [0; 4];
    timeout(Duration::from_secs(3), events.read_exact(&mut done)).await??;
    if &done != b"done" {
        return Err("child did not finish literal response".into());
    }
    let output = timeout(Duration::from_secs(3), child.wait_with_output()).await??;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout).contains("KEEPER_CHILD_ACCEPT_REPLY_CLOSED")
    {
        return Err(format!(
            "child accept fixture failed: {:?} {} {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    drop(sender);
    drop(commands);
    Ok(())
}
async fn continuity(tcp: bool) -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let mut registry = ListenerRegistry::new(
        SingletonAuthority::acquire(&directory.path().join("host.lock")).await?,
    );
    let (kind, address) = if tcp {
        (
            ListenerKind::ProxyHttp,
            ListenerAddress::tcp("127.0.0.1:0".parse()?)?,
        )
    } else {
        (
            ListenerKind::NativeRelay,
            ListenerAddress::unix(directory.path().join("native.sock"))?,
        )
    };
    let actual = registry.bind(kind.clone(), address.clone()).await?;
    let spec = actual.descriptor_spec();
    child_accept(&registry, &kind, false, None).await?;
    // There is no child acceptor now. The retained original queues this actual request.
    let gap_started = std::time::Instant::now();
    let queued = timeout(Duration::from_secs(3), connect(&spec)).await??;
    queued.send(REQUEST, &[]).await?;
    if (registry.bind(kind.clone(), address).await?) != (actual) {
        return Err("listener_continuity scenario assertion failed".into());
    }
    child_accept(&registry, &kind, true, Some(queued)).await?;
    let gap = gap_started.elapsed();
    println!(
        "keeper fixture replacement first-reply/reap tcp={tcp} elapsed_ms={}",
        gap.as_millis()
    );
    if gap > Duration::from_secs(1) {
        return Err("fixture replacement first response exceeded 1 s".into());
    }
    // A third real recipient establishes that both child closures kept the original alive.
    child_accept(&registry, &kind, false, None).await?;
    Ok(())
}
#[tokio::test]
async fn unix_original_queues_request_between_reaped_children_then_accepts_first_reply()
-> TestResult {
    continuity(false).await
}
#[tokio::test]
async fn tcp_original_queues_request_between_reaped_children_then_accepts_first_reply() -> TestResult
{
    continuity(true).await
}

#[tokio::test]
async fn registry_consumer_refuses_malformed_associations_before_accept_effect() -> TestResult {
    use std::os::fd::AsFd;
    let gate = DescriptorGate::global();
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let mut registry = ListenerRegistry::new(
        SingletonAuthority::acquire(&directory.path().join("host.lock")).await?,
    );
    let actual = registry
        .bind(
            ListenerKind::ProxyHttp,
            ListenerAddress::tcp("127.0.0.1:0".parse()?)?,
        )
        .await?;
    let spec = actual.descriptor_spec();
    for case in 0..10 {
        let (command_read, command_write) = OwnedPipe::pair(gate).await?;
        let (event_read, event_write) = OwnedPipe::pair(gate).await?;
        let (send, receive) = OwnedSocket::pair(gate).await?;
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command
            .args([
                "--ignored",
                "--exact",
                "keeper_listener_fixture",
                "--nocapture",
            ])
            .env(
                "KEEPER_EXPECTED",
                serde_json::to_string(&(spec.clone(), ListenerKind::ProxyHttp, false))?,
            )
            .stdin(Stdio::from(receive.into_owned()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = gate.spawn_child(&mut command).await?;
        let (_, _, listener) = registry
            .duplicate(&ListenerKind::ProxyHttp)
            .await?
            .into_parts();
        let mut rights = vec![
            command_read.into_owned(),
            event_write.into_owned(),
            listener,
        ];
        let mut frame_literal = BOOTSTRAP_PROXY_HTTP_LITERAL;
        let mut held_socket = None;
        let mut held_listener = None;
        match case {
            0 => {
                *rights.last_mut().ok_or("last right absent")? =
                    std::fs::File::open("/dev/null")?.into()
            }
            1 => {
                *rights.last_mut().ok_or("last right absent")? =
                    OwnedPipe::pair(gate).await?.1.into_owned()
            }
            2 => {
                let _shared = gate.creation().await;
                let fd = rustix::net::socket(
                    rustix::net::AddressFamily::INET,
                    rustix::net::SocketType::DGRAM,
                    None,
                )?;
                rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)?;
                *rights.last_mut().ok_or("last right absent")? = fd;
            }
            3 => {
                let unix =
                    OwnedListener::bind_unix(&directory.path().join("wrong-family.sock"), gate)
                        .await?;
                *rights.last_mut().ok_or("last right absent")? =
                    gate.duplicate(unix.as_fd()).await?;
                held_listener = Some(unix);
            }
            4 => {
                let other = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
                *rights.last_mut().ok_or("last right absent")? =
                    gate.duplicate(other.as_fd()).await?;
                held_listener = Some(other);
            }
            5 => {
                let (peer, accepted) = tokio::join!(connect(&spec), async {
                    let (_, _, fd) = registry
                        .duplicate(&ListenerKind::ProxyHttp)
                        .await?
                        .into_parts();
                    let address = match &spec {
                        DescriptorSpec::TcpListener { address } => *address,
                        _ => return Err("TCP spec absent".into()),
                    };
                    let listener = OwnedListener::from_tcp_owned(fd, address, gate).await?;
                    Ok::<_, Box<dyn std::error::Error + Send + Sync>>(listener.accept(gate).await?)
                });
                held_socket = Some(peer?);
                *rights.last_mut().ok_or("last right absent")? = accepted?.into_owned();
            }
            6 => {
                rights.pop();
            }
            7 => {
                frame_literal = LISTENER_GRANT_PROXY_HTTP_LITERAL;
            }
            8 => {
                rights.swap(0, 1);
            }
            _ => {
                frame_literal = BOOTSTRAP_NATIVE_RELAY_LITERAL;
            }
        }
        let mut bytes = u32::try_from(frame_literal.len())?.to_be_bytes().to_vec();
        bytes.extend_from_slice(frame_literal.as_bytes());
        let borrowed: Vec<_> = rights.iter().map(AsFd::as_fd).collect();
        if send.send(&bytes, &borrowed).await? != bytes.len() {
            return Err("bounded malicious fixture shortsend".into());
        }
        let output = timeout(Duration::from_secs(3), child.wait_with_output()).await??;
        if output.status.code() != Some(70)
            || String::from_utf8_lossy(&output.stdout).contains("KEEPER_CHILD_ACCEPT_REPLY_CLOSED")
        {
            return Err(format!(
                "malformed registry grant case{case} reached effect: {:?} {} {}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        drop(send);
        drop(rights);
        drop(command_write);
        drop(event_read);
        drop(held_socket);
        drop(held_listener);
    }
    // All refused recipients died; the keeper's original still serves a correct child.
    child_accept(&registry, &ListenerKind::ProxyHttp, false, None).await?;
    Ok(())
}
