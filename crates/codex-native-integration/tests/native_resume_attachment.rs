use codex_native_integration::{NativeConnectionError, NativeProtocolConnection};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

#[tokio::test]
async fn resume_preserves_effective_configuration_and_buffers_attachment_events() {
    let (client, server) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let mut client = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let fixture = tokio::spawn(async move {
        for target in ["thread", "mismatch"] {
            let message = server
                .next()
                .await
                .unwrap_or_else(|| panic!("request"))
                .unwrap_or_else(|e| panic!("frame: {e}"));
            let request: Value =
                serde_json::from_str(message.to_text().unwrap_or_else(|e| panic!("text: {e}")))
                    .unwrap_or_else(|e| panic!("JSON: {e}"));
            assert_eq!(request["method"], "thread/resume");
            assert_eq!(request["params"], json!({"threadId":target}));
            server.send(Message::Text(json!({"method":"thread/status/changed","params":{"threadId":target,"status":{"type":"idle"}}}).to_string().into())).await.unwrap_or_else(|e|panic!("event: {e}"));
            server.send(Message::Text(json!({"id":request["id"],"result":{"thread":{"id":"thread"},"cwd":"/tmp/worktree","model":"native-model"}}).to_string().into())).await.unwrap_or_else(|e|panic!("result: {e}"));
        }
    });
    let resumed = client
        .resume_thread("thread")
        .await
        .unwrap_or_else(|e| panic!("resume: {e}"));
    assert_eq!(resumed["cwd"], "/tmp/worktree");
    assert_eq!(resumed["model"], "native-model");
    assert_eq!(
        client
            .next_message()
            .await
            .unwrap_or_else(|e| panic!("event: {e}"))["method"],
        "thread/status/changed"
    );
    assert!(matches!(
        client.resume_thread("mismatch").await,
        Err(NativeConnectionError::OutcomeUnknown)
    ));
    fixture.await.unwrap_or_else(|e| panic!("join: {e}"));
}
