use collaboration_service::{ServiceIdentity, SessionDeliveryRouter, serve_control_connection};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn message_to_missing_endpoint_reports_no_native_effects() {
    // Arrange: an initialized real Control socket without a backend.
    let id = "00000000-0000-4000-8000-000000000001";
    let identity = ServiceIdentity::new(id, id, &format!("sha256:{}", "a".repeat(64)))
        .unwrap()
        .with_session_delivery(Arc::new(SessionDeliveryRouter::new(vec![])));
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let task = tokio::spawn(serve_control_connection(server, identity));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    let target =
        json!({"endpoint":{"serviceId":id,"endpointId":"codex-local"},"sessionId":"thread"});
    let requests = [
        json!({"jsonrpc":"2.0","id":"init","method":"control/initialize","params":{"version":{"major":1,"minor":0},"client":{"name":"fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","id":"send","method":"message/send","params":{"target":target,"generationGuard":{"serviceEpoch":id,"generation":1},"message":{"kind":"agent","sender":target,"text":"information"}}}),
    ];
    // Act: submit through the actual dispatcher, not an error helper.
    let mut last = Value::Null;
    for request in requests {
        writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        last = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    }
    writer.shutdown().await.unwrap();
    task.await.unwrap().unwrap();
    // Assert: method exists and rejection truthfully establishes no effects.
    assert_eq!(last["result"]["outcome"]["kind"], "rejected");
    assert_eq!(last["result"]["outcome"]["reason"], "noRoute");
    assert!(last["result"]["reachability"].is_null());
    assert!(last["result"]["client"].is_null());
}
