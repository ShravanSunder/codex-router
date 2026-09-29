use collaboration_client::ControlClient;
use collaboration_protocol::{
    CodexGeneration, NativeSessionListParams, NativeSessionObservation, NativeSessionScope,
    NativeSessionSource, NativeSessionView, SessionRef,
};
use collaboration_service::{
    NativeControlBackend, NativeGenerationGate, ServiceIdentity, serve_control_connection,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn empty_loaded_thread_is_hidden_by_default_and_visible_with_explicit_opt_in() {
    let temporary = tempfile::tempdir().expect("isolated inventory directory");
    let backend_path = temporary.path().join("backend.sock");
    let listener = tokio::net::UnixListener::bind(&backend_path).expect("backend socket");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let service_epoch = "00000000-0000-4000-8000-000000000002";
    let generation: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch":service_epoch,
        "generation":1
    }))
    .expect("generation");
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"empty-thread"
    }))
    .expect("target");
    let mut endpoint: collaboration_protocol::EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,
        "label":"Fixture Codex",
        "availability":{"state":"available","observedAt":"2026-09-28T12:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":generation}]
    }))
    .expect("endpoint description");
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "ThreadTurnsList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}})).expect("schema JSON"),
    )]))
    .expect("schema bundle");
    let digest = format!(
        "sha256:{}",
        bundle
            .digest()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    if let Some(collaboration_protocol::ChannelDescription::NativeCodex { schema_digest, .. }) =
        endpoint.channels.first_mut()
    {
        *schema_digest = Some(digest.try_into().expect("schema digest"));
    }
    let schemas = Arc::new(
        codex_native_integration::NativePayloadSchemas::from_bundle(&bundle)
            .expect("validated app-server schemas"),
    );
    let gate = NativeGenerationGate::default();
    gate.activate(generation.clone(), backend_path.clone(), Some(schemas))
        .expect("activate app-server generation");
    let native_backend = NativeControlBackend {
        endpoint: target.endpoint.clone(),
        gate,
        codex_home: temporary.path().to_owned(),
    };
    let identity = ServiceIdentity::new(
        service_id,
        service_epoch,
        &format!("sha256:{}", "a".repeat(64)),
    )
    .expect("service identity")
    .with_endpoints(vec![endpoint])
    .expect("register endpoint")
    .with_native_backend(native_backend)
    .expect("register native backend");
    let (client_stream, service_stream) = tokio::net::UnixStream::pair().expect("control pair");
    let service = tokio::spawn(serve_control_connection(service_stream, identity));
    let backend = tokio::spawn(async move {
        for expected_include_empty_sessions in [false, true, false, true] {
            let (stream, _) = listener.accept().await.expect("app-server accept");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("app-server websocket");
            let mut expected_methods = vec!["initialize", "thread/loaded/list", "thread/read"];
            if !expected_include_empty_sessions {
                expected_methods.push("thread/turns/list");
            }
            let last_expected_method = *expected_methods.last().expect("expected operations");
            for expected_method in expected_methods {
                let request = loop {
                    let frame =
                        tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
                            .await
                            .expect("bounded app-server request")
                            .expect("app-server request frame")
                            .expect("app-server request text");
                    let Message::Text(text) = frame else {
                        continue;
                    };
                    let request: Value = serde_json::from_str(&text).expect("app-server JSON-RPC");
                    if request.get("id").is_some() {
                        break request;
                    }
                };
                let method = request["method"].as_str().expect("request method");
                assert_eq!(method, expected_method);
                let id = &request["id"];
                let result = match method {
                    "initialize" => json!({}),
                    "thread/loaded/list" => {
                        json!({"data":["empty-thread"],"nextCursor":null})
                    }
                    "thread/read" => json!({"thread":{
                        "id":"empty-thread",
                        "title":"empty-thread",
                        "cwd":"/repo",
                        "status":{"type":"active","activeFlags":[]},
                        "createdAt":1_700_000_000,
                        "updatedAt":1_700_000_000
                    }}),
                    "thread/turns/list" => {
                        assert!(!expected_include_empty_sessions);
                        assert_eq!(request["params"]["sortDirection"], "asc");
                        assert_eq!(request["params"]["itemsView"], "full");
                        json!({"data":[],"nextCursor":null,"backwardsCursor":null})
                    }
                    unexpected => panic!("unexpected app-server method: {unexpected}"),
                };
                socket
                    .send(Message::Text(
                        json!({"id":id,"result":result}).to_string().into(),
                    ))
                    .await
                    .expect("app-server response");
                if method == last_expected_method {
                    socket
                        .send(Message::Close(None))
                        .await
                        .expect("app-server close frame");
                }
            }
        }
    });
    let mut client = ControlClient::initialize(client_stream, "empty-session-inventory", "1")
        .await
        .expect("initialize Control client");
    let request = |view, include_empty_sessions| NativeSessionListParams {
        endpoint: target.endpoint.clone(),
        view,
        scope: NativeSessionScope::Any,
        source: NativeSessionSource::All,
        include_empty_sessions,
        query: None,
        page_size: 10,
        cursor: None,
    };

    let hidden = client
        .list_sessions(request(NativeSessionView::Loaded, false))
        .await
        .expect("default inventory");
    assert!(
        hidden.sessions.is_empty(),
        "empty live thread must be hidden by default"
    );

    let visible = client
        .list_sessions(request(NativeSessionView::Loaded, true))
        .await
        .expect("explicit empty-session inventory");
    assert_eq!(visible.sessions.len(), 1);
    assert!(matches!(
        visible.sessions[0].observation,
        NativeSessionObservation::Runtime { .. }
    ));

    let hidden_active = client
        .list_sessions(request(NativeSessionView::Active, false))
        .await
        .expect("default active inventory");
    assert!(
        hidden_active.sessions.is_empty(),
        "empty active thread must be hidden"
    );

    let visible_active = client
        .list_sessions(request(NativeSessionView::Active, true))
        .await
        .expect("explicit empty active inventory");
    assert_eq!(visible_active.sessions.len(), 1);

    client.close().await.expect("close Control client");
    service
        .await
        .expect("service task")
        .expect("service result");
    backend.await.expect("app-server task");
}
