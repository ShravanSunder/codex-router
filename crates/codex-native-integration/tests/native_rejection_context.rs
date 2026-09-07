use codex_native_integration::{NativeConnectionError, NativeProtocolConnection};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{Message, protocol::Role};

#[tokio::test]
async fn explicit_rejection_context_is_consumable_and_absent_from_error_display() {
    // Arrange: native error text may contain sensitive backend content.
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let wire =
        tokio_tungstenite::WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let backend = tokio::spawn(async move {
        let mut server =
            tokio_tungstenite::WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let request: Value =
            serde_json::from_str(server.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        server.send(Message::Text(json!({"id":request["id"],"error":{"code":-32600,"message":"private diagnostic sentinel"}}).to_string().into())).await.unwrap();
    });
    let mut client = NativeProtocolConnection::from_websocket(wire);
    // Act.
    let error = client.request("thread/start", json!({})).await.unwrap_err();
    let context = client.take_last_rejection().unwrap();
    // Assert: opt-in diagnostic data, never automatic error text, and no stale reuse.
    assert!(matches!(
        error,
        NativeConnectionError::Rejected { code: -32600 }
    ));
    assert!(!error.to_string().contains("private diagnostic sentinel"));
    assert_eq!(context["message"], "private diagnostic sentinel");
    assert!(client.take_last_rejection().is_none());
    backend.await.unwrap();
}
