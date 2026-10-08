use codex_router_descriptor_boundary::{
    DescriptorGate, OwnedListener, OwnedPipe, OwnedSocket, PipeReader, PipeWriter,
};
use codex_router_keeper_protocol::{
    DescriptorSpec, GrantEnvelope, GrantPhase, GrantReceiver, GrantSender, JsonMessage,
};
use std::{os::fd::AsFd, process::Stdio};
use tokio::time::{Duration, timeout};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
async fn fixture(
    socket: OwnedSocket,
    phase: GrantPhase,
    specs: &[DescriptorSpec],
    mode: &str,
) -> Result<tokio::process::Child, Box<dyn std::error::Error + Send + Sync>> {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "grant_receiver_fixture",
            "--nocapture",
        ])
        .env("GRANT_EXPECTED", serde_json::to_string(&(phase, specs))?)
        .env("GRANT_MODE", mode)
        .stdin(Stdio::from(socket.into_owned()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(DescriptorGate::global().spawn_child(&mut command).await?)
}
#[tokio::test]
#[ignore = "grant fixture executed by permanent descriptor scenarios"]
async fn grant_receiver_fixture() -> TestResult {
    let gate = DescriptorGate::global();
    let (phase, specs): (GrantPhase, Vec<DescriptorSpec>) =
        serde_json::from_str(&std::env::var("GRANT_EXPECTED")?)?;
    let mut receiver = GrantReceiver::new(OwnedSocket::inherit_stdin(gate).await?);
    let grant = receiver
        .receive::<serde_json::Value>(phase, &specs, gate)
        .await?;
    for fd in &grant.descriptors {
        if !rustix::io::fcntl_getfd(fd)?.contains(rustix::io::FdFlags::CLOEXEC) {
            return Err("grant fd lacks CLOEXEC".into());
        }
    }
    match std::env::var("GRANT_MODE")?.as_str() {
        "bootstrap" => {
            let mut rights = grant.descriptors.into_iter();
            let read =
                PipeReader::from_owned(rights.next().ok_or("read pipe absent")?, gate).await?;
            let write =
                PipeWriter::from_owned(rights.next().ok_or("write pipe absent")?, gate).await?;
            let mut bytes = [0; 7];
            read.read_exact(&mut bytes).await?;
            if &bytes != b"control" {
                return Err("bootstrap changed control bytes".into());
            }
            write.write_all(b"event").await?;
        }
        "unix-accept" => {
            let fd = grant
                .descriptors
                .into_iter()
                .next()
                .ok_or("Unix listener absent")?;
            let listener = std::os::unix::net::UnixListener::from(fd);
            listener.set_nonblocking(true)?;
            let listener = tokio::net::UnixListener::from_std(listener)?;
            let (mut connection, _) = timeout(Duration::from_secs(2), listener.accept()).await??;
            use tokio::io::AsyncWriteExt;
            connection.write_all(b"Unix grant reply").await?;
        }
        "accept" => {
            let fd = grant
                .descriptors
                .into_iter()
                .next()
                .ok_or("listener absent")?;
            let std_listener = std::net::TcpListener::from(fd);
            std_listener.set_nonblocking(true)?;
            let listener = tokio::net::TcpListener::from_std(std_listener)?;
            let (mut connection, _) = timeout(Duration::from_secs(2), listener.accept()).await??;
            use tokio::io::AsyncWriteExt;
            connection.write_all(b"grant reply").await?;
        }
        _ => {}
    }
    println!("GRANT_EFFECT_VALIDATED");
    Ok(())
}
async fn normal_exit(child: tokio::process::Child) -> TestResult {
    let output = timeout(Duration::from_secs(3), child.wait_with_output()).await??;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout).contains("GRANT_EFFECT_VALIDATED")
    {
        return Err(format!(
            "valid grant failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}
#[tokio::test]
async fn real_bootstrap_pipe_directions_preserve_control_and_events() -> TestResult {
    let gate = DescriptorGate::global();
    let (command_read, command_write) = OwnedPipe::pair(gate).await?;
    let (event_read, event_write) = OwnedPipe::pair(gate).await?;
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let specs = vec![DescriptorSpec::PipeRead, DescriptorSpec::PipeWrite];
    let child = fixture(receive, GrantPhase::Bootstrap, &specs, "bootstrap").await?;
    let mut sender = GrantSender::new(send);
    sender
        .send(
            &GrantEnvelope {
                phase: GrantPhase::Bootstrap,
                descriptors: specs,
                context: serde_json::json!({"capture":"test"}),
            },
            &[command_read.into_owned(), event_write.into_owned()],
        )
        .await?;
    command_write.write_all(b"control").await?;
    let mut event = [0; 5];
    timeout(Duration::from_secs(2), event_read.read_exact(&mut event)).await??;
    if &event != b"event" {
        return Err("bootstrap event direction failed".into());
    }
    normal_exit(child).await
}
#[tokio::test]
async fn zero_one_and_64_valid_grants_are_associated_and_cloexec() -> TestResult {
    let gate = DescriptorGate::global();
    let listener = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
    let address = listener.tcp_address()?;
    for count in [0, 1, 64] {
        let (send, receive) = OwnedSocket::pair(gate).await?;
        let specs = vec![DescriptorSpec::TcpListener { address }; count];
        let child = fixture(receive, GrantPhase::ListenerGrant, &specs, "validate").await?;
        let mut rights = Vec::new();
        for _ in 0..count {
            rights.push(gate.duplicate(listener.as_fd()).await?);
        }
        let mut sender = GrantSender::new(send);
        sender
            .send(
                &GrantEnvelope {
                    phase: GrantPhase::ListenerGrant,
                    descriptors: specs,
                    context: serde_json::json!({"count":count}),
                },
                &rights,
            )
            .await?;
        normal_exit(child)
            .await
            .map_err(|error| format!("grant count {count}: {error}"))?;
    }
    Ok(())
}
#[tokio::test]
async fn listened_tcp_grant_child_response_and_parent_original_survive_close() -> TestResult {
    use tokio::io::AsyncReadExt;
    let gate = DescriptorGate::global();
    let listener = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
    let address = listener.tcp_address()?;
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let specs = vec![DescriptorSpec::TcpListener { address }];
    let child = fixture(receive, GrantPhase::ListenerGrant, &specs, "accept").await?;
    let duplicate = gate.duplicate(listener.as_fd()).await?;
    let mut sender = GrantSender::new(send);
    sender
        .send(
            &GrantEnvelope {
                phase: GrantPhase::ListenerGrant,
                descriptors: specs,
                context: serde_json::json!({}),
            },
            &[duplicate],
        )
        .await?;
    let mut client = tokio::net::TcpStream::connect(address).await?;
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(3), client.read_to_end(&mut bytes)).await??;
    if bytes != b"grant reply" {
        return Err("actual child grant accept failed".into());
    }
    normal_exit(child).await?;
    let peer = OwnedSocket::connect_tcp(address, gate).await?;
    let accepted = listener.accept(gate).await?;
    accepted.into_writer().write_all(b"parent").await?;
    let mut bytes = [0; 6];
    let receipt = codex_router_descriptor_boundary::UnixReceipt::new(peer);
    if timeout(
        Duration::from_secs(2),
        receipt.read(&mut bytes, false, gate),
    )
    .await??
    .bytes
        != 6
        || &bytes != b"parent"
    {
        return Err("parent listener original failed".into());
    }
    Ok(())
}
#[tokio::test]
async fn outgoing_65_refusal_has_no_socket_send() -> TestResult {
    let gate = DescriptorGate::global();
    let listener = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
    let address = listener.tcp_address()?;
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let mut rights = Vec::new();
    for _ in 0..65 {
        rights.push(gate.duplicate(listener.as_fd()).await?);
    }
    let mut sender = GrantSender::new(send);
    let envelope = GrantEnvelope {
        phase: GrantPhase::ListenerGrant,
        descriptors: vec![DescriptorSpec::TcpListener { address }; 65],
        context: (),
    };
    if !matches!(
        sender.send(&envelope, &rights).await,
        Err(codex_router_descriptor_boundary::BoundaryError::TooLarge)
    ) {
        return Err("outgoing65 was not refused".into());
    }
    let receipt = codex_router_descriptor_boundary::UnixReceipt::new(receive);
    if timeout(
        Duration::from_millis(20),
        receipt.read(&mut [0], false, gate),
    )
    .await
    .is_ok()
    {
        return Err("refused grant emitted bytes".into());
    }
    Ok(())
}
#[tokio::test]
async fn wrong_kind_count_phase_direction_and_address_kill_receiver_before_effect() -> TestResult {
    let gate = DescriptorGate::global();
    let listener = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
    let address = listener.tcp_address()?;
    for case in 0..5 {
        let (send, receive) = OwnedSocket::pair(gate).await?;
        let expected = vec![DescriptorSpec::TcpListener { address }];
        let child = fixture(receive, GrantPhase::ListenerGrant, &expected, "validate").await?;
        let mut specs = expected.clone();
        let mut phase = GrantPhase::ListenerGrant;
        let rights = match case {
            0 => vec![std::fs::File::open("/dev/null")?.into()],
            1 => Vec::new(),
            2 => {
                phase = GrantPhase::Bootstrap;
                vec![gate.duplicate(listener.as_fd()).await?]
            }
            3 => {
                specs = vec![DescriptorSpec::PipeRead];
                vec![OwnedPipe::pair(gate).await?.1.into_owned()]
            }
            _ => {
                specs = vec![DescriptorSpec::TcpListener {
                    address: "127.0.0.1:1".parse()?,
                }];
                vec![gate.duplicate(listener.as_fd()).await?]
            }
        };
        let message = JsonMessage::encode(&GrantEnvelope {
            phase,
            descriptors: specs,
            context: (),
        })?;
        let mut bytes = u32::try_from(message.bytes().len())?.to_be_bytes().to_vec();
        bytes.extend(message.bytes());
        let borrowed: Vec<_> = rights.iter().map(AsFd::as_fd).collect();
        if send.send(&bytes, &borrowed).await? != bytes.len() {
            return Err("malicious short senderfixture".into());
        }
        let output = timeout(Duration::from_secs(3), child.wait_with_output()).await??;
        if output.status.code() != Some(70)
            || String::from_utf8_lossy(&output.stdout).contains("GRANT_EFFECT_VALIDATED")
        {
            return Err(format!(
                "malformed grant did not die beforeeffect case{case}: {:?}",
                output.status
            )
            .into());
        }
        drop(send);
        drop(rights);
    }
    Ok(())
}

#[tokio::test]
async fn listened_unix_grant_retains_exact_path_and_parent_listener() -> TestResult {
    use tokio::io::AsyncReadExt;
    let gate = DescriptorGate::global();
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("endpoint.sock");
    let listener = OwnedListener::bind_unix(&path, gate).await?;
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let specs = vec![DescriptorSpec::UnixListener { path: path.clone() }];
    let child = fixture(receive, GrantPhase::ListenerGrant, &specs, "unix-accept").await?;
    let duplicate = gate.duplicate(listener.as_fd()).await?;
    let mut sender = GrantSender::new(send);
    sender
        .send(
            &GrantEnvelope {
                phase: GrantPhase::ListenerGrant,
                descriptors: specs,
                context: (),
            },
            &[duplicate],
        )
        .await?;
    let mut peer = tokio::net::UnixStream::connect(&path).await?;
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(2), peer.read_to_end(&mut bytes)).await??;
    if bytes != b"Unix grant reply" {
        return Err("Unix grant address/accept mismatched".into());
    }
    normal_exit(child).await?;
    let (connected, accepted) = tokio::join!(
        OwnedSocket::connect_unix(&path, gate),
        listener.accept(gate)
    );
    accepted?.into_writer().write_all(b"original").await?;
    let mut bytes = [0; 8];
    let receipt = codex_router_descriptor_boundary::UnixReceipt::new(connected?);
    if timeout(
        Duration::from_secs(1),
        receipt.read(&mut bytes, false, gate),
    )
    .await??
    .bytes
        != 8
        || &bytes != b"original"
    {
        return Err("recipient close removed Unix original".into());
    }
    Ok(())
}
#[tokio::test]
async fn bootstrap_swapped_pipe_directions_are_fatal_before_use() -> TestResult {
    let gate = DescriptorGate::global();
    let (read, write) = OwnedPipe::pair(gate).await?;
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let specs = vec![DescriptorSpec::PipeRead, DescriptorSpec::PipeWrite];
    let child = fixture(receive, GrantPhase::Bootstrap, &specs, "bootstrap").await?;
    let rights = [write.into_owned(), read.into_owned()];
    let message = JsonMessage::encode(&GrantEnvelope {
        phase: GrantPhase::Bootstrap,
        descriptors: specs,
        context: (),
    })?;
    let mut bytes = u32::try_from(message.bytes().len())?.to_be_bytes().to_vec();
    bytes.extend(message.bytes());
    let rights: Vec<_> = rights.iter().map(AsFd::as_fd).collect();
    if send.send(&bytes, &rights).await? != bytes.len() {
        return Err("small swapped grant shortsend".into());
    }
    let output = timeout(Duration::from_secs(2), child.wait_with_output()).await??;
    if output.status.code() != Some(70)
        || String::from_utf8_lossy(&output.stdout).contains("GRANT_EFFECT_VALIDATED")
    {
        return Err("wrong-direction pipe reached effects".into());
    }
    Ok(())
}
#[tokio::test]
async fn connected_stream_is_refused_even_at_expected_listener_address() -> TestResult {
    let gate = DescriptorGate::global();
    let listener = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
    let address = listener.tcp_address()?;
    let (connected, accepted) = tokio::join!(
        OwnedSocket::connect_tcp(address, gate),
        listener.accept(gate)
    );
    let _peer = connected?;
    let accepted = accepted?;
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let specs = vec![DescriptorSpec::TcpListener { address }];
    let child = fixture(receive, GrantPhase::ListenerGrant, &specs, "validate").await?;
    let message = JsonMessage::encode(&GrantEnvelope {
        phase: GrantPhase::ListenerGrant,
        descriptors: specs,
        context: (),
    })?;
    let mut bytes = u32::try_from(message.bytes().len())?.to_be_bytes().to_vec();
    bytes.extend(message.bytes());
    if send.send(&bytes, &[accepted.as_fd()]).await? != bytes.len() {
        return Err("small connected grant shortsend".into());
    }
    let output = timeout(Duration::from_secs(2), child.wait_with_output()).await??;
    if output.status.code() != Some(70)
        || String::from_utf8_lossy(&output.stdout).contains("GRANT_EFFECT_VALIDATED")
    {
        return Err("connected stream passed listener association".into());
    }
    Ok(())
}

#[tokio::test]
async fn grant_large_shortwrites_and_late_rights_remain_bound_to_prefix_zero() -> TestResult {
    let gate = DescriptorGate::global();
    let listener = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
    let address = listener.tcp_address()?;
    let specs = vec![DescriptorSpec::TcpListener { address }];
    let (send, receive) = OwnedSocket::pair(gate).await?;
    rustix::net::sockopt::set_socket_send_buffer_size(send.as_fd(), 1024)?;
    let child = fixture(receive, GrantPhase::ListenerGrant, &specs, "validate").await?;
    let duplicate = gate.duplicate(listener.as_fd()).await?;
    let mut sender = GrantSender::new(send);
    let context = serde_json::json!({"large":"x".repeat(512*1024)});
    sender
        .send(
            &GrantEnvelope {
                phase: GrantPhase::ListenerGrant,
                descriptors: specs.clone(),
                context,
            },
            &[duplicate],
        )
        .await?;
    normal_exit(child).await?;
    // Independent sender attaches rights to byte 1 instead of the first prefix byte.
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let child = fixture(receive, GrantPhase::ListenerGrant, &specs, "validate").await?;
    let message = JsonMessage::encode(&GrantEnvelope {
        phase: GrantPhase::ListenerGrant,
        descriptors: specs,
        context: (),
    })?;
    let prefix = u32::try_from(message.bytes().len())?.to_be_bytes();
    send.send(prefix.get(..1).ok_or("prefix absent")?, &[])
        .await?;
    let mut rest = prefix.get(1..).ok_or("prefix absent")?.to_vec();
    rest.extend(message.bytes());
    send.send(&rest, &[listener.as_fd()]).await?;
    let output = timeout(Duration::from_secs(2), child.wait_with_output()).await??;
    if output.status.code() != Some(70)
        || String::from_utf8_lossy(&output.stdout).contains("GRANT_EFFECT_VALIDATED")
    {
        return Err("misplaced ancillary reached validated grant effect".into());
    }
    Ok(())
}

#[tokio::test]
async fn malicious_65_and_128_grant_rights_close_witness_after_child_reap_and_carrier_close()
-> TestResult {
    use std::{io::IoSlice, mem::MaybeUninit, os::unix::process::ExitStatusExt};
    let gate = DescriptorGate::global();
    for count in [65, 128] {
        let (send, receive) = OwnedSocket::pair(gate).await?;
        let child = fixture(receive, GrantPhase::ListenerGrant, &[], "validate").await?;
        let (witness, writer) = OwnedPipe::pair(gate).await?;
        let mut copies = Vec::new();
        for _ in 0..count {
            copies.push(gate.duplicate(writer.as_fd()).await?);
        }
        {
            let borrowed: Vec<_> = copies.iter().map(AsFd::as_fd).collect();
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(128))];
            let mut control = rustix::net::SendAncillaryBuffer::new(&mut space);
            if !control.push(rustix::net::SendAncillaryMessage::ScmRights(&borrowed)) {
                return Err("malicious grant sender fixture capacity failed".into());
            }
            if rustix::net::sendmsg(
                send.as_fd(),
                &[IoSlice::new(b"x")],
                &mut control,
                rustix::net::SendFlags::empty(),
            )? != 1
            {
                return Err("malicious grant did not send prefix".into());
            }
        }
        drop(copies);
        drop(writer);
        let output = timeout(Duration::from_secs(2), child.wait_with_output()).await??;
        if (output.status.code() != Some(70) && output.status.signal().is_none())
            || String::from_utf8_lossy(&output.stdout).contains("GRANT_EFFECT_VALIDATED")
        {
            return Err("overcapacity grant reached effects or did not exitfatally".into());
        }
        drop(send);
        if timeout(Duration::from_secs(2), witness.read(&mut [0])).await?? != 0 {
            return Err("malicious grant rights remain afterexit/reap/allendsclose".into());
        }
    }
    Ok(())
}
