use communication_service::connect_native_relay;
use futures_util::{SinkExt, StreamExt};
use std::os::unix::fs::DirBuilderExt;
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
