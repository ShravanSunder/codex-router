use serde_json::{Value, json};
use std::os::unix::fs::DirBuilderExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn interrupt_cli_reports_unknown_when_control_disconnects_after_submission() {
    // Arrange: a Control fixture accepts the exact command and drops its response.
    let root = std::path::PathBuf::from(format!("/tmp/interrupt-cli-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let listener = tokio::net::UnixListener::bind(root.join("control.sock"))
        .unwrap_or_else(|error| panic!("listener: {error}"));
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let manifest = serde_json::from_value(json!({"version":2,"serviceId":service_id,"serviceEpoch":epoch,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).unwrap_or_else(|error| panic!("manifest: {error}"));
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
    std::fs::remove_dir(root).unwrap_or_else(|error| panic!("directory cleanup: {error}"));
    // Assert.
    assert_eq!(output.status.code(), Some(5));
    let result: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("output: {error}"));
    assert_eq!(result["error"]["kind"], "outcomeUnknown");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn message_cli_retains_target_after_response_loss_and_keeps_refusal_distinct() {
    for (label, rejection_kind, expected_exit) in [
        ("response-loss", None, 5),
        ("outcome-unknown", Some("outcomeUnknown"), 5),
        ("native-refusal", Some("nativeRejected"), 4),
    ] {
        let root =
            std::path::PathBuf::from(format!("/tmp/message-cli-{label}-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap_or_else(|error| panic!("directory: {error}"));
        let listener = tokio::net::UnixListener::bind(root.join("control.sock"))
            .unwrap_or_else(|error| panic!("listener: {error}"));
        let service_id = "00000000-0000-4000-8000-000000000001";
        let epoch = "00000000-0000-4000-8000-000000000002";
        let digest = format!("sha256:{}", "a".repeat(64));
        let manifest = serde_json::from_value(json!({
            "version":2,"serviceId":service_id,"serviceEpoch":epoch,
            "control":{"transport":"unixJsonLines","path":"control.sock"},
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
                        let response = json!({
                            "jsonrpc":"2.0","id":request["id"],
                            "result":{"outcome":outcome,"reachability":"codexAppServer","client":null}
                        });
                        write
                            .write_all(format!("{response}\n").as_bytes())
                            .await
                            .expect("rejection response");
                    }
                    break;
                }
                let result = if method == "control/initialize" {
                    json!({"version":{"major":1,"minor":0},"serviceId":service_id,"serviceEpoch":epoch,"serviceVersion":"0.1.37","controlSchemaDigest":digest})
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
        std::fs::remove_dir(&root).expect("directory cleanup");

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
                result["result"]["record"]["reachability"], "codexAppServer",
                "{label}"
            );
            let outcome = &result["result"]["record"]["outcome"];
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
