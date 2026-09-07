use codex_native_integration::NativeProtocolConnection;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

#[tokio::test]
async fn native_response_keeps_interleaved_events_and_callbacks() {
    let (client, server) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let socket = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let fixture = tokio::spawn(async move {
        let frame = server
            .next()
            .await
            .unwrap_or_else(|| panic!("request"))
            .unwrap_or_else(|e| panic!("frame: {e}"));
        let request: Value =
            serde_json::from_str(frame.to_text().unwrap_or_else(|e| panic!("text: {e}")))
                .unwrap_or_else(|e| panic!("JSON: {e}"));
        assert!(request.get("jsonrpc").is_none());
        for value in [
            json!({"method":"thread/status/changed","params":{"threadId":"t","status":{"type":"idle"}}}),
            json!({"id":9223372036854775807_i64,"method":"item/commandExecution/requestApproval","params":{}}),
            json!({"id":request["id"],"result":{"thread":{"id":"t"}}}),
        ] {
            server
                .send(Message::Text(value.to_string().into()))
                .await
                .unwrap_or_else(|e| panic!("send: {e}"));
        }
    });
    let mut client = NativeProtocolConnection::from_websocket(socket);
    let result = client
        .request("thread/read", json!({"threadId":"t"}))
        .await
        .unwrap_or_else(|e| panic!("request: {e}"));
    assert_eq!(result["thread"]["id"], "t");
    assert_eq!(
        client
            .next_message()
            .await
            .unwrap_or_else(|e| panic!("event: {e}"))["method"],
        "thread/status/changed"
    );
    assert_eq!(
        client
            .next_message()
            .await
            .unwrap_or_else(|e| panic!("callback: {e}"))["id"]
            .as_i64(),
        Some(i64::MAX)
    );
    fixture.await.unwrap_or_else(|e| panic!("join: {e}"));
}
