//! Owned ACP socket cleanup after listener shutdown.
use codex_acp_adapter::NativeStoredSessions;
use collaboration_service::{AcpChannelListener, NativeGenerationGate, UnmaterializedThreadHolder};
use std::{os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn acp_listener_cleans_its_owned_socket_after_shutdown() {
    use codex_native_integration::{NativePayloadSchemas, NativeSchemaBundle};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    // Arrange: schema-admitted fixture generation; initialize does not start native work.
    let root = std::path::PathBuf::from(format!("/tmp/acp-ready-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let mut defs = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ] {
        defs.insert(format!("{name}Params"), json!({"type":"object"}));
        defs.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle=NativeSchemaBundle::from_documents(BTreeMap::from([("codex_app_server_protocol.schemas.json".to_owned(),serde_json::to_vec(&json!({"definitions":{"v2":defs,"ServerRequest":{"type":"object"},"ServerNotification":{"type":"object"}}})).unwrap())])).unwrap();
    let schemas = Arc::new(NativePayloadSchemas::from_bundle(&bundle).unwrap());
    assert!(schemas.supports_server_messages());
    let gate = NativeGenerationGate::default();
    gate.activate(
        serde_json::from_value(
            json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
        )
        .unwrap(),
        root.join("backend.sock"),
        Some(schemas),
    )
    .unwrap();
    let path = root.join("codex-acp.sock");
    let listener = AcpChannelListener::bind(
        &path,
        gate.clone(),
        Arc::new(NativeStoredSessions::new(root.clone(), "fixture".into())),
        Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        Arc::new(UnmaterializedThreadHolder::new()),
        Arc::new(collaboration_service::UnavailableConversationOperationRecorder),
    )
    .unwrap();
    let stop = CancellationToken::new();
    let task = tokio::spawn(listener.run(stop.clone()));
    let client = tokio::net::UnixStream::connect(&path).await.unwrap();
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    // Act: real ACP JSONL handshake at the listener boundary.
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1,\"clientCapabilities\":{}}}\n").await.unwrap();
    let line = tokio::time::timeout(Duration::from_secs(3), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["result"]["protocolVersion"], 1);
    stop.cancel();
    task.await.unwrap().unwrap();
    // Assert: only owned listener removed, no backend process/socket created.
    assert!(!path.exists());
    assert!(!root.join("backend.sock").exists());
    std::fs::remove_dir(root).unwrap();
}
