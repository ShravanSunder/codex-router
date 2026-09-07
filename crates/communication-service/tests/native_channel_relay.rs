use communication_service::relay_native_channels;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn opaque_messages_keep_native_ids_and_reverse_requests() {
    // Arrange: actual Unix transports with WebSocket framing, no native process.
    let (caller, front) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("front: {e}"));
    let (back, native) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("back: {e}"));
    let mut caller = WebSocketStream::from_raw_socket(caller, Role::Client, None).await;
    let front = WebSocketStream::from_raw_socket(front, Role::Server, None).await;
    let back = WebSocketStream::from_raw_socket(back, Role::Client, None).await;
    let mut native = WebSocketStream::from_raw_socket(native, Role::Server, None).await;
    let stop = CancellationToken::new();
    let task = tokio::spawn(relay_native_channels(front, back, stop.clone()));
    // Act / Assert: preserve whitespace, integer precision and dialect exactly.
    let request =
        Message::Text("{  \"id\":9223372036854775807, \"method\":\"thread/read\" }".into());
    caller
        .send(request.clone())
        .await
        .unwrap_or_else(|e| panic!("send: {e}"));
    assert_eq!(
        native
            .next()
            .await
            .unwrap_or_else(|| panic!("frame"))
            .unwrap_or_else(|e| panic!("read: {e}")),
        request
    );
    let callback =
        Message::Text("{\"id\":42,\"method\":\"item/commandExecution/requestApproval\"}".into());
    native
        .send(callback.clone())
        .await
        .unwrap_or_else(|e| panic!("callback: {e}"));
    assert_eq!(
        caller
            .next()
            .await
            .unwrap_or_else(|| panic!("frame"))
            .unwrap_or_else(|e| panic!("read: {e}")),
        callback
    );
    let binary = Message::Binary(vec![0, 255, 1].into());
    native
        .send(binary.clone())
        .await
        .unwrap_or_else(|e| panic!("binary: {e}"));
    assert_eq!(
        caller
            .next()
            .await
            .unwrap_or_else(|| panic!("frame"))
            .unwrap_or_else(|e| panic!("read: {e}")),
        binary
    );
    stop.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap_or_else(|e| panic!("retirement: {e}"))
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("relay: {e}"));
}
