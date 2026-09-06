use codex_native_integration::{NativeInputSubmission, NativeProtocolConnection};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

#[tokio::test]
async fn start_and_exact_steer_preserve_input_and_return_acceptance() {
    let (client, server) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let mut client = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let fixture = tokio::spawn(async move {
        for method in ["turn/start", "turn/steer"] {
            let message = server
                .next()
                .await
                .unwrap_or_else(|| panic!("request"))
                .unwrap_or_else(|e| panic!("frame: {e}"));
            let request: Value =
                serde_json::from_str(message.to_text().unwrap_or_else(|e| panic!("text: {e}")))
                    .unwrap_or_else(|e| panic!("JSON: {e}"));
            assert_eq!(request["method"], method);
            assert_eq!(
                request["params"]["clientUserMessageId"],
                "sender-correlation"
            );
            assert_eq!(
                request["params"]["input"],
                json!([{"type":"text","text":"check this","textElements":[]}])
            );
            let result = if method == "turn/start" {
                assert!(request["params"].get("expectedTurnId").is_none());
                json!({"turn":{"id":"turn-1","status":"inProgress"}})
            } else {
                assert_eq!(request["params"]["expectedTurnId"], "turn-1");
                json!({"turnId":"turn-1"})
            };
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
    let input = vec![json!({"type":"text","text":"check this","textElements":[]})];
    let submission = || NativeInputSubmission {
        thread_id: "thread-1",
        input: &input,
        client_user_message_id: Some("sender-correlation"),
    };
    assert_eq!(
        client
            .start_input(submission())
            .await
            .unwrap_or_else(|e| panic!("start: {e}")),
        "turn-1"
    );
    assert_eq!(
        client
            .steer_input(submission(), "turn-1")
            .await
            .unwrap_or_else(|e| panic!("steer: {e}")),
        "turn-1"
    );
    fixture.await.unwrap_or_else(|e| panic!("join: {e}"));
}
