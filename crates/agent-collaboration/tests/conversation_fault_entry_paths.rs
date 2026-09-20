//! CLI subprocess proof for conversation failures before any ACP dispatch.

use serde_json::Value;
use serde_json::json;
use std::os::unix::fs::DirBuilderExt;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

#[allow(clippy::panic)]
fn validate_conversation_records(
    stdout: &[u8],
    label: &str,
) -> Vec<collaboration_client::protocol::ConversationRecord> {
    let schemas = collaboration_client::protocol::protocol_type_schemas()
        .unwrap_or_else(|error| panic!("{label} schema export: {error}"));
    let schema = schemas
        .get("ConversationRecord")
        .unwrap_or_else(|| panic!("{label} ConversationRecord schema"));
    let validator = jsonschema::validator_for(schema)
        .unwrap_or_else(|error| panic!("{label} validator: {error}"));
    String::from_utf8(stdout.to_vec())
        .unwrap_or_else(|error| panic!("{label} UTF-8: {error}"))
        .lines()
        .map(|line| {
            let value: Value =
                serde_json::from_str(line).unwrap_or_else(|error| panic!("{label} JSON: {error}"));
            if let Err(error) = validator.validate(&value) {
                panic!("{label} exported schema rejected actual stdout: {error}; {value}");
            }
            serde_json::from_value(value)
                .unwrap_or_else(|error| panic!("{label} record contract: {error}"))
        })
        .collect()
}

#[tokio::test]
async fn standalone_create_manifest_preflight_reports_no_effect_without_acp_dispatch() {
    // Arrange: no manifest or control peer exists, so the compiled command cannot
    // discover an ACP endpoint, much less dispatch `session/new`.
    let directory = std::env::temp_dir().join(format!(
        "conversation-manifest-preflight-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir(&directory).expect("private fixture directory");
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args([
                "conversation",
                "create",
                "--endpoint",
                "codex-local",
                "--model",
                "gpt-5.6-sol",
                "--effort",
                "low",
                "--access",
                "workspace-write",
                "--cwd",
                "/tmp",
                "--service-directory",
            ])
            .arg(&directory)
            .arg("--json")
            .output(),
    )
    .await
    .expect("CLI deadline")
    .expect("CLI output");

    // Assert: this is a truthful pre-dispatch failure, rather than uncertainty
    // about a conversation that might have been created.
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stderr.is_empty());
    let record: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
    let typed_record: collaboration_client::protocol::ConversationRecord =
        serde_json::from_value(record.clone()).expect("published ConversationRecord");
    assert_eq!(record["kind"], "conversationError");
    assert_eq!(record["target"], Value::Null);
    assert_eq!(record["error"]["effect"], "none");
    assert_eq!(record["error"]["stage"], "connect");
    assert!(matches!(
        typed_record,
        collaboration_client::protocol::ConversationRecord::ConversationError { .. }
    ));
    std::fs::remove_dir(&directory).expect("fixture cleanup");
}

#[tokio::test]
async fn fork_response_loss_after_session_new_reports_unknown_without_replay() {
    let root = std::path::PathBuf::from(format!("/tmp/cfl-{}", uuid::Uuid::now_v7()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("private fixture directory");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint":endpoint,"label":"fork loss fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock",
            "schemaDigest":format!("sha256:{}", collaboration_client::protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("identity")
        .with_endpoints(vec![description])
        .expect("endpoint");
    let control =
        collaboration_service::LocalControlService::bind(&root.join("control.sock"), identity)
            .expect("control bind");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("manifest");
    let publication = collaboration_service::ManifestPublication::publish(&root, &manifest)
        .expect("publish manifest");
    let stop = CancellationToken::new();
    let service = tokio::spawn(control.run(stop.clone()));
    let acp = tokio::net::UnixListener::bind(root.join("acp.sock")).expect("ACP bind");
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.expect("ACP accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("init read")
                .expect("init frame"),
        )
        .expect("init JSON");
        assert_eq!(initialize["method"], "initialize");
        writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.expect("init response");
        let creation: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("new read")
                .expect("new frame"),
        )
        .expect("new JSON");
        assert_eq!(creation["method"], "session/new");
        // Drop after the real fork allocation request is observed: no second request
        // is accepted, and the CLI must retain possible-effect uncertainty.
    });
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args([
                "conversation",
                "prompt",
                "--endpoint",
                "codex-local",
                "--fork",
                "source-thread",
                "--model",
                "gpt-5.6-sol",
                "--effort",
                "low",
                "--access",
                "workspace-write",
                "--cwd",
            ])
            .arg(&root)
            .args(["--text", "fork proof", "--service-directory"])
            .arg(&root)
            .args(["--json"])
            .env("CODEX_THREAD_ID", "fork-requester")
            .output(),
    )
    .await
    .expect("CLI deadline")
    .expect("CLI output");
    peer.await.expect("peer join");
    stop.cancel();
    service.await.expect("service join").expect("service stop");
    drop(publication);
    std::fs::remove_file(root.join("acp.sock")).expect("ACP cleanup");
    std::fs::remove_dir(&root).expect("fixture cleanup");
    assert_eq!(output.status.code(), Some(5));
    let record: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
    let _: collaboration_client::protocol::ConversationRecord =
        serde_json::from_value(record.clone()).expect("published ConversationRecord");
    assert_eq!(record["kind"], "conversationError");
    assert_eq!(record["error"]["stage"], "fork");
    assert_eq!(record["error"]["effect"], "unknown");
}

#[tokio::test]
async fn acp_initialize_response_loss_reports_no_effect_before_conversation_creation() {
    let root = std::path::PathBuf::from(format!("/tmp/cil-{}", uuid::Uuid::now_v7()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("private fixture directory");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let description = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"}, "label":"initialize loss fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}", collaboration_client::protocol::ACP_SCHEMA_DIGEST)}]
    })).expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("identity")
        .with_endpoints(vec![description])
        .expect("endpoint");
    let control =
        collaboration_service::LocalControlService::bind(&root.join("control.sock"), identity)
            .expect("control bind");
    let manifest = serde_json::from_value(json!({"version":2,"serviceId":service_id,"serviceEpoch":epoch,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).expect("manifest");
    let publication = collaboration_service::ManifestPublication::publish(&root, &manifest)
        .expect("publish manifest");
    let stop = CancellationToken::new();
    let service = tokio::spawn(control.run(stop.clone()));
    let acp = tokio::net::UnixListener::bind(root.join("acp.sock")).expect("ACP bind");
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.expect("ACP accept");
        let mut lines = BufReader::new(stream).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("init read")
                .expect("init frame"),
        )
        .expect("init JSON");
        assert_eq!(initialize["method"], "initialize");
        // The peer observed the dispatched initialization and deliberately loses its reply.
    });
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args([
                "conversation",
                "create",
                "--endpoint",
                "codex-local",
                "--model",
                "gpt-5.6-sol",
                "--effort",
                "low",
                "--access",
                "workspace-write",
                "--cwd",
            ])
            .arg(&root)
            .args(["--service-directory"])
            .arg(&root)
            .arg("--json")
            .env("CODEX_THREAD_ID", "initialize-requester")
            .output(),
    )
    .await
    .expect("CLI deadline")
    .expect("CLI output");
    peer.await.expect("peer join");
    stop.cancel();
    service.await.expect("service join").expect("service stop");
    drop(publication);
    std::fs::remove_file(root.join("acp.sock")).expect("ACP cleanup");
    std::fs::remove_dir(&root).expect("fixture cleanup");
    assert_eq!(output.status.code(), Some(2));
    let record: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
    let _: collaboration_client::protocol::ConversationRecord =
        serde_json::from_value(record.clone()).expect("published ConversationRecord");
    assert_eq!(record["kind"], "conversationError");
    assert_eq!(record["error"]["stage"], "initialize");
    assert_eq!(record["error"]["effect"], "none");
}

#[tokio::test]
async fn compiled_cli_conversation_records_deserialize_for_success_errors_deadline_and_interrupt() {
    // This is deliberately a compiled-CLI test. Each outcome comes from a real
    // Control discovery plus a deterministic ACP peer, then is decoded through
    // the public closed DTO rather than constructed as a fixture record.
    let root = std::path::PathBuf::from(format!(
        "/tmp/conversation-records-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("private fixture directory");
    let service_id = "00000000-0000-4000-8000-0000c0dec001";
    let epoch = "00000000-0000-4000-8000-0000c0dec002";
    let digest = format!("sha256:{}", "d".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint":endpoint,"label":"record fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock",
            "schemaDigest":format!("sha256:{}", collaboration_client::protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("identity")
        .with_endpoints(vec![description])
        .expect("endpoint");
    let control =
        collaboration_service::LocalControlService::bind(&root.join("control.sock"), identity)
            .expect("control bind");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    })).expect("manifest");
    let publication = collaboration_service::ManifestPublication::publish(&root, &manifest)
        .expect("publish manifest");
    let stop = CancellationToken::new();
    let service = tokio::spawn(control.run(stop.clone()));
    let acp = tokio::net::UnixListener::bind(root.join("acp.sock")).expect("ACP bind");
    let (interrupt_prompt_seen, interrupt_prompt_ready) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let mut interrupt_prompt_seen = Some(interrupt_prompt_seen);
        for outcome in [
            "create-probe",
            "create",
            "success",
            "rejected",
            "deadline",
            "interrupt",
        ] {
            let (stream, _) = acp.accept().await.expect("ACP accept");
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let initialize: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("init read")
                    .expect("init frame"),
            )
            .expect("init JSON");
            assert_eq!(initialize["method"], "initialize");
            writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.expect("init response");
            if outcome == "create-probe" {
                continue;
            }
            let creation: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("new read")
                    .expect("new frame"),
            )
            .expect("new JSON");
            assert_eq!(creation["method"], "session/new");
            let session_id = format!("{outcome}-thread");
            writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":creation["id"],"result":{"sessionId":session_id}})).as_bytes()).await.expect("new response");
            if outcome == "create" {
                continue;
            }
            let prompt: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("prompt read")
                    .expect("prompt frame"),
            )
            .expect("prompt JSON");
            assert_eq!(prompt["method"], "session/prompt");
            match outcome {
                "success" => writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn","_meta":{"codexRouter":{"effectiveModel":"gpt-5.6-sol","effectiveEffort":"low","effectiveAccess":"workspace-write","settingsObservation":{"kind":"observed","source":"threadStart","observedAt":"2026-09-19T00:00:00Z","routerAccess":"workspace-write","nativeSandbox":null,"permissionProfile":null,"approvalPolicy":null,"approvalsReviewer":null}}}}})).as_bytes()).await.expect("success response"),
                "rejected" => writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":prompt["id"],"error":{"code":-32603,"message":"fixture rejection","data":{"kind":"nativeRejected"}}})).as_bytes()).await.expect("rejection response"),
                "deadline" | "interrupt" => {
                    if outcome == "interrupt" {
                        interrupt_prompt_seen
                            .take()
                            .expect("one interrupt signal")
                            .send(())
                            .expect("interrupt signal");
                    }
                    let cancel: Value = serde_json::from_str(&lines.next_line().await.expect("cancel read").expect("cancel frame")).expect("cancel JSON");
                    assert_eq!(cancel["method"], "session/cancel");
                    writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"cancelled"}})).as_bytes()).await.expect("settlement response");
                }
                _ => panic!("fixed fixture outcomes"),
            }
        }
    });

    let create = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "conversation",
            "create",
            "--endpoint",
            "codex-local",
            "--model",
            "gpt-5.6-sol",
            "--effort",
            "low",
            "--access",
            "workspace-write",
            "--cwd",
        ])
        .arg(&root)
        .args(["--service-directory"])
        .arg(&root)
        .arg("--json")
        .env("CODEX_THREAD_ID", "record-creator")
        .output()
        .await
        .expect("create output");
    assert!(create.status.success());
    let created_records = validate_conversation_records(&create.stdout, "created");
    assert!(created_records.iter().any(|record| matches!(
        record,
        collaboration_client::protocol::ConversationRecord::ConversationCreated { .. }
    )));

    async fn prompt(root: &std::path::Path, timeout_seconds: u64) -> std::process::Output {
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args([
                "conversation",
                "prompt",
                "--endpoint",
                "codex-local",
                "--new",
                "--model",
                "gpt-5.6-sol",
                "--effort",
                "low",
                "--access",
                "workspace-write",
                "--cwd",
            ])
            .arg(root)
            .args(["--text", "contract proof", "--timeout-seconds"])
            .arg(timeout_seconds.to_string())
            .args(["--service-directory"])
            .arg(root)
            .arg("--json")
            .env("CODEX_THREAD_ID", "record-prompter")
            .output()
            .await
            .expect("prompt output")
    }
    let success = prompt(&root, 5).await;
    assert!(success.status.success());
    let success_records = validate_conversation_records(&success.stdout, "success");
    assert!(success_records.iter().any(|record| matches!(
        record,
        collaboration_client::protocol::ConversationRecord::PromptResult { .. }
    )));

    let rejected = prompt(&root, 5).await;
    assert_eq!(rejected.status.code(), Some(4));
    let rejected_records = validate_conversation_records(&rejected.stdout, "rejection");
    assert!(matches!(
        rejected_records.last(),
        Some(collaboration_client::protocol::ConversationRecord::ConversationError { .. })
    ));

    let deadline = prompt(&root, 1).await;
    assert_eq!(deadline.status.code(), Some(124));
    let deadline_records = validate_conversation_records(&deadline.stdout, "deadline");
    assert!(matches!(
        deadline_records.last(),
        Some(
            collaboration_client::protocol::ConversationRecord::ConversationSettlement {
                terminal_reason:
                    collaboration_client::protocol::ConversationTerminalReason::TimedOut,
                ..
            }
        )
    ));

    let mut interrupted = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    interrupted
        .args([
            "conversation",
            "prompt",
            "--endpoint",
            "codex-local",
            "--new",
            "--model",
            "gpt-5.6-sol",
            "--effort",
            "low",
            "--access",
            "workspace-write",
            "--cwd",
        ])
        .arg(&root)
        .args([
            "--text",
            "interrupt proof",
            "--timeout-seconds",
            "5",
            "--service-directory",
        ])
        .arg(&root)
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CODEX_THREAD_ID", "record-interrupter");
    let child = interrupted.spawn().expect("interrupt command");
    interrupt_prompt_ready
        .await
        .expect("interrupt prompt observed");
    // Let the compiled command's Ctrl-C listener register before delivering the
    // native interrupt; the peer has already observed the real prompt dispatch.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let pid = child.id().expect("child PID").to_string();
    let signal = tokio::process::Command::new("/bin/kill")
        .args(["-INT", &pid])
        .status()
        .await
        .expect("SIGINT command");
    assert!(signal.success());
    let interrupted = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .expect("interrupt deadline")
        .expect("interrupt output");
    assert_eq!(interrupted.status.code(), Some(130));
    let interrupted_records = validate_conversation_records(&interrupted.stdout, "interrupt");
    assert!(matches!(
        interrupted_records.last(),
        Some(
            collaboration_client::protocol::ConversationRecord::ConversationSettlement {
                terminal_reason:
                    collaboration_client::protocol::ConversationTerminalReason::Cancelled,
                ..
            }
        )
    ));

    tokio::time::timeout(Duration::from_secs(5), peer)
        .await
        .expect("peer deadline")
        .expect("peer join");
    stop.cancel();
    service.await.expect("service join").expect("service stop");
    drop(publication);
    std::fs::remove_file(root.join("acp.sock")).expect("ACP cleanup");
    std::fs::remove_dir(&root).expect("fixture cleanup");
}

#[tokio::test]
async fn resumed_prompt_load_response_loss_retains_target_and_never_replays() {
    let output = run_resumed_prompt_after_load(None).await;

    assert_eq!(output.status.code(), Some(5));
    let record: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
    assert_eq!(record["kind"], "conversationError");
    assert_eq!(
        record["target"],
        json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
            "sessionId":"resumed-thread"
        })
    );
    assert_eq!(record["error"]["stage"], "load");
    assert_eq!(record["error"]["effect"], "unknown");
}

#[tokio::test]
async fn resumed_prompt_load_rejection_retains_target_code_and_data_without_prompt_replay() {
    let output = run_resumed_prompt_after_load(Some(json!({
        "code": -32050,
        "message": "fixture load rejected",
        "data": {"kind":"nativeRejected","stage":"load","reason":"fixture-policy"}
    })))
    .await;

    assert_eq!(output.status.code(), Some(4));
    let record: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
    assert_eq!(record["kind"], "conversationError");
    assert_eq!(
        record["target"],
        json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
            "sessionId":"resumed-thread"
        })
    );
    assert_eq!(record["error"]["stage"], "load");
    assert_eq!(record["error"]["effect"], "unknown");
    assert_eq!(record["error"]["code"], -32050);
    assert_eq!(
        record["error"]["data"],
        json!({"kind":"nativeRejected","stage":"load","reason":"fixture-policy"})
    );
}

#[allow(clippy::expect_used, clippy::indexing_slicing)]
async fn run_resumed_prompt_after_load(load_error: Option<Value>) -> std::process::Output {
    let root = std::path::PathBuf::from(format!("/tmp/clr-{}", uuid::Uuid::now_v7()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("private fixture directory");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let description = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"}, "label":"resumed load fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}", collaboration_client::protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("identity")
        .with_endpoints(vec![description])
        .expect("endpoint");
    let control =
        collaboration_service::LocalControlService::bind(&root.join("control.sock"), identity)
            .expect("control bind");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("manifest");
    let publication = collaboration_service::ManifestPublication::publish(&root, &manifest)
        .expect("publish manifest");
    let stop = CancellationToken::new();
    let service = tokio::spawn(control.run(stop.clone()));
    let acp = tokio::net::UnixListener::bind(root.join("acp.sock")).expect("ACP bind");
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.expect("ACP accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("init read")
                .expect("init frame"),
        )
        .expect("init JSON");
        assert_eq!(initialize["method"], "initialize");
        writer
            .write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes())
            .await
            .expect("init response");
        let load: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("load read")
                .expect("load frame"),
        )
        .expect("load JSON");
        assert_eq!(load["method"], "session/load");
        assert_eq!(load["params"]["sessionId"], "resumed-thread");
        if let Some(error) = load_error {
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":load["id"],"error":error})
                    )
                    .as_bytes(),
                )
                .await
                .expect("load rejection");
            assert!(
                lines
                    .next_line()
                    .await
                    .expect("post-rejection read")
                    .is_none(),
                "a rejected load must not prompt or replay"
            );
        } else {
            drop(lines);
            drop(writer);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(250), acp.accept())
                    .await
                    .is_err(),
                "a lost load response must not trigger a second ACP connection or replay"
            );
        }
    });
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args([
                "conversation",
                "prompt",
                "--endpoint",
                "codex-local",
                "--session",
                "resumed-thread",
                "--cwd",
            ])
            .arg(&root)
            .args(["--text", "resume proof", "--service-directory"])
            .arg(&root)
            .arg("--json")
            .env("CODEX_THREAD_ID", "resumed-requester")
            .output(),
    )
    .await
    .expect("CLI deadline")
    .expect("CLI output");
    peer.await.expect("peer join");
    stop.cancel();
    service.await.expect("service join").expect("service stop");
    drop(publication);
    std::fs::remove_file(root.join("acp.sock")).expect("ACP cleanup");
    std::fs::remove_dir(&root).expect("fixture cleanup");
    output
}
