use codex_acp_adapter::{
    AcpSchemaCatalog, AcpSessionBinding, PendingAcpPrompt, PromptEvent, SessionSetupInputs,
};
use codex_native_integration::{
    NativePayloadSchemas, NativeProtocolConnection, NativeSchemaBundle,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};
#[path = "support/conversation_operation_recorder.rs"]
mod conversation_operation_recorder;
use conversation_operation_recorder::AcceptingConversationRecorder;

#[tokio::test]
async fn prompt_rechecks_native_activity_before_starting_a_turn() {
    let mut catalog = AcpSchemaCatalog::load().unwrap();
    let definitions: serde_json::Map<String, Value> = [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ]
    .into_iter()
    .flat_map(|name| {
        [
            (format!("{name}Params"), json!({"type":"object"})),
            (format!("{name}Response"), json!({"type":"object"})),
        ]
    })
    .collect();
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions,"ServerRequest":{"type":"object"},"ServerNotification":{"type":"object"}}})).unwrap(),
    )]))
    .unwrap();
    let schemas = Arc::new(NativePayloadSchemas::from_bundle(&bundle).unwrap());
    let generation = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let connection = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let backend = tokio::spawn(async move {
        let mut socket = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        for (method, status) in [("thread/resume", "idle"), ("thread/read", "active")] {
            let frame = socket.next().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            assert_eq!(request["method"], method);
            socket.send(Message::Text(json!({"id":request["id"],"result":{"cwd":"/work","thread":{"id":"thread-a","cwd":"/work","status":{"type":status},"turns":[]}}}).to_string().into())).await.unwrap();
        }
        if let Some(Ok(frame)) = socket.next().await {
            assert!(
                frame.is_close(),
                "busy prompt sent another native request: {frame}"
            );
        }
    });
    let (session, _) = AcpSessionBinding::load_existing(
        &mut catalog,
        SessionSetupInputs {
            operation_id: None,
            recorder: Arc::new(AcceptingConversationRecorder),
            connection,
            schemas,
            generation,
            params: json!({"sessionId":"thread-a","cwd":"/work","mcpServers":[]}),
            approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        },
    )
    .await
    .unwrap();
    let error = PendingAcpPrompt::start(
        session,
        &mut catalog,
        json!("prompt"),
        &json!({"sessionId":"thread-a","prompt":[{"type":"text","text":"work"}]}),
    )
    .await
    .err()
    .expect("active native thread must reject prompt");
    assert_eq!(error.to_string(), "native session has an active turn");
    backend.await.unwrap();
}

#[tokio::test]
async fn failed_native_turn_keeps_its_error_instead_of_projecting_a_result() {
    let mut catalog = AcpSchemaCatalog::load().unwrap();
    let definitions: serde_json::Map<String, Value> = [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ]
    .into_iter()
    .flat_map(|name| {
        [
            (format!("{name}Params"), json!({"type":"object"})),
            (format!("{name}Response"), json!({"type":"object"})),
        ]
    })
    .collect();
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions,"ServerRequest":{"type":"object"},"ServerNotification":{"type":"object"}}})).unwrap(),
    )])).unwrap();
    let schemas = Arc::new(NativePayloadSchemas::from_bundle(&bundle).unwrap());
    let generation = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let connection = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let backend = tokio::spawn(async move {
        let mut socket = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        for (method, result) in [
            (
                "thread/resume",
                json!({"cwd":"/work","thread":{"id":"thread-a","cwd":"/work","status":{"type":"idle"},"turns":[]}}),
            ),
            (
                "thread/read",
                json!({"thread":{"id":"thread-a","status":{"type":"idle"},"turns":[]}}),
            ),
            ("turn/start", json!({"turn":{"id":"turn-a"}})),
        ] {
            let frame = socket.next().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            assert_eq!(request["method"], method);
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        }
        socket.send(Message::Text(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"partial-message","delta":"partial answer"}}).to_string().into())).await.unwrap();
        socket.send(Message::Text(json!({"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"failed"}}}).to_string().into())).await.unwrap();
        if let Some(Ok(frame)) = socket.next().await {
            assert!(
                frame.is_close(),
                "failed terminal caused another native request: {frame}"
            );
        }
    });
    let (session, _) = AcpSessionBinding::load_existing(
        &mut catalog,
        SessionSetupInputs {
            operation_id: None,
            recorder: Arc::new(AcceptingConversationRecorder),
            connection,
            schemas,
            generation,
            params: json!({"sessionId":"thread-a","cwd":"/work","mcpServers":[]}),
            approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        },
    )
    .await
    .unwrap();
    let mut prompt = PendingAcpPrompt::start(
        session,
        &mut catalog,
        json!("prompt"),
        &json!({"sessionId":"thread-a","prompt":[{"type":"text","text":"work"}]}),
    )
    .await
    .unwrap();
    assert!(matches!(
        prompt.next_event(&mut catalog).await.unwrap(),
        Some(PromptEvent::Update(update))
            if update["params"]["update"]["content"]["text"] == "partial answer"
    ));
    let Some(PromptEvent::Terminal(terminal)) = prompt.next_event(&mut catalog).await.unwrap()
    else {
        panic!("failed terminal was not delivered");
    };
    assert_eq!(
        terminal["error"]["message"],
        "Native prompt did not complete successfully"
    );
    assert!(terminal.get("result").is_none());
    drop(prompt);
    backend.await.unwrap();
}
