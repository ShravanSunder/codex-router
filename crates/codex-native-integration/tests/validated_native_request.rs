use codex_native_integration::{
    NativeConnectionError, NativeOperation, NativePayloadSchemas, NativeProtocolConnection,
    NativeSchemaBundle,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

#[tokio::test]
async fn invalid_payload_never_dispatches_and_invalid_mutation_result_retires_connection() {
    // Arrange: exact fixture schemas and a real local WebSocket pair.
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object","required":["threadId"],"properties":{"threadId":{"type":"string"}}}));
        definitions.insert(
            format!("{name}Response"),
            json!({"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}}}),
        );
    }
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))
            .unwrap_or_else(|error| panic!("schema: {error}")),
    )]))
    .unwrap_or_else(|error| panic!("bundle: {error}"));
    let schemas = NativePayloadSchemas::from_bundle(&bundle)
        .unwrap_or_else(|error| panic!("schemas: {error}"));
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
    let mut client = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let fixture = tokio::spawn(async move {
        let message = server
            .next()
            .await
            .unwrap_or_else(|| panic!("request"))
            .unwrap_or_else(|error| panic!("receive: {error}"));
        let request: Value = serde_json::from_str(
            message
                .to_text()
                .unwrap_or_else(|error| panic!("text: {error}")),
        )
        .unwrap_or_else(|error| panic!("request JSON: {error}"));
        assert_eq!(request["params"], json!({"threadId":"proof-thread"}));
        server
            .send(Message::Text(
                json!({"id":request["id"],"result":{}}).to_string().into(),
            ))
            .await
            .unwrap_or_else(|error| panic!("response: {error}"));
    });
    // Act.
    let invalid = client
        .request_validated(&schemas, NativeOperation::StartTurn, json!({"threadId":12}))
        .await;
    let uncertain = client
        .request_validated(
            &schemas,
            NativeOperation::StartTurn,
            json!({"threadId":"proof-thread"}),
        )
        .await;
    let retired = client
        .request("thread/read", json!({"threadId":"proof-thread"}))
        .await;
    fixture
        .await
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    // Assert.
    assert!(matches!(invalid, Err(NativeConnectionError::InvalidInput)));
    assert!(matches!(
        uncertain,
        Err(NativeConnectionError::OutcomeUnknown)
    ));
    assert!(matches!(retired, Err(NativeConnectionError::Unavailable)));
}
