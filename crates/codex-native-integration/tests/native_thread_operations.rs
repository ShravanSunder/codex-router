use codex_native_integration::{NativeConnectionError, NativeProtocolConnection};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

#[tokio::test]
async fn exact_interrupt_rejects_empty_id_and_inspect_does_not_resume() {
    let (client, server) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let mut client = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    assert!(matches!(
        client.interrupt_turn("thread", "").await,
        Err(NativeConnectionError::InvalidInput)
    ));
    let fixture = tokio::spawn(async move {
        for (method, params, result) in [
            (
                "thread/read",
                json!({"threadId":"thread","includeTurns":false}),
                json!({"thread":{"id":"thread","status":{"type":"idle"}}}),
            ),
            (
                "turn/interrupt",
                json!({"threadId":"thread","turnId":"turn"}),
                json!({}),
            ),
        ] {
            let message = server
                .next()
                .await
                .unwrap_or_else(|| panic!("request"))
                .unwrap_or_else(|e| panic!("read: {e}"));
            let request: Value =
                serde_json::from_str(message.to_text().unwrap_or_else(|e| panic!("text: {e}")))
                    .unwrap_or_else(|e| panic!("JSON: {e}"));
            assert_eq!(request["method"], method);
            assert_eq!(request["params"], params);
            server
                .send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap_or_else(|e| panic!("send: {e}"));
        }
    });
    let thread = client
        .inspect_thread("thread")
        .await
        .unwrap_or_else(|e| panic!("inspect: {e}"));
    assert_eq!(thread["id"], "thread");
    client
        .interrupt_turn("thread", "turn")
        .await
        .unwrap_or_else(|e| panic!("interrupt: {e}"));
    fixture.await.unwrap_or_else(|e| panic!("join: {e}"));
}
