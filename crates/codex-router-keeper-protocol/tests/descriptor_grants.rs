use codex_router_descriptor_boundary::{
    DescriptorGate, OwnedListener, OwnedPipe, OwnedSocket, PipeReader, PipeWriter,
};
use codex_router_keeper_protocol::{
    ChildGrantExpectation, ChildGrantFrame, ChildLaunchContext, DescriptorSpec, GrantReceiver,
    GrantSender, ListenerKind,
};
use collaboration_protocol::EndpointId;
use std::{os::fd::AsFd, process::Stdio};
use tokio::time::{Duration, timeout};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const TYPED_CHILD_BOOTSTRAP_LITERAL: &str = r#"{"type":"bootstrap","launch":{"role":"agentProxyServices","image":{"retainedPath":"/fixture/codex-router","fileSha256":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"device":1,"inode":2},"fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"},"listeners":[{"kind":"proxyHttp"}]}"#;
const CHILD_LAUNCH_LITERAL: &str = r#"{"role":"agentProxyServices","image":{"retainedPath":"/fixture/codex-router","fileSha256":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"device":1,"inode":2},"fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
const BOOTSTRAP_EMPTY_LISTENERS_LITERAL: &str = r#"{"type":"bootstrap","launch":{"role":"agentProxyServices","image":{"retainedPath":"/fixture/codex-router","fileSha256":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"device":1,"inode":2},"fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"},"listeners":[]}"#;
const LISTENER_GRANT_PROXY_HTTP_LITERAL: &str =
    r#"{"type":"listenerGrant","listeners":[{"kind":"proxyHttp"}]}"#;
const LEGACY_GENERIC_ENVELOPE_LITERAL: &str =
    r#"{"phase":"listenerGrant","descriptors":[],"context":null}"#;

#[tokio::test]
async fn literal_typed_bootstrap_transfers_control_and_event_pipes() -> TestResult {
    let gate = DescriptorGate::global();
    let (command_read, command_write) = OwnedPipe::pair(gate).await?;
    let (event_read, event_write) = OwnedPipe::pair(gate).await?;
    let listener = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
    let address = listener.tcp_address()?;
    let specs = vec![
        DescriptorSpec::PipeRead,
        DescriptorSpec::PipeWrite,
        DescriptorSpec::TcpListener { address },
    ];
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let expected_frame = serde_json::from_str::<ChildGrantFrame>(TYPED_CHILD_BOOTSTRAP_LITERAL)?;
    let child = fixture(receive, expected_frame, &specs, "bootstrap").await?;
    let mut bytes = u32::try_from(TYPED_CHILD_BOOTSTRAP_LITERAL.len())?
        .to_be_bytes()
        .to_vec();
    bytes.extend_from_slice(TYPED_CHILD_BOOTSTRAP_LITERAL.as_bytes());
    let rights = [
        command_read.into_owned(),
        event_write.into_owned(),
        gate.duplicate(listener.as_fd()).await?,
    ];
    let borrowed: Vec<_> = rights.iter().map(AsFd::as_fd).collect();
    if send.send(&bytes, &borrowed).await? != bytes.len() {
        return Err("literal typed bootstrap shortsend".into());
    }
    drop(rights);
    if let Err(error) = command_write.write_all(b"control").await {
        let output = timeout(Duration::from_secs(3), child.wait_with_output()).await??;
        return Err(format!(
            "typed bootstrap was refused before control-pipe effect: {error}; child status {:?}, stdout {}, stderr {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let mut event = [0; 5];
    timeout(Duration::from_secs(2), event_read.read_exact(&mut event)).await??;
    if &event != b"event" {
        return Err("literal typed bootstrap event direction failed".into());
    }
    normal_exit(child).await
}

async fn fixture(
    socket: OwnedSocket,
    expected_frame: ChildGrantFrame,
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
        .env(
            "GRANT_EXPECTED",
            serde_json::to_string(&(expected_frame, specs))?,
        )
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
    let (expected_frame, specs): (ChildGrantFrame, Vec<DescriptorSpec>) =
        serde_json::from_str(&std::env::var("GRANT_EXPECTED")?)?;
    let expectation = match &expected_frame {
        ChildGrantFrame::Bootstrap { listeners, .. } => {
            ChildGrantExpectation::bootstrap(listeners.clone(), specs)?
        }
        ChildGrantFrame::ListenerGrant { listeners } => {
            ChildGrantExpectation::listener_grant(listeners.clone(), specs)?
        }
    };
    let mut receiver = GrantReceiver::new(OwnedSocket::inherit_stdin(gate).await?);
    let (_, descriptors) = receiver.receive(&expectation, gate).await?.into_parts();
    for fd in &descriptors {
        if !rustix::io::fcntl_getfd(fd)?.contains(rustix::io::FdFlags::CLOEXEC) {
            return Err("grant fd lacks CLOEXEC".into());
        }
    }
    match std::env::var("GRANT_MODE")?.as_str() {
        "bootstrap" => {
            let mut rights = descriptors.into_iter();
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
            let fd = descriptors
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
            let fd = descriptors.into_iter().next().ok_or("listener absent")?;
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
    let launch = serde_json::from_str::<ChildLaunchContext>(CHILD_LAUNCH_LITERAL)?;
    let frame = ChildGrantFrame::bootstrap(launch, Vec::new())?;
    let child = fixture(receive, frame.clone(), &specs, "bootstrap").await?;
    let mut sender = GrantSender::new(send);
    sender
        .send(
            &frame,
            &specs,
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
        let listeners = vec![ListenerKind::ProxyHttp; count];
        let frame = ChildGrantFrame::listener_grant(listeners)?;
        let child = fixture(receive, frame.clone(), &specs, "validate").await?;
        let mut rights = Vec::new();
        for _ in 0..count {
            rights.push(gate.duplicate(listener.as_fd()).await?);
        }
        let mut sender = GrantSender::new(send);
        sender.send(&frame, &specs, &rights).await?;
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
    let frame = ChildGrantFrame::listener_grant(vec![ListenerKind::ProxyHttp])?;
    let child = fixture(receive, frame.clone(), &specs, "accept").await?;
    let duplicate = gate.duplicate(listener.as_fd()).await?;
    let mut sender = GrantSender::new(send);
    sender.send(&frame, &specs, &[duplicate]).await?;
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
    let specs = vec![DescriptorSpec::TcpListener { address }; 65];
    let frame = ChildGrantFrame::ListenerGrant {
        listeners: vec![ListenerKind::ProxyHttp; 65],
    };
    if !matches!(
        sender.send(&frame, &specs, &rights).await,
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
async fn old_generic_envelope_is_refused_before_recipient_effect() -> TestResult {
    let gate = DescriptorGate::global();
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let expected_frame = ChildGrantFrame::listener_grant(Vec::new())?;
    let child = fixture(receive, expected_frame, &[], "validate").await?;
    let mut bytes = u32::try_from(LEGACY_GENERIC_ENVELOPE_LITERAL.len())?
        .to_be_bytes()
        .to_vec();
    bytes.extend_from_slice(LEGACY_GENERIC_ENVELOPE_LITERAL.as_bytes());
    if send.send(&bytes, &[]).await? != bytes.len() {
        return Err("legacy envelope fixture shortsend".into());
    }
    let output = timeout(Duration::from_secs(3), child.wait_with_output()).await??;
    if output.status.code() != Some(70)
        || String::from_utf8_lossy(&output.stdout).contains("GRANT_EFFECT_VALIDATED")
    {
        return Err(format!(
            "legacy generic envelope reached recipient effect: {:?} {} {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

#[tokio::test]
async fn wrong_kind_count_frame_variant_direction_and_address_kill_receiver_before_effect()
-> TestResult {
    let gate = DescriptorGate::global();
    let listener = OwnedListener::bind_tcp("127.0.0.1:0".parse()?, gate).await?;
    let address = listener.tcp_address()?;
    for case in 0..5 {
        let (send, receive) = OwnedSocket::pair(gate).await?;
        let expected_frame = ChildGrantFrame::listener_grant(vec![ListenerKind::ProxyHttp])?;
        let expected_descriptors = if case == 4 {
            vec![DescriptorSpec::TcpListener {
                address: "127.0.0.1:1".parse()?,
            }]
        } else {
            vec![DescriptorSpec::TcpListener { address }]
        };
        let child = fixture(receive, expected_frame, &expected_descriptors, "validate").await?;
        let rights = match case {
            0 => vec![std::fs::File::open("/dev/null")?.into()],
            1 => Vec::new(),
            2 => vec![gate.duplicate(listener.as_fd()).await?],
            3 => vec![OwnedPipe::pair(gate).await?.1.into_owned()],
            _ => {
                vec![gate.duplicate(listener.as_fd()).await?]
            }
        };
        let frame_literal = if case == 2 {
            BOOTSTRAP_EMPTY_LISTENERS_LITERAL
        } else {
            LISTENER_GRANT_PROXY_HTTP_LITERAL
        };
        let mut bytes = u32::try_from(frame_literal.len())?.to_be_bytes().to_vec();
        bytes.extend_from_slice(frame_literal.as_bytes());
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
    let frame = ChildGrantFrame::listener_grant(vec![ListenerKind::NativeRelay])?;
    let child = fixture(receive, frame.clone(), &specs, "unix-accept").await?;
    let duplicate = gate.duplicate(listener.as_fd()).await?;
    let mut sender = GrantSender::new(send);
    sender.send(&frame, &specs, &[duplicate]).await?;
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
    let expected_frame =
        serde_json::from_str::<ChildGrantFrame>(BOOTSTRAP_EMPTY_LISTENERS_LITERAL)?;
    let child = fixture(receive, expected_frame, &specs, "bootstrap").await?;
    let rights = [write.into_owned(), read.into_owned()];
    let mut bytes = u32::try_from(BOOTSTRAP_EMPTY_LISTENERS_LITERAL.len())?
        .to_be_bytes()
        .to_vec();
    bytes.extend_from_slice(BOOTSTRAP_EMPTY_LISTENERS_LITERAL.as_bytes());
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
    let expected_frame = ChildGrantFrame::listener_grant(vec![ListenerKind::ProxyHttp])?;
    let child = fixture(receive, expected_frame, &specs, "validate").await?;
    let mut bytes = u32::try_from(LISTENER_GRANT_PROXY_HTTP_LITERAL.len())?
        .to_be_bytes()
        .to_vec();
    bytes.extend_from_slice(LISTENER_GRANT_PROXY_HTTP_LITERAL.as_bytes());
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
    let endpoint = EndpointId::try_from("a".repeat(64))?;
    let listeners = vec![ListenerKind::RouterSessionFace { endpoint }; 64];
    let frame = ChildGrantFrame::listener_grant(listeners)?;
    let frame_bytes = serde_json::to_vec(&frame)?;
    if frame_bytes.len() <= 1024 {
        return Err("typed maximum-size grant did not exceed the test send buffer".into());
    }
    let specs = vec![DescriptorSpec::TcpListener { address }; 64];
    let (send, receive) = OwnedSocket::pair(gate).await?;
    rustix::net::sockopt::set_socket_send_buffer_size(send.as_fd(), 1024)?;
    let child = fixture(receive, frame.clone(), &specs, "validate").await?;
    let mut rights = Vec::new();
    for _ in 0..64 {
        rights.push(gate.duplicate(listener.as_fd()).await?);
    }
    let mut sender = GrantSender::new(send);
    sender.send(&frame, &specs, &rights).await?;
    normal_exit(child).await?;
    // Independent sender attaches rights to byte 1 instead of the first prefix byte.
    let (send, receive) = OwnedSocket::pair(gate).await?;
    let expected_frame = ChildGrantFrame::listener_grant(vec![ListenerKind::ProxyHttp])?;
    let expected_specs = vec![DescriptorSpec::TcpListener { address }];
    let child = fixture(receive, expected_frame, &expected_specs, "validate").await?;
    let prefix = u32::try_from(LISTENER_GRANT_PROXY_HTTP_LITERAL.len())?.to_be_bytes();
    send.send(prefix.get(..1).ok_or("prefix absent")?, &[])
        .await?;
    let mut rest = prefix.get(1..).ok_or("prefix absent")?.to_vec();
    rest.extend_from_slice(LISTENER_GRANT_PROXY_HTTP_LITERAL.as_bytes());
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
        let expected_frame = ChildGrantFrame::listener_grant(Vec::new())?;
        let child = fixture(receive, expected_frame, &[], "validate").await?;
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
