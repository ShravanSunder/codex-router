use collaboration_client::{
    AcpConversation, ConversationCreatePromptRequest, ConversationCreateRequest, ConversationEnd,
    ConversationEvent, ConversationPromptRequest, PublicPromptContent,
};
use collaboration_protocol::{MessageText, SessionId};
use collaboration_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use serde_json::{Value, json};
use std::{os::unix::fs::DirBuilderExt, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn create_and_first_prompt_share_one_connection_and_return_correlated_settlement() {
    let root = std::path::PathBuf::from(format!(
        "/tmp/acp-create-prompt-flow-{}",
        std::process::id()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let service_id = "00000000-0000-4000-8000-000000000001";
    let digest = format!("sha256:{}", "a".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let identity=ServiceIdentity::new(service_id,service_id,&digest).unwrap().with_endpoints(vec![serde_json::from_value(json!({"endpoint":endpoint,"label":"ACP fixture","availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},"channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}",collaboration_protocol::ACP_SCHEMA_DIGEST)}]})).unwrap()]).unwrap();
    let listener = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
    let manifest:collaboration_protocol::ServiceManifest=serde_json::from_value(json!({"version":2,"serviceId":service_id,"serviceEpoch":service_id,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).unwrap();
    let manifest = ManifestPublication::publish(&root, &manifest).unwrap();
    let stop = CancellationToken::new();
    let service = tokio::spawn(listener.run(stop.clone()));
    let acp = tokio::net::UnixListener::bind(root.join("acp.sock")).unwrap();
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(initialize["method"], "initialize");
        writer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.unwrap();
        let create: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(create["method"], "session/new");
        writer
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":create["id"],"result":{"sessionId":"fresh-thread"}})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let prompt: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(prompt["method"], "session/prompt");
        let text = prompt
            .pointer("/params/prompt/0/text")
            .and_then(Value::as_str)
            .unwrap();
        assert!(text.contains("first-sender"));
        assert!(text.contains("first prompt"));
        writer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fresh-thread","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"FIRST_RESULT"}}}})).as_bytes()).await.unwrap();
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
    });
    let endpoint_ref: collaboration_protocol::EndpointRef =
        serde_json::from_value(endpoint).unwrap();
    let sender: collaboration_protocol::SessionRef = serde_json::from_value(json!({
        "endpoint":endpoint_ref,
        "sessionId":"first-sender"
    }))
    .unwrap();
    let result = AcpConversation::create_and_prompt(
        &root,
        ConversationCreatePromptRequest {
            create: ConversationCreateRequest {
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
                    text: MessageText::try_from("first prompt".to_owned()).unwrap(),
                },
                effort: Some("low".to_owned()),
                timeout_seconds: 3,
            },
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(String::from(result.target.session_id), "fresh-thread");
    assert_eq!(result.end, ConversationEnd::Completed);
    assert!(
        result
            .updates
            .iter()
            .any(|update| update.to_string().contains("FIRST_RESULT"))
    );
    peer.await.unwrap();
    stop.cancel();
    service.await.unwrap().unwrap();
    drop(manifest);
    std::fs::remove_file(root.join("acp.sock")).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[tokio::test]
async fn create_and_first_prompt_backend_rejection_retains_created_target() {
    let root = std::path::PathBuf::from(format!(
        "/tmp/acp-create-prompt-rejection-{}",
        std::process::id()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let service_id = "00000000-0000-4000-8000-000000000003";
    let digest = format!("sha256:{}", "b".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let identity=ServiceIdentity::new(service_id,service_id,&digest).unwrap().with_endpoints(vec![serde_json::from_value(json!({"endpoint":endpoint,"label":"ACP rejection fixture","availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},"channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}",collaboration_protocol::ACP_SCHEMA_DIGEST)}]})).unwrap()]).unwrap();
    let listener = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
    let manifest:collaboration_protocol::ServiceManifest=serde_json::from_value(json!({"version":2,"serviceId":service_id,"serviceEpoch":service_id,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).unwrap();
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
        assert_eq!(create["method"], "session/new");
        writer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":create["id"],"result":{"sessionId":"created-before-rejection"}})).as_bytes()).await.unwrap();
        let prompt: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(prompt["method"], "session/prompt");
        writer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":prompt["id"],"error":{"code":-32603,"message":"backend rejected first prompt","data":{"phase":"prompt"}}})).as_bytes()).await.unwrap();
    });
    let endpoint_ref: collaboration_protocol::EndpointRef =
        serde_json::from_value(endpoint).unwrap();
    let sender: collaboration_protocol::SessionRef =
        serde_json::from_value(json!({"endpoint":endpoint_ref,"sessionId":"sender"})).unwrap();
    let error = AcpConversation::create_and_prompt(
        &root,
        ConversationCreatePromptRequest {
            create: ConversationCreateRequest {
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
                    text: MessageText::try_from("valid first prompt".to_owned()).unwrap(),
                },
                effort: Some("low".to_owned()),
                timeout_seconds: 3,
            },
        },
        CancellationToken::new(),
    )
    .await
    .expect_err("backend prompt rejection must be returned");
    let target = error.target().expect("created target retained");
    assert_eq!(
        String::from(target.session_id.clone()),
        "created-before-rejection"
    );
    assert!(matches!(
        error,
        collaboration_client::ConversationCreatePromptError::AfterCreation {
            source: collaboration_client::ClientError::Rejected { code: -32603, .. },
            ..
        }
    ));
    peer.await.unwrap();
    stop.cancel();
    service.await.unwrap().unwrap();
    drop(publication);
    std::fs::remove_file(root.join("acp.sock")).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[tokio::test]
async fn reusable_acp_client_orders_load_updates_cancels_permissions_and_settles_cancel() {
    // Arrange: actual Unix discovery and a deterministic independent ACP peer.
    let root = std::path::PathBuf::from(format!("/tmp/acp-client-flow-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let id = "00000000-0000-4000-8000-000000000001";
    let digest = format!("sha256:{}", "a".repeat(64));
    let endpoint = json!({"serviceId":id,"endpointId":"codex-local"});
    let identity=ServiceIdentity::new(id,id,&digest).unwrap().with_endpoints(vec![serde_json::from_value(json!({"endpoint":endpoint,"label":"ACP fixture","availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},"channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}",collaboration_protocol::ACP_SCHEMA_DIGEST)}]})).unwrap()]).unwrap();
    let listener = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
    let manifest:collaboration_protocol::ServiceManifest=serde_json::from_value(json!({"version":2,"serviceId":id,"serviceEpoch":id,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).unwrap();
    let manifest = ManifestPublication::publish(&root, &manifest).unwrap();
    let stop = CancellationToken::new();
    let service = tokio::spawn(listener.run(stop.clone()));
    let acp = tokio::net::UnixListener::bind(root.join("acp.sock")).unwrap();
    let (prompt_started, receive_started) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        for (method, result) in [
            (
                "initialize",
                json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}),
            ),
            ("session/load", json!({})),
        ] {
            let frame: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(frame["method"], method);
            if method == "session/load" {
                writer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"owned","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"history"}}}})).as_bytes()).await.unwrap();
            }
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":frame["id"],"result":result})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        let prompt: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(prompt["method"], "session/prompt");
        writer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"sessionId":"owned","toolCall":{"toolCallId":"tool1","title":"Fixture action"},"options":[{"optionId":"allow","name":"Allow once","kind":"allow_once"}]}})).as_bytes()).await.unwrap();
        let permission: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(permission["id"], 7);
        assert_eq!(permission["result"]["outcome"]["outcome"], "cancelled");
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
        let prompt: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(prompt["method"], "session/prompt");
        let _signalled = prompt_started.send(());
        let cancel: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(cancel["method"], "session/cancel");
        assert_eq!(cancel["params"]["sessionId"], "owned");
        writer
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"cancelled"}})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    // Act: client initialization, load buffering, permission denial, same-connection cancellation.
    let mut client = AcpConversation::connect(&root, "codex-local".to_owned().try_into().unwrap())
        .await
        .unwrap();
    let mut events = Vec::new();
    let mut emit = |event| {
        events.push(event);
        Ok(())
    };
    client
        .open_session(
            &ConversationCreateRequest {
                endpoint: client.endpoint().clone(),
                cwd: root.clone(),
                session: Some(SessionId::try_from("owned".to_owned()).unwrap()),
                fork: None,
                model: None,
                effort: Some("medium".to_owned()),
                access: None,
                created_by: None,
                approver: None,
                root_message_id: None,
            },
            &mut emit,
        )
        .await
        .unwrap();
    assert_eq!(
        client
            .prompt_and_wait(
                ConversationPromptRequest {
                    message: PublicPromptContent::HumanUser {
                        text: MessageText::try_from("question".to_owned()).unwrap(),
                    },
                    effort: Some("medium".to_owned()),
                    timeout_seconds: 3,
                },
                CancellationToken::new(),
                &mut emit
            )
            .await
            .unwrap(),
        ConversationEnd::Completed
    );
    let cancel = CancellationToken::new();
    let cancel_after_start = cancel.clone();
    let canceller = tokio::spawn(async move {
        receive_started.await.unwrap();
        cancel_after_start.cancel();
    });
    assert_eq!(
        client
            .prompt_and_wait(
                ConversationPromptRequest {
                    message: PublicPromptContent::HumanUser {
                        text: MessageText::try_from("cancel this".to_owned()).unwrap(),
                    },
                    effort: Some("medium".to_owned()),
                    timeout_seconds: 3,
                },
                cancel,
                &mut emit
            )
            .await
            .unwrap(),
        ConversationEnd::Cancelled
    );
    canceller.await.unwrap();
    let already_cancelled = CancellationToken::new();
    already_cancelled.cancel();
    assert_eq!(
        client
            .prompt(
                "must not submit",
                Some("medium"),
                Duration::from_secs(3),
                already_cancelled,
                &mut emit
            )
            .await
            .unwrap(),
        ConversationEnd::Cancelled
    );
    drop(client);
    peer.await.unwrap();
    stop.cancel();
    service.await.unwrap().unwrap();
    drop(manifest);
    // Assert: setup updates appear only after ready; callback and terminals are explicit.
    assert!(matches!(
        events.first(),
        Some(ConversationEvent::SessionReady(_))
    ));
    assert!(matches!(
        events.get(1),
        Some(ConversationEvent::SessionUpdate { .. })
    ));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ConversationEvent::PermissionRequired(_)))
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, ConversationEvent::PromptResult { .. }))
            .count(),
        2
    );
    std::fs::remove_file(root.join("acp.sock")).unwrap();
    std::fs::remove_dir(root).unwrap();
}
