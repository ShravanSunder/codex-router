use codex_native_integration::NativeProtocolConnection;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};

#[tokio::test]
async fn callback_submission_preserves_native_id_and_does_not_wait_for_fabricated_ack() {
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
    let mut client = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    client
        .submit_callback_response(
            json!(9223372036854775807_i64),
            json!({"decision":"decline"}),
        )
        .await
        .unwrap_or_else(|error| panic!("submit: {error}"));
    let message = server
        .next()
        .await
        .unwrap_or_else(|| panic!("message"))
        .unwrap_or_else(|error| panic!("read: {error}"));
    let value: Value = serde_json::from_str(
        message
            .to_text()
            .unwrap_or_else(|error| panic!("text: {error}")),
    )
    .unwrap_or_else(|error| panic!("JSON: {error}"));
    assert_eq!(
        value,
        json!({"id":9223372036854775807_i64,"result":{"decision":"decline"}})
    );
}
