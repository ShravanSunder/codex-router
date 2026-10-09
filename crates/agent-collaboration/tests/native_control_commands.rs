use serde_json::{Value, json};

mod fake_api_support;
use fake_api_support::{FakeCollaborationApi, FakeReply};

/// One available Codex endpoint with a native carrier at generation 1.
fn native_inventory(service_id: &str, epoch: &str) -> Value {
    json!({"serviceEpoch":epoch,"sequence":0,"endpoints":[{
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"label":"Fixture Codex",
        "availability":{"state":"available","observedAt":"2026-09-05T12:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock",
            "schemaDigest":null,"generation":{"serviceEpoch":epoch,"generation":1}}]
    }]})
}

#[tokio::test]
async fn interrupt_cli_reports_unknown_when_the_api_disconnects_after_submission() {
    // Arrange: a stand-in API accepts the exact command and drops its response.
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let mut fixture = FakeCollaborationApi::new(service_id, epoch)
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    let served = fixture.serve(vec![
        FakeReply::Result(native_inventory(service_id, epoch)),
        FakeReply::Disconnect,
    ]);
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
            .arg(fixture.directory())
            .output(),
    )
    .await
    .unwrap_or_else(|error| panic!("command deadline: {error}"))
    .unwrap_or_else(|error| panic!("command: {error}"));
    let calls = served
        .await
        .unwrap_or_else(|error| panic!("fixture: {error}"))
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    // Assert.
    let tools: Vec<&Value> = calls.iter().map(|call| &call["tool"]).collect();
    assert_eq!(tools, [&json!("endpoints_list"), &json!("turn_interrupt")]);
    let interrupt = &calls[1]["arguments"];
    assert_eq!(interrupt["target"]["sessionId"], "proof-thread");
    assert_eq!(interrupt["turnId"], "proof-turn");
    assert_eq!(interrupt["generation"]["generation"], 1);
    assert_eq!(output.status.code(), Some(5));
    let result: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("output: {error}"));
    assert_eq!(result["error"]["kind"], "outcomeUnknown");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn session_inspect_cli_preserves_native_rejection_message() {
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let mut fixture = FakeCollaborationApi::new(service_id, epoch)
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    let message = "native thread is unreadable: fixture refusal";
    let peer = fixture.serve(vec![
        FakeReply::Result(native_inventory(service_id, epoch)),
        FakeReply::Error(json!({
            "code":-32050,"message":message,"data":{
                "kind":"nativeRejected","stage":"inspect","message":message,
                "reason":"unknown","nextAction":"inspectTarget","nativeCode":-32600
            }
        })),
    ]);

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
            .arg(fixture.directory())
            .output(),
    )
    .await
    .unwrap_or_else(|error| panic!("command deadline: {error}"))
    .unwrap_or_else(|error| panic!("command: {error}"));
    let calls = peer
        .await
        .unwrap_or_else(|error| panic!("API fixture: {error}"))
        .unwrap_or_else(|error| panic!("API fixture: {error}"));
    let tools: Vec<&Value> = calls.iter().map(|call| &call["tool"]).collect();
    assert_eq!(tools, [&json!("endpoints_list"), &json!("session_inspect")]);

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
        let service_id = "00000000-0000-4000-8000-000000000001";
        let epoch = "00000000-0000-4000-8000-000000000002";
        let mut fixture = FakeCollaborationApi::new(service_id, epoch)
            .unwrap_or_else(|error| panic!("fixture: {error}"));
        let reply = match rejection_kind {
            None => FakeReply::Disconnect,
            Some(kind) => {
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
                FakeReply::Receipt(json!({
                    "pushId":push_id,
                    "link":link,
                    "target":{"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"proof-thread"},
                    "targetIdentity":"Codex target",
                    "deliveryState":delivery_state,
                    "receipt":{"outcome":outcome,"reachability":"codexAppServer","client":null}
                }))
            }
        };
        let peer = fixture.serve(vec![reply]);
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
                .arg(fixture.directory())
                .output(),
        )
        .await
        .unwrap_or_else(|error| panic!("{label} command deadline: {error}"))
        .unwrap_or_else(|error| panic!("command: {error}"));
        let calls = peer.await.expect("peer join").expect("API fixture");
        assert_eq!(calls[0]["tool"], "message_send", "{label}");
        assert_eq!(
            calls[0]["arguments"]["target"]["sessionId"], "proof-thread",
            "{label}"
        );

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
