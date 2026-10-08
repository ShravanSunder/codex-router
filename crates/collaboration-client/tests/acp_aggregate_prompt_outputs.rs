use collaboration_client::{
    AcpConversation, ConversationCreatePromptRequest, ConversationCreateRequest,
    ConversationPromptRequest, ExistingConversationPromptRequest, PublicPromptContent,
};
use collaboration_protocol::MessageText;
use collaboration_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use serde_json::{Value, json};
use std::os::unix::fs::DirBuilderExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn aggregate_existing_prompt_handles_missing_reply_and_history_setup_failure() {
    let root = std::path::PathBuf::from(format!(
        "/tmp/acp-existing-prompt-flow-{}",
        std::process::id()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let service_id = "00000000-0000-4000-8000-000000000004";
    let digest = format!("sha256:{}", "c".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let identity = ServiceIdentity::new(service_id, service_id)
        .unwrap()
        .with_endpoints(vec![serde_json::from_value(json!({
            "endpoint":endpoint,
            "label":"Existing ACP fixture",
            "availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},
            "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}",collaboration_protocol::ACP_SCHEMA_DIGEST)}]
        })).unwrap()])
        .unwrap();
    let listener = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
    let manifest: collaboration_protocol::ServiceManifest = serde_json::from_value(json!({
        "version":2,
        "serviceId":service_id,
        "serviceEpoch":service_id,
        "machineLabel":"fixture-host",
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .unwrap();
    let publication = ManifestPublication::publish(&root, &manifest).unwrap();
    let stop = CancellationToken::new();
    let service = tokio::spawn(listener.run(stop.clone()));
    let acp = tokio::net::UnixListener::bind(root.join("acp.sock")).unwrap();
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.unwrap();
        let load: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(load["method"], "session/load");
        assert_eq!(
            load.pointer("/params/_meta/codex-router~1replayHistory"),
            Some(&json!(false))
        );
        for index in 0..=1024 {
            let update = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"existing-thread","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("setup-{index}")}}}});
            if writer
                .write_all(format!("{update}\n").as_bytes())
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = writer
            .write_all(
                format!("{}\n", json!({"jsonrpc":"2.0","id":load["id"],"result":{}})).as_bytes(),
            )
            .await;

        let prompt: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(prompt["method"], "session/prompt");
        writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"existing-thread","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"live detail"}}}})).as_bytes()).await.unwrap();
        writer
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}})
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let (stream, _) = acp.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.unwrap();
        let load: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(load["method"], "session/load");
        assert_eq!(
            load.pointer("/params/_meta/codex-router~1replayHistory"),
            Some(&json!(false))
        );
        writer
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":load["id"],"error":{"code":-32603,"message":"Native session setup rejected","data":{"kind":"protocolViolation","stage":"load","message":"ACP history replay exceeded the setup update limit"}}})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });

    let endpoint_ref: collaboration_protocol::EndpointRef =
        serde_json::from_value(endpoint).unwrap();
    let target: collaboration_protocol::SessionRef = serde_json::from_value(json!({
        "endpoint":endpoint_ref,
        "sessionId":"existing-thread"
    }))
    .unwrap();
    let result = AcpConversation::prompt_existing(
        &root,
        ExistingConversationPromptRequest {
            target,
            cwd: root.clone(),
            prompt: ConversationPromptRequest {
                message: PublicPromptContent::HumanUser {
                    text: MessageText::try_from("answer from old Host".to_owned()).unwrap(),
                },
                effort: None,
                timeout_seconds: 3,
            },
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let encoded_result = serde_json::to_value(&result).unwrap();
    assert_eq!(encoded_result["end"], "completed");
    assert_eq!(
        encoded_result["output"],
        json!({"kind":"unavailable","reason":"notRetained"})
    );
    assert!(encoded_result.get("updates").is_none());
    assert_eq!(encoded_result["result"]["stopReason"], "end_turn");

    let old_host_error = AcpConversation::prompt_existing(
        &root,
        ExistingConversationPromptRequest {
            target: serde_json::from_value(json!({
                "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
                "sessionId":"old-host-long-thread"
            }))
            .unwrap(),
            cwd: root.clone(),
            prompt: ConversationPromptRequest {
                message: PublicPromptContent::HumanUser {
                    text: MessageText::try_from("do not synthesize a settlement".to_owned())
                        .unwrap(),
                },
                effort: None,
                timeout_seconds: 3,
            },
        },
        CancellationToken::new(),
    )
    .await
    .expect_err("older Host load failure stays an operation failure");
    let (failure, failed_target, _) = old_host_error.into_parts();
    assert_eq!(failure.stage, "load");
    assert_eq!(
        failure.kind,
        collaboration_protocol::OperationFailureKind::Rejected
    );
    assert_eq!(failure.service_kind.as_deref(), Some("protocolViolation"));
    assert_eq!(
        failure.message,
        "ACP history replay exceeded the setup update limit"
    );
    assert_eq!(
        failure.effect,
        collaboration_protocol::OperationEffect::Unknown
    );
    assert_eq!(
        String::from(failed_target.unwrap().session_id),
        "old-host-long-thread"
    );

    peer.await.unwrap();
    stop.cancel();
    service.await.unwrap().unwrap();
    drop(publication);
    std::fs::remove_file(root.join("acp.sock")).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[tokio::test]
async fn aggregate_prompt_does_not_retain_more_than_one_thousand_updates() {
    let root = std::path::PathBuf::from(format!(
        "/tmp/acp-prompt-aggregate-bound-{}",
        std::process::id()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let service_id = "00000000-0000-4000-8000-000000000005";
    let digest = format!("sha256:{}", "c".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let identity=ServiceIdentity::new(service_id,service_id).unwrap().with_endpoints(vec![serde_json::from_value(json!({"endpoint":endpoint,"label":"ACP aggregate fixture","availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},"channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}",collaboration_protocol::ACP_SCHEMA_DIGEST)}]})).unwrap()]).unwrap();
    let listener = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
    let manifest:collaboration_protocol::ServiceManifest=serde_json::from_value(json!({"version":2,"serviceId":service_id,"serviceEpoch":service_id,"machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).unwrap();
    let publication = ManifestPublication::publish(&root, &manifest).unwrap();
    let stop = CancellationToken::new();
    let service = tokio::spawn(listener.run(stop.clone()));
    let acp = tokio::net::UnixListener::bind(root.join("acp.sock")).unwrap();
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        writer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.unwrap();
        let create: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        writer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":create["id"],"result":{"sessionId":"aggregate-thread"}})).as_bytes()).await.unwrap();
        let prompt: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        for index in 0..=1024 {
            let update = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"aggregate-thread","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("chunk-{index}")}}}});
            if writer
                .write_all(format!("{update}\n").as_bytes())
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = writer
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}})
                )
                .as_bytes(),
            )
            .await;
    });
    let endpoint_ref: collaboration_protocol::EndpointRef =
        serde_json::from_value(endpoint).unwrap();
    let sender: collaboration_protocol::SessionRef =
        serde_json::from_value(json!({"endpoint":endpoint_ref,"sessionId":"aggregate-sender"}))
            .unwrap();
    let result = AcpConversation::create_and_prompt(
        &root,
        ConversationCreatePromptRequest {
            create: ConversationCreateRequest {
                operation_id: collaboration_protocol::OperationId::generate(),
                endpoint: sender.endpoint.clone(),
                cwd: root.clone(),
                session: None,
                fork: None,
                model: Some("gpt-5.6-luna".to_owned()),
                effort: Some("low".to_owned()),
                access: Some("workspace-write".to_owned()),
                created_by: Some(sender.clone()),
                approver: Some(sender.clone()),
                root_message_id: None,
            },
            prompt: ConversationPromptRequest {
                message: PublicPromptContent::Agent {
                    sender,
                    text: MessageText::try_from("aggregate proof".to_owned()).unwrap(),
                },
                effort: Some("low".to_owned()),
                timeout_seconds: 3,
            },
        },
        CancellationToken::new(),
    )
    .await
    .expect("aggregate result does not retain streamed updates");
    let encoded_result = serde_json::to_value(&result).unwrap();
    assert_eq!(
        String::from(result.target.session_id.clone()),
        "aggregate-thread"
    );
    assert_eq!(encoded_result["end"], "completed");
    assert_eq!(
        encoded_result["output"],
        json!({"kind":"unavailable","reason":"notRetained"})
    );
    assert!(encoded_result.get("updates").is_none());
    assert_eq!(encoded_result["result"]["stopReason"], "end_turn");
    peer.await.unwrap();
    stop.cancel();
    service.await.unwrap().unwrap();
    drop(publication);
    std::fs::remove_file(root.join("acp.sock")).unwrap();
    std::fs::remove_dir(root).unwrap();
}
