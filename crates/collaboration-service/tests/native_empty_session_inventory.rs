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
async fn runtime_inventory_filters_unmaterialized_threads_and_keeps_rows_on_turn_errors() {
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
    let thread_ids = ["empty-thread", "error-thread", "preview-thread"];
    let backend = tokio::spawn(async move {
        let mut turns_list_params = Vec::new();
        for expected_include_empty_sessions in [false, true, false, true] {
            let (stream, _) = listener.accept().await.expect("app-server accept");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("app-server websocket");
            let expected_turn_requests = if expected_include_empty_sessions {
                0
            } else {
                2
            };
            let mut read_count = 0;
            let mut turn_request_count = 0;
            while read_count < thread_ids.len() || turn_request_count < expected_turn_requests {
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
                let id = &request["id"];
                match method {
                    "initialize" => {
                        socket
                            .send(Message::Text(
                                json!({"id":id,"result":{}}).to_string().into(),
                            ))
                            .await
                            .expect("initialize response");
                    }
                    "thread/loaded/list" => {
                        socket
                            .send(Message::Text(
                                json!({"id":id,"result":{"data":thread_ids,"nextCursor":null}})
                                    .to_string()
                                    .into(),
                            ))
                            .await
                            .expect("loaded-thread response");
                    }
                    "thread/read" => {
                        let thread_id = request["params"]["threadId"]
                            .as_str()
                            .expect("thread read id");
                        assert!(thread_ids.contains(&thread_id));
                        read_count += 1;
                        let preview = if thread_id == "preview-thread" {
                            "A real first user message"
                        } else {
                            ""
                        };
                        let result = json!({"thread":{
                            "id":thread_id,
                            "title":thread_id,
                            "preview":preview,
                            "cwd":"/repo",
                            "status":{"type":"active","activeFlags":[]},
                            "createdAt":1_700_000_000,
                            "updatedAt":1_700_000_000
                        }});
                        socket
                            .send(Message::Text(
                                json!({"id":id,"result":result}).to_string().into(),
                            ))
                            .await
                            .expect("thread read response");
                    }
                    "thread/turns/list" => {
                        assert!(!expected_include_empty_sessions);
                        turns_list_params.push(request["params"].clone());
                        let thread_id = request["params"]["threadId"]
                            .as_str()
                            .expect("turns list thread id");
                        assert!(matches!(thread_id, "empty-thread" | "error-thread"));
                        turn_request_count += 1;
                        let message = if thread_id == "empty-thread" {
                            format!(
                                "thread {thread_id} is not materialized yet; thread/turns/list is unavailable before first user message"
                            )
                        } else {
                            "a different per-thread turns error".to_owned()
                        };
                        socket
                            .send(Message::Text(
                                json!({"id":id,"error":{"code":-32600,"message":message}})
                                    .to_string()
                                    .into(),
                            ))
                            .await
                            .expect("turns-list rejection response");
                    }
                    unexpected => panic!("unexpected app-server method: {unexpected}"),
                }
            }
            socket
                .send(Message::Close(None))
                .await
                .expect("app-server close frame");
        }
        turns_list_params
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
    let loaded_ids = hidden
        .sessions
        .iter()
        .map(|session| String::from(session.target.session_id.clone()))
        .collect::<Vec<_>>();
    assert_eq!(loaded_ids, ["error-thread", "preview-thread"]);

    let visible = client
        .list_sessions(request(NativeSessionView::Loaded, true))
        .await
        .expect("explicit empty-session inventory");
    assert_eq!(visible.sessions.len(), 3);
    assert!(visible.sessions.iter().all(|session| matches!(
        session.observation,
        NativeSessionObservation::Runtime { .. }
    )));

    let hidden_active = client
        .list_sessions(request(NativeSessionView::Active, false))
        .await
        .expect("default active inventory");
    let active_ids = hidden_active
        .sessions
        .iter()
        .map(|session| String::from(session.target.session_id.clone()))
        .collect::<Vec<_>>();
    assert_eq!(active_ids, ["error-thread", "preview-thread"]);

    let visible_active = client
        .list_sessions(request(NativeSessionView::Active, true))
        .await
        .expect("explicit empty active inventory");
    assert_eq!(visible_active.sessions.len(), 3);

    let turns_list_params = backend.await.expect("app-server task");
    assert_eq!(turns_list_params.len(), 4);
    assert!(turns_list_params.iter().all(|params| {
        params["limit"] == 1
            && params["sortDirection"] == "asc"
            && params["itemsView"] == "summary"
            && matches!(
                params["threadId"].as_str(),
                Some("empty-thread" | "error-thread")
            )
    }));

    client.close().await.expect("close Control client");
    service
        .await
        .expect("service task")
        .expect("service result");
}
