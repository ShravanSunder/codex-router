use communication_service::{NativeGenerationGate, NativeRelayListener};
use futures_util::{SinkExt, StreamExt};
use std::{os::unix::fs::DirBuilderExt, time::Duration};
use tokio::net::{UnixListener, UnixStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn stable_frontend_reconnects_only_to_the_admitted_successor() {
    let root = std::path::PathBuf::from(format!("/tmp/native-listener-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|e| panic!("directory: {e}"));
    let front = root.join("front.sock");
    let gate = NativeGenerationGate::default();
    let listener =
        NativeRelayListener::bind(&front, gate.clone()).unwrap_or_else(|e| panic!("bind: {e}"));
    let stop = CancellationToken::new();
    let running = tokio::spawn(listener.run(stop.clone()));
    for number in [1, 2] {
        let unavailable = UnixStream::connect(&front)
            .await
            .unwrap_or_else(|e| panic!("connect: {e}"));
        assert!(
            tokio::time::timeout(
                Duration::from_secs(2),
                tokio_tungstenite::client_async("ws://localhost/", unavailable)
            )
            .await
            .unwrap_or_else(|e| panic!("unavailable timeout: {e}"))
            .is_err()
        );
        let path = root.join(format!("back-{number}.sock"));
        let backend = UnixListener::bind(&path).unwrap_or_else(|e| panic!("backend: {e}"));
        let fixture = tokio::spawn(async move {
            let (stream, _) = backend
                .accept()
                .await
                .unwrap_or_else(|e| panic!("accept: {e}"));
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .unwrap_or_else(|e| panic!("upgrade: {e}"));
            socket
                .send(Message::Text(format!("generation-{number}").into()))
                .await
                .unwrap_or_else(|e| panic!("send: {e}"));
            let _closed = socket.next().await;
        });
        let generation=serde_json::from_value(serde_json::json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":number})).unwrap_or_else(|e|panic!("generation: {e}"));
        gate.activate(generation, path.clone(), None)
            .unwrap_or_else(|e| panic!("activate: {e}"));
        let stream = UnixStream::connect(&front)
            .await
            .unwrap_or_else(|e| panic!("client: {e}"));
        let (mut client, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
            .await
            .unwrap_or_else(|e| panic!("upgrade: {e}"));
        let received = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .unwrap_or_else(|e| panic!("timeout: {e}"))
            .unwrap_or_else(|| panic!("message"))
            .unwrap_or_else(|e| panic!("frame: {e}"));
        assert_eq!(
            received,
            Message::Text(format!("generation-{number}").into())
        );
        gate.retire().unwrap_or_else(|e| panic!("retire: {e}"));
        let closed = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .unwrap_or_else(|e| panic!("close timeout: {e}"));
        assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
        fixture.await.unwrap_or_else(|e| panic!("fixture: {e}"));
        std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
    }
    stop.cancel();
    running
        .await
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("listener: {e}"));
    assert!(!front.exists());
    std::fs::remove_dir(root).unwrap_or_else(|e| panic!("cleanup directory: {e}"));
}
