use super::*;

#[tokio::test]
async fn conversation_create_without_identity_or_from_reports_unavailable() {
    let root = std::path::PathBuf::from(format!("/tmp/cfl-identity-{}", uuid::Uuid::now_v7()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("private fixture directory");
    let service_id = "00000000-0000-4000-8000-0000000000aa";
    let epoch = "00000000-0000-4000-8000-0000000000ab";
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint":endpoint,"label":"identity gap fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock",
            "schemaDigest":format!("sha256:{}", collaboration_client::protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch)
        .expect("identity")
        .with_endpoints(vec![description])
        .expect("endpoint");
    let served = collaboration_mcp::test_support::ServedCollaborationApi::start(
        &root,
        collaboration_service::CollaborationApplication::new(identity),
    )
    .await
    .expect("serve collaboration API");
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
    let _ = served.stop().await;
    std::fs::remove_dir(&root).expect("fixture cleanup");
    assert_eq!(output.status.code(), Some(2));
    let record: Value = preflight_create_error_line(&output.stdout);
    assert_eq!(record["kind"], "conversationError");
    assert_eq!(
        record["error"]["message"],
        "collaboration API protocol violation: current session identity unavailable; run agent-collaboration whoami --json or pass --from SessionRef JSON"
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
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint":endpoint,"label":"from override fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock",
            "schemaDigest":format!("sha256:{}", collaboration_client::protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch)
        .expect("identity")
        .with_endpoints(vec![description])
        .expect("endpoint");
    let served = collaboration_mcp::test_support::ServedCollaborationApi::start(
        &root,
        collaboration_service::CollaborationApplication::new(identity),
    )
    .await
    .expect("serve collaboration API");
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
    served.stop().await.expect("collaboration API stops");
    let _ = std::fs::remove_file(root.join("acp.sock"));
    std::fs::remove_dir(&root).expect("fixture cleanup");
}
