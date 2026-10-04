use futures_util::StreamExt;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// Fixture setup failures must abort the scenario with their specific cause.
#[allow(clippy::expect_used, clippy::panic)]
fn workspace_native_cli_tempdir(prefix: &str) -> tempfile::TempDir {
    let workspace_tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
    std::fs::create_dir_all(&workspace_tmp)
        .unwrap_or_else(|error| panic!("workspace tmp directory: {error}"));
    let workspace_tmp = std::fs::canonicalize(workspace_tmp)
        .unwrap_or_else(|error| panic!("canonical workspace tmp directory: {error}"));
    let temporary = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(workspace_tmp)
        .unwrap_or_else(|error| panic!("private fixture directory: {error}"));
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("private fixture permissions: {error}"));
    temporary
}

// Malformed or missing native frames must fail the real CLI scenario.
#[allow(clippy::expect_used, clippy::panic)]
async fn read_cli_native_interrupt_frame(
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

// A failed isolated child process is a fixture failure, not test data.
#[allow(clippy::panic)]
async fn run_native_interrupt_cli(
    service_directory: &std::path::Path,
    turn_id: &str,
    machine_output: bool,
) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    command.args([
        "turn",
        "interrupt",
        "--endpoint",
        "codex-local",
        "--session",
        "proof-thread",
        "--turn",
        turn_id,
    ]);
    if machine_output {
        command.arg("--json");
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        command
            .arg("--service-directory")
            .arg(service_directory)
            // The timeout drops output(); keep its owned CLI child from escaping the fixture.
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap_or_else(|error| panic!("CLI command deadline: {error}"))
    .unwrap_or_else(|error| panic!("CLI command: {error}"))
}

#[tokio::test]
async fn interrupt_cli_reports_unknown_when_control_disconnects_after_submission() {
    // Arrange: a Control fixture accepts the exact command and drops its response.
    let temporary = workspace_native_cli_tempdir("c1-");
    let root = temporary.path().to_path_buf();
    let listener = tokio::net::UnixListener::bind(root.join("control.sock"))
        .unwrap_or_else(|error| panic!("listener: {error}"));
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let manifest = serde_json::from_value(json!({"version":2,"serviceId":service_id,"serviceEpoch":epoch,"machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).unwrap_or_else(|error| panic!("manifest: {error}"));
    let publication = collaboration_service::ManifestPublication::publish(&root, &manifest)
        .unwrap_or_else(|error| panic!("publish: {error}"));
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .unwrap_or_else(|error| panic!("accept: {error}"));
        let mut stream = BufReader::new(stream);
        for method in ["control/initialize", "endpoint/list", "codex/turnInterrupt"] {
            let mut line = String::new();
            stream
                .read_line(&mut line)
                .await
                .unwrap_or_else(|error| panic!("read: {error}"));
            let request: Value =
                serde_json::from_str(&line).unwrap_or_else(|error| panic!("request: {error}"));
            assert_eq!(request["method"], method);
            if method == "codex/turnInterrupt" {
                assert_eq!(request["params"]["target"]["sessionId"], "proof-thread");
                assert_eq!(request["params"]["turnId"], "proof-turn");
                assert_eq!(request["params"]["generation"]["generation"], 1);
                break;
            }
            let result = if method == "control/initialize" {
                json!({"version":{"major":1,"minor":0},"serviceId":service_id,"serviceEpoch":epoch,"controlSchemaDigest":digest})
            } else {
                json!({"serviceEpoch":epoch,"sequence":0,"endpoints":[{"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"label":"Fixture Codex","availability":{"state":"available","observedAt":"2026-09-05T12:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":{"serviceEpoch":epoch,"generation":1}}]}]})
            };
            let response = format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":request["id"],"result":result})
            );
            stream
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap_or_else(|error| panic!("response: {error}"));
        }
    });
    // Act.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args([
                "turn",
                "interrupt",
                "--endpoint",
                "codex-local",
                "--session",
                "proof-thread",
                "--turn",
                "proof-turn",
                "--json",
                "--service-directory",
            ])
            .arg(&root)
            .output(),
    )
    .await
    .unwrap_or_else(|error| panic!("command deadline: {error}"))
    .unwrap_or_else(|error| panic!("command: {error}"));
    fixture
        .await
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    drop(publication);
    std::fs::remove_file(root.join("control.sock"))
        .unwrap_or_else(|error| panic!("socket cleanup: {error}"));
    temporary
        .close()
        .unwrap_or_else(|error| panic!("directory cleanup: {error}"));
    // Assert.
    assert_eq!(output.status.code(), Some(5));
    let result: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("output: {error}"));
    assert_eq!(result["error"]["kind"], "outcomeUnknown");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn session_inspect_cli_preserves_native_rejection_message() {
    let temporary = workspace_native_cli_tempdir("c2-");
    let root = temporary.path().to_path_buf();
    let listener = tokio::net::UnixListener::bind(root.join("control.sock"))
        .unwrap_or_else(|error| panic!("listener: {error}"));
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .unwrap_or_else(|error| panic!("manifest: {error}"));
    let publication = collaboration_service::ManifestPublication::publish(&root, &manifest)
        .unwrap_or_else(|error| panic!("publish: {error}"));
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("Control accept");
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("read init")
                .expect("init frame"),
        )
        .expect("init JSON");
        let initialized = json!({"jsonrpc":"2.0","id":initialize["id"],"result":{
            "version":{"major":1,"minor":0},"serviceId":service_id,
            "serviceEpoch":epoch,"serviceVersion":env!("CARGO_PKG_VERSION"),"controlSchemaDigest":digest
        }});
        write
            .write_all(format!("{initialized}\n").as_bytes())
            .await
            .expect("write init");
        let discovery: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("read discovery")
                .expect("discovery frame"),
        )
        .expect("discovery JSON");
        assert_eq!(discovery["method"], "endpoint/list");
        let inventory = json!({"jsonrpc":"2.0","id":discovery["id"],"result":{
            "serviceEpoch":epoch,"sequence":0,"endpoints":[{
                "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
                "label":"Fixture Codex",
                "availability":{"state":"available","observedAt":"2026-09-05T12:00:00Z"},
                "channels":[{"kind":"nativeCodex","transport":"unixWebSocket",
                    "path":"codex-native.sock","schemaDigest":null,
                    "generation":{"serviceEpoch":epoch,"generation":1}}]
            }]
        }});
        write
            .write_all(format!("{inventory}\n").as_bytes())
            .await
            .expect("write discovery");
        let inspect: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("read inspect")
                .expect("inspect frame"),
        )
        .expect("inspect JSON");
        assert_eq!(inspect["method"], "codex/sessionInspect");
        let message = "native thread is unreadable: fixture refusal";
        let response = json!({"jsonrpc":"2.0","id":inspect["id"],"error":{
            "code":-32050,"message":message,"data":{
                "kind":"nativeRejected","stage":"inspect","message":message,
                "reason":"unknown","nextAction":"inspectTarget","nativeCode":-32600
            }
        }});
        write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .expect("write rejection");
    });

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args([
                "session",
                "inspect",
                "--endpoint",
                "codex-local",
                "--session",
                "unreadable-thread",
                "--json",
                "--service-directory",
            ])
            .arg(&root)
            .output(),
    )
    .await
    .unwrap_or_else(|error| panic!("command deadline: {error}"))
    .unwrap_or_else(|error| panic!("command: {error}"));
    peer.await
        .unwrap_or_else(|error| panic!("Control fixture: {error}"));
    drop(publication);
    std::fs::remove_file(root.join("control.sock"))
        .unwrap_or_else(|error| panic!("socket cleanup: {error}"));
    temporary
        .close()
        .unwrap_or_else(|error| panic!("directory cleanup: {error}"));

    assert_eq!(
        output.status.code(),
        Some(4),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
    assert_eq!(
        result["error"]["message"],
        "native thread is unreadable: fixture refusal"
    );
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn message_cli_retains_target_after_response_loss_and_keeps_refusal_distinct() {
    for (label, rejection_kind, expected_exit) in [
        ("response-loss", None, 5),
        ("outcome-unknown", Some("outcomeUnknown"), 5),
        ("native-refusal", Some("nativeRejected"), 4),
    ] {
        let temporary = workspace_native_cli_tempdir("c3-");
        let root = temporary.path().to_path_buf();
        let listener = tokio::net::UnixListener::bind(root.join("control.sock"))
            .unwrap_or_else(|error| panic!("listener: {error}"));
        let service_id = "00000000-0000-4000-8000-000000000001";
        let epoch = "00000000-0000-4000-8000-000000000002";
        let digest = format!("sha256:{}", "a".repeat(64));
        let manifest = serde_json::from_value(json!({
            "version":2,"serviceId":service_id,"serviceEpoch":epoch,
            "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        }))
        .unwrap_or_else(|error| panic!("manifest: {error}"));
        let publication = collaboration_service::ManifestPublication::publish(&root, &manifest)
            .unwrap_or_else(|error| panic!("publish: {error}"));
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("Control accept");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            for method in ["control/initialize", "message/send"] {
                let request: Value = serde_json::from_str(
                    &lines
                        .next_line()
                        .await
                        .expect("Control read")
                        .expect("Control frame"),
                )
                .expect("Control JSON");
                assert_eq!(request["method"], method);
                if method == "message/send" {
                    assert_eq!(request["params"]["target"]["sessionId"], "proof-thread");
                    if let Some(kind) = rejection_kind {
                        let outcome = if kind == "nativeRejected" {
                            json!({"kind":"rejected","reason":"busy","nextAction":"inspectTarget","clientCode":-32000,"detail":"native client is busy"})
                        } else {
                            json!({"kind":"unknown"})
                        };
                        let delivery_state = if kind == "nativeRejected" {
                            "rejected"
                        } else {
                            "outcome-unknown"
                        };
                        let push_id = "019f0000-0000-7000-8000-000000000101";
                        let link = format!("router://{service_id}/push/{push_id}");
                        let response = json!({
                            "jsonrpc":"2.0","id":request["id"],
                            "result":{
                                "pushId":push_id,
                                "link":link,
                                "target":request["params"]["target"].clone(),
                                "targetIdentity":"Codex target",
                                "deliveryState":delivery_state,
                                "receipt":{"outcome":outcome,"reachability":"codexAppServer","client":null}
                            }
                        });
                        write
                            .write_all(format!("{response}\n").as_bytes())
                            .await
                            .expect("rejection response");
                    }
                    break;
                }
                let result = if method == "control/initialize" {
                    json!({"version":{"major":1,"minor":0},"serviceId":service_id,"serviceEpoch":epoch,"serviceVersion":env!("CARGO_PKG_VERSION"),"controlSchemaDigest":digest})
                } else {
                    json!({"serviceEpoch":epoch,"sequence":0,"endpoints":[{
                        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"label":"Fixture Codex",
                        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
                        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":{"serviceEpoch":epoch,"generation":1}}]
                    }]})
                };
                write
                    .write_all(
                        format!(
                            "{}\n",
                            json!({"jsonrpc":"2.0","id":request["id"],"result":result})
                        )
                        .as_bytes(),
                    )
                    .await
                    .expect("Control response");
            }
        });
        let target = json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"proof-thread"}).to_string();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
                .args([
                    "message",
                    "send",
                    "--human-user",
                    "--to",
                    &target,
                    "--text",
                    "proof",
                    "--json",
                    "--service-directory",
                ])
                .arg(&root)
                .output(),
        )
        .await
        .unwrap_or_else(|error| panic!("{label} command deadline: {error}"))
        .unwrap_or_else(|error| panic!("command: {error}"));
        peer.await.expect("peer join");
        drop(publication);
        std::fs::remove_file(root.join("control.sock")).expect("socket cleanup");
        temporary.close().expect("directory cleanup");

        assert_eq!(output.status.code(), Some(expected_exit), "{label}");
        let result: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
        let schemas = collaboration_client::protocol::protocol_type_schemas()
            .expect("protocol schema export");
        let schema = schemas
            .get("FiniteCommandRecord")
            .expect("FiniteCommandRecord schema");
        let validator = jsonschema::validator_for(schema).expect("finite record validator");
        if let Err(error) = validator.validate(&result) {
            panic!("{label} exported schema rejected actual stdout: {error}; {result}");
        }
        let _: collaboration_client::protocol::FiniteCommandRecord<
            Value,
            collaboration_client::protocol::AdapterOperationFailure,
        > = serde_json::from_value(result.clone()).expect("published finite message record");
        if let Some(kind) = rejection_kind {
            assert_eq!(
                result["result"]["record"]["deliveryState"],
                if kind == "nativeRejected" {
                    "rejected"
                } else {
                    "outcome-unknown"
                },
                "{label}"
            );
            assert_eq!(
                result["result"]["record"]["receipt"]["reachability"], "codexAppServer",
                "{label}"
            );
            let outcome = &result["result"]["record"]["receipt"]["outcome"];
            assert_eq!(
                outcome["kind"],
                if kind == "nativeRejected" {
                    "rejected"
                } else {
                    "unknown"
                },
                "{label}: {result}"
            );
            if kind == "nativeRejected" {
                assert_eq!(outcome["reason"], "busy");
                assert_eq!(outcome["nextAction"], "inspectTarget");
                assert_eq!(outcome["clientCode"], -32000);
            }
        } else {
            assert_eq!(result["target"]["sessionId"], "proof-thread", "{label}");
            assert_eq!(result["error"]["effect"], "unknown", "{label}");
        }
        assert!(output.stderr.is_empty(), "{label}");
    }
}

#[tokio::test]
async fn native_interrupt_cli_formats_real_service_refusals_in_json_and_human_modes() {
    use collaboration_service::{
        LocalControlService, ManifestPublication, NativeControlBackend, NativeGenerationGate,
        ServiceIdentity,
    };
    use futures_util::SinkExt;
    use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_util::sync::CancellationToken;

    let temporary = workspace_native_cli_tempdir("u2-");
    let service_directory = temporary.path().to_path_buf();
    let native_directory = temporary.path().join("n");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&native_directory)
        .expect("private native socket parent");

    let service_id = "00000000-0000-4000-8000-000000000021";
    let service_epoch = "00000000-0000-4000-8000-000000000022";
    let control_digest = format!("sha256:{}", "a".repeat(64));
    let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch":service_epoch,"generation":1
    }))
    .expect("native generation");
    let target: collaboration_protocol::SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"proof-thread"
    }))
    .expect("native target");
    let native_socket_path = native_directory.join("codex-native.sock");
    let native_listener =
        tokio::net::UnixListener::bind(&native_socket_path).expect("native WebSocket listener");

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
                .expect("native schema JSON"),
        )]))
        .expect("native schema bundle");
    let native_schemas = Arc::new(
        codex_native_integration::NativePayloadSchemas::from_bundle(&native_bundle)
            .expect("native schemas"),
    );
    let native_digest = native_schemas.schema_digest().to_owned();
    let endpoint: collaboration_protocol::EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"CLI interrupt fixture",
        "availability":{"state":"available","observedAt":"2026-10-04T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket",
            "path":"codex-native.sock","schemaDigest":native_digest,"generation":generation}]
    }))
    .expect("native endpoint");
    let gate = NativeGenerationGate::default();
    gate.activate(generation, native_socket_path, Some(native_schemas))
        .expect("native generation admission");
    let backend = NativeControlBackend {
        endpoint: target.endpoint.clone(),
        gate,
        codex_home: native_directory,
    };
    let identity = ServiceIdentity::new(service_id, service_epoch, &control_digest)
        .expect("Control service identity")
        .with_endpoints(vec![endpoint])
        .expect("endpoint publication")
        .with_native_backend(backend)
        .expect("native backend");
    let control = LocalControlService::bind(&service_directory.join("control.sock"), identity)
        .expect("Control listener");
    let manifest: collaboration_protocol::ServiceManifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":service_epoch,
        "machineLabel":"cli-interrupt-fixture",
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":control_digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let publication =
        ManifestPublication::publish(&service_directory, &manifest).expect("manifest publication");
    let control_shutdown = CancellationToken::new();
    let control_task = tokio::spawn(control.run(control_shutdown.clone()));

    let native_peer = tokio::spawn(async move {
        let cases = [
            ("proof-cli-busy", -32000, "thread has an active turn"),
            ("proof-cli-unknown", -32099, "native host refused this turn"),
        ];
        let mut observed_requests = Vec::new();
        for (expected_turn_id, code, message) in cases {
            let (stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(10), native_listener.accept())
                    .await
                    .expect("native accept deadline")
                    .expect("native accept");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("native WebSocket upgrade");
            let initialize = read_cli_native_interrupt_frame(&mut socket).await;
            assert_eq!(initialize["method"], "initialize");
            socket
                .send(Message::Text(
                    json!({"id":initialize["id"],"result":{}})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("native initialize response");
            let initialized = read_cli_native_interrupt_frame(&mut socket).await;
            assert_eq!(initialized["method"], "initialized");
            let request = read_cli_native_interrupt_frame(&mut socket).await;
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
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"error":{"code":code,"message":message}})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("native explicit refusal");
        }
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                native_listener.accept(),
            )
            .await
            .is_err(),
            "CLI interruption refusals must not be replayed"
        );
        observed_requests
    });

    let machine_output = run_native_interrupt_cli(&service_directory, "proof-cli-busy", true).await;
    assert_eq!(machine_output.status.code(), Some(4));
    assert!(machine_output.stderr.is_empty());
    let machine_json: Value =
        serde_json::from_slice(&machine_output.stdout).expect("machine CLI refusal JSON");
    assert_eq!(machine_json["kind"], "error");
    assert_eq!(machine_json["error"]["kind"], "nativeRejected");
    assert_eq!(
        machine_json["error"]["message"],
        "thread has an active turn"
    );
    let machine_error = machine_json["error"]
        .as_object()
        .expect("machine error fields");
    assert_eq!(
        machine_error.len(),
        2,
        "CLI keeps its existing error field set"
    );
    assert!(machine_error.get("reason").is_none());
    assert!(machine_error.get("nativeCode").is_none());

    let human_output =
        run_native_interrupt_cli(&service_directory, "proof-cli-unknown", false).await;
    assert_eq!(human_output.status.code(), Some(4));
    assert!(human_output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(human_output.stderr).expect("human CLI UTF-8"),
        "native host refused this turn\n"
    );

    let observed_requests = native_peer.await.expect("native peer task");
    assert_eq!(
        observed_requests,
        vec![
            ("proof-thread".to_owned(), "proof-cli-busy".to_owned()),
            ("proof-thread".to_owned(), "proof-cli-unknown".to_owned()),
        ]
    );
    drop(publication);
    control_shutdown.cancel();
    control_task
        .await
        .expect("Control service task")
        .expect("Control service shutdown");
    temporary.close().expect("fixture directory cleanup");
}
