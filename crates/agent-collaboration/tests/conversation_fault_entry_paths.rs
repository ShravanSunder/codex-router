#![allow(clippy::expect_used, clippy::indexing_slicing)]
//! CLI subprocess proof for conversation failures before any ACP dispatch.

use serde_json::Value;
use serde_json::json;
use std::os::unix::fs::DirBuilderExt;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

fn create_result_line(stdout: &[u8]) -> Value {
    let lines: Vec<_> = String::from_utf8_lossy(stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), 2, "create operation ID precedes its result");
    let started: collaboration_client::protocol::ConversationRecord =
        serde_json::from_str(&lines[0]).expect("operation start record");
    assert!(matches!(
        started,
        collaboration_client::protocol::ConversationRecord::ConversationCreateStarted { .. }
    ));
    serde_json::from_str(&lines[1]).expect("create result JSON")
}

fn preflight_create_error_line(stdout: &[u8]) -> Value {
    let lines = String::from_utf8_lossy(stdout)
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "preflight validation failures must not emit a create-start record"
    );
    serde_json::from_str(&lines[0]).expect("preflight error JSON")
}

fn create_prompt_result_line(stdout: &[u8]) -> Value {
    let lines: Vec<_> = String::from_utf8_lossy(stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines.len(),
        2,
        "create operation ID precedes prompt outcome"
    );
    let started: collaboration_client::protocol::ConversationRecord =
        serde_json::from_str(&lines[0]).expect("operation start");
    assert!(matches!(
        started,
        collaboration_client::protocol::ConversationRecord::ConversationOperationStarted { .. }
    ));
    let value: Value = serde_json::from_str(&lines[1]).expect("create prompt outcome");
    if value["kind"] != "error" {
        let _: collaboration_client::ConversationCreatePromptOutcome =
            serde_json::from_value(value.clone()).expect("typed create prompt outcome");
    }
    value
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
    let record: Value = preflight_create_error_line(&output.stdout);
    let typed_record: collaboration_client::protocol::ConversationRecord =
        serde_json::from_value(record.clone()).expect("published ConversationRecord");
    assert_eq!(record["kind"], "conversationError");
    assert_eq!(record["target"], Value::Null);
    assert_eq!(record["error"]["effect"], "none");
    assert_eq!(record["error"]["stage"], "manifest-read");
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
        assert_eq!(
            creation["params"]["_meta"]["codexRouter"]["operationId"],
            "019c6e27-e55b-73d1-87d8-4e01f1f75131"
        );
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
                "--operation-id",
                "019c6e27-e55b-73d1-87d8-4e01f1f75131",
                "--cwd",
            ])
            .arg(&root)
            .args(["--text", "fork proof", "--service-directory"])
            .arg(&root)
            .args(["--json"])
            .env("CODEX_THREAD_ID", "fork-requester")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CURSOR_CONVERSATION_ID")
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
async fn fork_rejects_uninspectable_prompt_operation_id() {
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "conversation",
            "prompt",
            "--endpoint",
            "codex-local",
            "--fork",
            "source-thread",
            "--prompt-operation-id",
            "019c6e27-e55b-73d1-87d8-4e01f1f75132",
            "--cwd",
            "/tmp",
            "--access",
            "workspace-write",
            "--text",
            "hello",
            "--json",
        ])
        .output()
        .await
        .expect("fork rejection output");
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).expect("rejection JSON");
    assert_eq!(error["error"]["kind"], "unsupportedCapability");
    assert_eq!(
        error["error"]["message"],
        "omit the operation ID for Codex prompts; it is not inspectable"
    );
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
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CURSOR_CONVERSATION_ID")
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
    let record: Value = create_result_line(&output.stdout);
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
        for outcome in ["create", "success", "rejected", "deadline", "interrupt"] {
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
            let session_id = format!("{outcome}-thread");
            writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":creation["id"],"result":{"sessionId":session_id}})).as_bytes()).await.expect("new response");
            if outcome == "create" {
                continue;
            }
            let (stream, _) = acp.accept().await.expect("prompt ACP accept");
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let initialize: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("prompt init read")
                    .expect("prompt init frame"),
            )
            .expect("prompt init JSON");
            assert_eq!(initialize["method"], "initialize");
            writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.expect("prompt init response");
            let load: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("load read")
                    .expect("load frame"),
            )
            .expect("load JSON");
            assert_eq!(load["method"], "session/load");
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":load["id"],"result":{"sessionId":session_id}})
                    )
                    .as_bytes(),
                )
                .await
                .expect("load response");
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
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CURSOR_CONVERSATION_ID")
        .output()
        .await
        .expect("create output");
    assert!(create.status.success());
    let output_lines: Vec<_> = String::from_utf8_lossy(&create.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(output_lines.len(), 2, "one operation start and one outcome");
    let started: collaboration_client::protocol::ConversationRecord =
        serde_json::from_str(&output_lines[0]).expect("operation start");
    let outcome: collaboration_client::protocol::ConversationCreateOutcome =
        serde_json::from_str(&output_lines[1]).expect("create outcome");
    let (
        collaboration_client::protocol::ConversationRecord::ConversationCreateStarted {
            operation_id,
        },
        collaboration_client::protocol::ConversationCreateOutcome::Created {
            operation_id: result_id,
            ..
        },
    ) = (started, outcome)
    else {
        panic!("create must print the operation ID before its result")
    };
    assert_eq!(operation_id, result_id);

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
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CURSOR_CONVERSATION_ID")
            .output()
            .await
            .expect("prompt output")
    }
    let success = prompt(&root, 5).await;
    assert!(success.status.success());
    let success_result = create_prompt_result_line(&success.stdout);
    assert_eq!(success_result["kind"], "prompt");
    assert_eq!(success_result["prompt"]["kind"], "completed");
    assert_eq!(
        success_result["prompt"]["settlement"]["detail"]["kind"],
        "codexPrompt"
    );
    assert!(success_result["prompt"].get("operationId").is_none());

    let rejected = prompt(&root, 5).await;
    assert_eq!(rejected.status.code(), Some(4));
    let rejected_result = create_prompt_result_line(&rejected.stdout);
    assert_eq!(rejected_result["kind"], "error");
    assert_eq!(rejected_result["error"]["kind"], "afterCreate");
    assert_eq!(
        rejected_result["error"]["target"]["sessionId"],
        "rejected-thread"
    );
    assert_eq!(rejected_result["error"]["source"]["code"], -32603);
    assert_eq!(
        rejected_result["error"]["source"]["data"]["kind"],
        "nativeRejected"
    );

    let deadline = prompt(&root, 1).await;
    assert_eq!(deadline.status.code(), Some(124));
    let deadline_result = create_prompt_result_line(&deadline.stdout);
    assert_eq!(
        deadline_result["prompt"]["settlement"]["stopReason"],
        "timedOut"
    );

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
        .env("CODEX_THREAD_ID", "record-interrupter")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CURSOR_CONVERSATION_ID");
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
    let interrupted_result = create_prompt_result_line(&interrupted.stdout);
    assert_eq!(
        interrupted_result["prompt"]["settlement"]["stopReason"],
        "cancelled"
    );

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
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CURSOR_CONVERSATION_ID")
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

#[tokio::test]
async fn conversation_create_without_identity_or_from_reports_unavailable() {
    let root = std::path::PathBuf::from(format!("/tmp/cfl-identity-{}", uuid::Uuid::now_v7()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("private fixture directory");
    let service_id = "00000000-0000-4000-8000-0000000000aa";
    let epoch = "00000000-0000-4000-8000-0000000000ab";
    let digest = format!("sha256:{}", "e".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint":endpoint,"label":"identity gap fixture",
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
    // Creator validation precedes the ACP connection in the common create path.
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
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CURSOR_CONVERSATION_ID")
            .output(),
    )
    .await
    .expect("CLI deadline")
    .expect("CLI output");
    stop.cancel();
    let _ = service.await;
    drop(publication);
    std::fs::remove_dir(&root).expect("fixture cleanup");
    assert_eq!(output.status.code(), Some(2));
    let record: Value = preflight_create_error_line(&output.stdout);
    assert_eq!(record["kind"], "conversationError");
    assert_eq!(
        record["error"]["message"],
        "Control protocol violation: current session identity unavailable; run agent-collaboration whoami --json or pass --from SessionRef JSON"
    );
}

#[tokio::test]
async fn conversation_create_from_supplies_created_by_without_env() {
    let root = std::path::PathBuf::from(format!("/tmp/cfl-from-{}", uuid::Uuid::now_v7()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("private fixture directory");
    let service_id = "00000000-0000-4000-8000-0000000000ac";
    let epoch = "00000000-0000-4000-8000-0000000000ad";
    let digest = format!("sha256:{}", "f".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint":endpoint,"label":"from override fixture",
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
        for outcome in ["create", "cancel", "load", "prompt", "prompt-id", "load-id"] {
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
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})
                    )
                    .as_bytes(),
                )
                .await
                .expect("init response");
            if matches!(outcome, "cancel" | "prompt-id" | "load-id") {
                assert!(
                    lines.next_line().await.expect("cancel read").is_none(),
                    "Codex cancel must not send ACP work"
                );
                continue;
            }
            if outcome == "load" || outcome == "prompt" {
                let load: Value = serde_json::from_str(
                    &lines
                        .next_line()
                        .await
                        .expect("load read")
                        .expect("load frame"),
                )
                .expect("load JSON");
                assert_eq!(load["method"], "session/load");
                writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":load["id"],"result":{"sessionId":"created-from-override"}})).as_bytes()).await.expect("load response");
                if outcome == "prompt" {
                    let prompt: Value = serde_json::from_str(
                        &lines
                            .next_line()
                            .await
                            .expect("prompt read")
                            .expect("prompt frame"),
                    )
                    .expect("prompt JSON");
                    assert_eq!(prompt["method"], "session/prompt");
                    writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}})).as_bytes()).await.expect("prompt response");
                }
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
            assert_eq!(
                creation["params"]["_meta"]["codexRouter"]["createdBy"]["sessionId"],
                "cursor-conversation"
            );
            assert_eq!(
                creation["params"]["_meta"]["codexRouter"]["approver"]["sessionId"],
                "cursor-conversation"
            );
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":creation["id"],"result":{"sessionId":"created-from-override"}})
                    )
                    .as_bytes(),
                )
                .await
                .expect("new response");
        }
    });
    let from = format!(
        r#"{{"endpoint":{{"serviceId":"{service_id}","endpointId":"codex-local"}},"sessionId":"cursor-conversation"}}"#
    );
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
            .args(["--from", &from, "--service-directory"])
            .arg(&root)
            .arg("--json")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CURSOR_CONVERSATION_ID")
            .output(),
    )
    .await
    .expect("CLI deadline")
    .expect("CLI output");
    assert!(
        output.status.success(),
        "stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let outcome = create_result_line(&output.stdout);
    assert_eq!(outcome["kind"], "created");
    assert_eq!(outcome["target"]["sessionId"], "created-from-override");
    let target = outcome["target"].to_string();
    let cancel = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "conversation",
            "cancel",
            "--target",
            &target,
            "--target-operation-id",
            "019c6e27-e55b-73d1-87d8-4e01f1f75122",
            "--from",
            &from,
            "--service-directory",
        ])
        .arg(&root)
        .arg("--json")
        .output()
        .await
        .expect("cancel output");
    assert_eq!(cancel.status.code(), Some(4));
    let cancelled: Value = serde_json::from_slice(&cancel.stdout).expect("cancel JSON");
    assert_eq!(cancelled["error"]["kind"], "unsupportedCapability");
    assert!(
        cancelled["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("turn interrupt"))
    );

    let load = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "conversation",
            "load",
            "--target",
            &target,
            "--access",
            "workspace-write",
            "--from",
            &from,
            "--cwd",
        ])
        .arg(&root)
        .args(["--service-directory"])
        .arg(&root)
        .arg("--json")
        .output()
        .await
        .expect("load output");
    assert!(
        load.status.success(),
        "stdout={}",
        String::from_utf8_lossy(&load.stdout)
    );
    let loaded: Value = serde_json::from_slice(&load.stdout).expect("load JSON");
    assert_eq!(loaded["kind"], "completed");
    assert_eq!(loaded["settlement"]["detail"]["kind"], "codexLoad");
    assert!(loaded.get("operationId").is_none());

    let prompt = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "conversation",
            "prompt",
            "--to",
            &target,
            "--from",
            &from,
            "--cwd",
        ])
        .arg(&root)
        .args(["--text", "hello", "--service-directory"])
        .arg(&root)
        .arg("--json")
        .output()
        .await
        .expect("prompt output");
    assert!(
        prompt.status.success(),
        "stdout={}",
        String::from_utf8_lossy(&prompt.stdout)
    );
    let prompted: Value = serde_json::from_slice(&prompt.stdout).expect("prompt JSON");
    assert_eq!(prompted["kind"], "completed");
    assert_eq!(prompted["settlement"]["detail"]["kind"], "codexPrompt");
    assert!(prompted.get("operationId").is_none());

    let supplied_id = "019c6e27-e55b-73d1-87d8-4e01f1f75133";
    let prompt_id = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "conversation",
            "prompt",
            "--to",
            &target,
            "--from",
            &from,
            "--cwd",
        ])
        .arg(&root)
        .args([
            "--text",
            "hello",
            "--operation-id",
            supplied_id,
            "--service-directory",
        ])
        .arg(&root)
        .arg("--json")
        .output()
        .await
        .expect("prompt ID rejection output");
    assert_eq!(prompt_id.status.code(), Some(2));
    let prompt_error: Value =
        serde_json::from_slice(&prompt_id.stdout).expect("prompt ID rejection JSON");
    assert_eq!(prompt_error["error"]["kind"], "unsupportedCapability");
    assert_eq!(
        prompt_error["error"]["message"],
        "omit the operation ID for Codex prompts; it is not inspectable"
    );

    let load_id = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "conversation",
            "load",
            "--target",
            &target,
            "--from",
            &from,
            "--cwd",
        ])
        .arg(&root)
        .args([
            "--access",
            "workspace-write",
            "--operation-id",
            supplied_id,
            "--service-directory",
        ])
        .arg(&root)
        .arg("--json")
        .output()
        .await
        .expect("load ID rejection output");
    assert_eq!(load_id.status.code(), Some(2));
    let load_error: Value =
        serde_json::from_slice(&load_id.stdout).expect("load ID rejection JSON");
    assert_eq!(load_error["error"]["kind"], "unsupportedCapability");
    assert_eq!(
        load_error["error"]["message"],
        "omit the operation ID for Codex prompts; it is not inspectable"
    );

    let new_prompt_id = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "conversation",
            "prompt",
            "--new",
            "--endpoint",
            "codex-local",
            "--from",
            &from,
            "--cwd",
        ])
        .arg(&root)
        .args([
            "--access",
            "workspace-write",
            "--text",
            "hello",
            "--prompt-operation-id",
            supplied_id,
            "--service-directory",
        ])
        .arg(&root)
        .arg("--json")
        .output()
        .await
        .expect("new prompt ID rejection output");
    assert_eq!(new_prompt_id.status.code(), Some(2));
    let new_error = create_prompt_result_line(&new_prompt_id.stdout);
    assert_eq!(new_error["error"]["kind"], "unsupportedCapability");
    assert_eq!(
        new_error["error"]["message"],
        "omit the operation ID for Codex prompts; it is not inspectable"
    );

    peer.await.expect("peer join");
    stop.cancel();
    service.await.expect("service join").expect("service stop");
    drop(publication);
    let _ = std::fs::remove_file(root.join("acp.sock"));
    std::fs::remove_dir(&root).expect("fixture cleanup");
}
