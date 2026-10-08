use super::workspace_native_control_tempdir;
use collaboration_client::{ClientError, ControlClient};
use collaboration_protocol::{CodexGeneration, EndpointDescription, SessionRef};
use collaboration_service::{
    LocalControlService, ManifestPublication, NativeControlBackend, NativeGenerationGate,
    ServiceIdentity,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc};
use tokio_tungstenite::tungstenite::Message;

// Malformed or missing native frames must fail the real propagation scenario.
#[allow(clippy::expect_used, clippy::panic)]
async fn read_interrupt_fixture_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Value {
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
        .await
        .expect("native fixture frame deadline")
        .expect("native fixture frame")
        .unwrap_or_else(|error| panic!("native fixture receive: {error}"));
    serde_json::from_str(
        frame
            .to_text()
            .unwrap_or_else(|error| panic!("native fixture text: {error}")),
    )
    .unwrap_or_else(|error| panic!("native fixture JSON: {error}"))
}

// A failed isolated Control connection is a fixture failure, not test data.
#[allow(clippy::panic)]
async fn open_interrupt_fixture_client(service_directory: &std::path::Path) -> ControlClient {
    ControlClient::connect(service_directory, "interrupt-proof", "1")
        .await
        .unwrap_or_else(|error| panic!("Control client: {error}"))
}

#[tokio::test]
async fn interrupt_refusal_preserves_diagnostics_and_lost_response_stays_unknown_without_replay() {
    enum NativeReply {
        Rejected { code: i64, message: &'static str },
        Completed,
        DropConnection,
    }

    let temporary = workspace_native_control_tempdir("u2-");
    let service_directory = temporary.path().to_path_buf();
    let native_directory = temporary.path().join("n");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&native_directory)
        .unwrap_or_else(|error| panic!("native directory: {error}"));

    let service_id = "00000000-0000-4000-8000-000000000011";
    let service_epoch = "00000000-0000-4000-8000-000000000012";
    let control_digest = format!("sha256:{}", "a".repeat(64));
    let generation: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch":service_epoch,"generation":1
    }))
    .unwrap_or_else(|error| panic!("generation: {error}"));
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"proof-thread"
    }))
    .unwrap_or_else(|error| panic!("target: {error}"));
    let native_socket_path = native_directory.join("codex-native.sock");
    let native_listener = tokio::net::UnixListener::bind(&native_socket_path)
        .unwrap_or_else(|error| panic!("native listener: {error}"));

    let mut native_definitions = serde_json::Map::new();
    for operation in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "ThreadTurnsList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
        "ThreadSetName",
    ] {
        native_definitions.insert(format!("{operation}Params"), json!({"type":"object"}));
        native_definitions.insert(format!("{operation}Response"), json!({"type":"object"}));
    }
    let native_bundle =
        codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
            "codex_app_server_protocol.schemas.json".to_owned(),
            serde_json::to_vec(&json!({"definitions":{"v2":native_definitions}}))
                .unwrap_or_else(|error| panic!("native schema JSON: {error}")),
        )]))
        .unwrap_or_else(|error| panic!("native schema bundle: {error}"));
    let native_schemas = Arc::new(
        codex_native_integration::NativePayloadSchemas::from_bundle(&native_bundle)
            .unwrap_or_else(|error| panic!("native schemas: {error}")),
    );
    let native_digest = native_schemas.schema_digest().to_owned();
    let mut endpoint: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Interrupt fixture",
        "availability":{"state":"available","observedAt":"2026-10-04T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket",
            "path":"codex-native.sock","schemaDigest":native_digest,"generation":generation}]
    }))
    .unwrap_or_else(|error| panic!("endpoint: {error}"));
    if let Some(collaboration_protocol::ChannelDescription::NativeCodex { schema_digest, .. }) =
        endpoint.channels.first_mut()
    {
        *schema_digest = Some(
            native_digest
                .clone()
                .try_into()
                .unwrap_or_else(|error| panic!("native digest: {error}")),
        );
    }
    let gate = NativeGenerationGate::default();
    gate.activate(generation.clone(), native_socket_path, Some(native_schemas))
        .unwrap_or_else(|error| panic!("native generation: {error}"));
    let backend = NativeControlBackend {
        endpoint: target.endpoint.clone(),
        gate,
        codex_home: native_directory,
    };
    let identity = ServiceIdentity::new(service_id, service_epoch, &control_digest)
        .unwrap_or_else(|error| panic!("service identity: {error}"))
        .with_endpoints(vec![endpoint])
        .unwrap_or_else(|error| panic!("endpoint publication: {error}"))
        .with_native_backend(backend)
        .unwrap_or_else(|error| panic!("native backend: {error}"));
    let control = LocalControlService::bind(&service_directory.join("control.sock"), identity)
        .unwrap_or_else(|error| panic!("Control listener: {error}"));
    let manifest: collaboration_protocol::ServiceManifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":service_epoch,
        "machineLabel":"u2-interrupt-fixture",
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":control_digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .unwrap_or_else(|error| panic!("service manifest: {error}"));
    let publication = ManifestPublication::publish(&service_directory, &manifest)
        .unwrap_or_else(|error| panic!("manifest publication: {error}"));
    let control_shutdown = tokio_util::sync::CancellationToken::new();
    let control_task = tokio::spawn(control.run(control_shutdown.clone()));

    let native_peer = tokio::spawn(async move {
        let cases = [
            (
                "proof-busy",
                NativeReply::Rejected {
                    code: -32000,
                    message: "thread has an active turn",
                },
            ),
            (
                "proof-unknown",
                NativeReply::Rejected {
                    code: -32099,
                    message: "native host refused this turn",
                },
            ),
            ("proof-success", NativeReply::Completed),
            ("proof-lost", NativeReply::DropConnection),
        ];
        let mut observed_requests = Vec::new();
        for (expected_turn_id, reply) in cases {
            let (stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(3), native_listener.accept())
                    .await
                    .expect("native accept deadline")
                    .unwrap_or_else(|error| panic!("native accept: {error}"));
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .unwrap_or_else(|error| panic!("native WebSocket upgrade: {error}"));
            let initialize = read_interrupt_fixture_frame(&mut socket).await;
            assert_eq!(initialize["method"], "initialize");
            socket
                .send(Message::Text(
                    json!({"id":initialize["id"],"result":{}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap_or_else(|error| panic!("native initialize response: {error}"));
            let initialized = read_interrupt_fixture_frame(&mut socket).await;
            assert_eq!(initialized["method"], "initialized");
            let request = read_interrupt_fixture_frame(&mut socket).await;
            assert_eq!(request["method"], "turn/interrupt");
            assert_eq!(request["params"]["threadId"], "proof-thread");
            assert_eq!(request["params"]["turnId"], expected_turn_id);
            observed_requests.push((
                request["params"]["threadId"]
                    .as_str()
                    .expect("native thread ID")
                    .to_owned(),
                request["params"]["turnId"]
                    .as_str()
                    .expect("native turn ID")
                    .to_owned(),
            ));

            match reply {
                NativeReply::Rejected { code, message } => {
                    socket
                        .send(Message::Text(
                            json!({"id":request["id"],"error":{"code":code,"message":message}})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap_or_else(|error| panic!("native refusal: {error}"));
                }
                NativeReply::Completed => {
                    socket
                        .send(Message::Text(
                            json!({"id":request["id"],"result":{}}).to_string().into(),
                        ))
                        .await
                        .unwrap_or_else(|error| panic!("native completion: {error}"));
                }
                NativeReply::DropConnection => {
                    socket
                        .close(None)
                        .await
                        .unwrap_or_else(|error| panic!("native lost response close: {error}"));
                }
            }
        }
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                native_listener.accept(),
            )
            .await
            .is_err(),
            "an interruption must not be replayed after an observed native request"
        );
        observed_requests
    });

    // Each ControlClient retires its exchange after a returned service error;
    // production MCP uses one fresh connection per tool operation as well.
    let mut busy_client = open_interrupt_fixture_client(&service_directory).await;
    let busy = busy_client
        .interrupt_turn(&target, &generation, "proof-busy")
        .await;
    let busy_data = match busy {
        Err(ClientError::Rejected {
            code: -32050,
            data: Some(data),
        }) => data,
        other => panic!("classified native refusal was not retained: {other:?}"),
    };
    assert_eq!(busy_data["kind"], "nativeRejected");
    assert_eq!(busy_data["stage"], "interrupt");
    assert_eq!(busy_data["message"], "thread has an active turn");
    assert_eq!(busy_data["reason"], "busy");
    assert_eq!(busy_data["nextAction"], "useDeliverySteer");
    assert!(busy_data.get("nativeCode").is_none());
    busy_client
        .close()
        .await
        .unwrap_or_else(|error| panic!("busy Control client close: {error}"));

    let mut unknown_client = open_interrupt_fixture_client(&service_directory).await;
    let unknown = unknown_client
        .interrupt_turn(&target, &generation, "proof-unknown")
        .await;
    let unknown_data = match unknown {
        Err(ClientError::Rejected {
            code: -32050,
            data: Some(data),
        }) => data,
        other => panic!("unclassified native refusal was not retained: {other:?}"),
    };
    assert_eq!(unknown_data["kind"], "nativeRejected");
    assert_eq!(unknown_data["stage"], "interrupt");
    assert_eq!(unknown_data["message"], "native host refused this turn");
    assert_eq!(unknown_data["reason"], "unknown");
    assert_eq!(unknown_data["nextAction"], "retryLater");
    assert_eq!(unknown_data["nativeCode"], -32099);
    unknown_client
        .close()
        .await
        .unwrap_or_else(|error| panic!("unknown Control client close: {error}"));

    let mut success_client = open_interrupt_fixture_client(&service_directory).await;
    let completed = success_client
        .interrupt_turn(&target, &generation, "proof-success")
        .await
        .unwrap_or_else(|error| panic!("successful interruption: {error}"));
    assert_eq!(completed.target, target);
    assert_eq!(completed.generation, generation);
    assert_eq!(String::from(completed.turn_id), "proof-success");
    assert_eq!(
        completed.kind,
        collaboration_protocol::NativeInterruptKind::InterruptCompleted
    );
    success_client
        .close()
        .await
        .unwrap_or_else(|error| panic!("success Control client close: {error}"));

    let mut stale_generation = generation.clone();
    stale_generation.generation = 2_u64
        .try_into()
        .unwrap_or_else(|error| panic!("stale generation number: {error}"));
    let mut stale_client = open_interrupt_fixture_client(&service_directory).await;
    let stale = stale_client
        .interrupt_turn(&target, &stale_generation, "proof-stale")
        .await;
    assert!(
        matches!(&stale, Err(ClientError::Rejected { data: Some(data), .. }) if data["kind"] == "staleGeneration"),
        "stale generation must be refused before the native request: {stale:?}"
    );
    stale_client
        .close()
        .await
        .unwrap_or_else(|error| panic!("stale Control client close: {error}"));

    let mut lost_client = open_interrupt_fixture_client(&service_directory).await;
    let lost = lost_client
        .interrupt_turn(&target, &generation, "proof-lost")
        .await;
    assert!(
        matches!(&lost, Err(ClientError::Rejected { data: Some(data), .. })
            if data["kind"] == "outcomeUnknown"
                && data["stage"] == "interrupt"
                && data["message"].as_str().is_some_and(|message| message.contains("no request was replayed"))),
        "post-dispatch loss must stay unknown and retain its no-replay diagnostic: {lost:?}"
    );
    lost_client
        .close()
        .await
        .unwrap_or_else(|error| panic!("lost Control client close: {error}"));

    let observed_requests = native_peer
        .await
        .unwrap_or_else(|error| panic!("native peer task: {error}"));
    assert_eq!(
        observed_requests,
        vec![
            ("proof-thread".to_owned(), "proof-busy".to_owned()),
            ("proof-thread".to_owned(), "proof-unknown".to_owned()),
            ("proof-thread".to_owned(), "proof-success".to_owned()),
            ("proof-thread".to_owned(), "proof-lost".to_owned()),
        ],
        "only the exact-generation interruption requests reach the native peer"
    );

    drop(publication);
    control_shutdown.cancel();
    control_task
        .await
        .unwrap_or_else(|error| panic!("Control service task: {error}"))
        .unwrap_or_else(|error| panic!("Control service shutdown: {error}"));
}
