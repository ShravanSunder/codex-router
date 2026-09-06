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
    let manifest = serde_json::from_value(json!({"version":1,"serviceId":service_id,"serviceEpoch":epoch,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest})).unwrap_or_else(|error| panic!("manifest: {error}"));
    let publication = communication_service::ManifestPublication::publish(&root, &manifest)
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
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
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
