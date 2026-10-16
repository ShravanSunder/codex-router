use collaboration_service::connect_native_relay;
use futures_util::{SinkExt, StreamExt};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn upgrades_both_transports_without_inserting_native_requests() {
    let root = std::path::PathBuf::from(format!("/tmp/native-upgrade-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|e| panic!("directory: {e}"));
    let path = root.join("backend.sock");
    let backend = UnixListener::bind(&path).unwrap_or_else(|e| panic!("bind: {e}"));
    let (caller, frontend) = UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let retirement = CancellationToken::new();
    let relay_retirement = retirement.clone();
    let relay_path = path.clone();
    let relay =
        tokio::spawn(
            async move { connect_native_relay(frontend, &relay_path, relay_retirement).await },
        );
    let backend_task = tokio::spawn(async move {
        let (stream, _) = backend
            .accept()
            .await
            .unwrap_or_else(|e| panic!("accept: {e}"));
        let mut websocket = tokio_tungstenite::accept_async(stream)
            .await
            .unwrap_or_else(|e| panic!("upgrade: {e}"));
        let frame = websocket
            .next()
            .await
            .unwrap_or_else(|| panic!("request"))
            .unwrap_or_else(|e| panic!("read: {e}"));
        assert_eq!(
            frame,
            Message::Text("{\"id\":\"proof\",\"method\":\"initialize\"}".into())
        );
        websocket
            .send(Message::Text("{\"id\":\"proof\",\"result\":{}}".into()))
            .await
            .unwrap_or_else(|e| panic!("send: {e}"));
        // Keep backend alive until generation retirement closes the connection.
        let _closed = websocket.next().await;
    });
    let (mut caller, _) = tokio::time::timeout(
        Duration::from_secs(2),
        tokio_tungstenite::client_async("ws://localhost/", caller),
    )
    .await
    .unwrap_or_else(|e| panic!("upgrade timeout: {e}"))
    .unwrap_or_else(|e| panic!("upgrade: {e}"));
    caller
        .send(Message::Text(
            "{\"id\":\"proof\",\"method\":\"initialize\"}".into(),
        ))
        .await
        .unwrap_or_else(|e| panic!("send: {e}"));
    let response = tokio::time::timeout(Duration::from_secs(2), caller.next())
        .await
        .unwrap_or_else(|e| panic!("response timeout: {e}"))
        .unwrap_or_else(|| panic!("response"))
        .unwrap_or_else(|e| panic!("response: {e}"));
    assert_eq!(
        response,
        Message::Text("{\"id\":\"proof\",\"result\":{}}".into())
    );
    retirement.cancel();
    tokio::time::timeout(Duration::from_secs(2), relay)
        .await
        .unwrap_or_else(|e| panic!("retirement: {e}"))
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("relay: {e}"));
    backend_task
        .await
        .unwrap_or_else(|e| panic!("backend join: {e}"));
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("socket cleanup: {e}"));
    std::fs::remove_dir(root).unwrap_or_else(|e| panic!("directory cleanup: {e}"));
}

/// Foundation proof for source-side attachment: an upgraded connection belongs to
/// the admitted generation, and retirement cannot reconnect it to its replacement.
#[tokio::test]
async fn native_upgrade_retains_its_admitted_generation_across_replacement() {
    use collaboration_protocol::CodexGeneration;
    use collaboration_service::{NativeGenerationGate, NativeRelayListener};
    use std::task::Poll;

    // Arrange: the real stable public listener and two distinct native backends.
    let root = tempfile::Builder::new()
        .prefix("native-pin-")
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    assert_eq!(
        root.path().metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
    let frontend_path = root.path().join("frontend.sock");
    let first_path = root.path().join("first.sock");
    let second_path = root.path().join("second.sock");
    let first_backend = UnixListener::bind(&first_path).unwrap();
    let second_backend = UnixListener::bind(&second_path).unwrap();
    let generation = |number: u64| CodexGeneration {
        service_epoch: "00000000-0000-4000-8000-000000000001"
            .to_owned()
            .try_into()
            .unwrap(),
        generation: number.try_into().unwrap(),
    };
    let gate = NativeGenerationGate::default();
    gate.activate(generation(1), first_path, None).unwrap();
    let listener = NativeRelayListener::bind(&frontend_path, gate.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let listener_task = tokio::spawn(listener.run(shutdown.clone()));
    let (upgraded, backend_upgraded) = tokio::sync::oneshot::channel();
    let first_task = tokio::spawn(async move {
        let (stream, _) = first_backend.accept().await.unwrap();
        let mut stream = tokio_tungstenite::accept_async(stream).await.unwrap();
        upgraded.send(()).unwrap();
        // No native initialize is sent for the transport-only upgrade.
        let mut application_frames = 0;
        while let Some(frame) = stream.next().await {
            match frame {
                Ok(Message::Text(_) | Message::Binary(_)) => application_frames += 1,
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        application_frames
    });

    // Act: complete the actual frontend upgrade, then replace its generation.
    let socket = UnixStream::connect(&frontend_path).await.unwrap();
    let (mut held, _) = tokio::time::timeout(
        Duration::from_secs(2),
        tokio_tungstenite::client_async("ws://localhost/", socket),
    )
    .await
    .unwrap()
    .unwrap();
    backend_upgraded.await.unwrap();
    assert_eq!(gate.acquire().unwrap().generation(), &generation(1));
    gate.retire().unwrap();
    gate.activate(generation(2), second_path, None).unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(2), held.next())
        .await
        .unwrap();
    assert!(matches!(
        closed,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));

    // Assert: the retired connection cannot send initialize or open generation2.
    assert!(
        held.send(Message::Text("{\"id\":1,\"method\":\"initialize\"}".into()))
            .await
            .is_err()
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), first_task)
            .await
            .unwrap()
            .unwrap(),
        0
    );
    let mut replacement_accept = Box::pin(second_backend.accept());
    assert!(matches!(
        futures_util::poll!(replacement_accept.as_mut()),
        Poll::Pending
    ));
    drop(replacement_accept);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), listener_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
