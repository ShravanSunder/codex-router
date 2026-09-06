use communication_client::{AcpConversation, ConversationEnd, ConversationEvent};
use communication_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use serde_json::{Value, json};
use std::{os::unix::fs::DirBuilderExt, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

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
    let identity=ServiceIdentity::new(id,id,&digest).unwrap().with_endpoints(vec![serde_json::from_value(json!({"endpoint":endpoint,"label":"ACP fixture","availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},"channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}",communication_protocol::ACP_SCHEMA_DIGEST)}]})).unwrap()]).unwrap();
    let listener = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
    let manifest:communication_protocol::ServiceManifest=serde_json::from_value(json!({"version":1,"serviceId":id,"serviceEpoch":id,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest})).unwrap();
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
        .open_session(Some("owned"), &root, &mut emit)
        .await
        .unwrap();
    assert_eq!(
        client
            .prompt(
                "question",
                Duration::from_secs(3),
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
            .prompt("cancel this", Duration::from_secs(3), cancel, &mut emit)
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
