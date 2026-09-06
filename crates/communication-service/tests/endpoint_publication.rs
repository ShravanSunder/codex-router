use communication_protocol::{EndpointDescription, UuidIdentity};
use communication_service::EndpointDirectory;
use serde_json::json;

#[tokio::test]
async fn snapshots_and_subscribers_share_publication_order() {
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())
        .unwrap_or_else(|e| panic!("id: {e}"));
    let directory = EndpointDirectory::new(service_id);
    let mut first = directory
        .subscribe()
        .unwrap_or_else(|e| panic!("subscribe: {e}"));
    let endpoint: EndpointDescription = serde_json::from_value(json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"label":"Codex","availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":null}]})).unwrap_or_else(|e|panic!("endpoint: {e}"));
    directory
        .publish(endpoint.clone())
        .unwrap_or_else(|e| panic!("publish: {e}"));
    let snapshot = first.snapshot().unwrap_or_else(|e| panic!("snapshot: {e}"));
    assert_eq!(snapshot.sequence, 1);
    assert_eq!(snapshot.endpoints, vec![endpoint.clone()]);
    let update = first.next().await.unwrap_or_else(|e| panic!("event: {e}"));
    assert_eq!(update.sequence, 1);
    let second = directory
        .subscribe()
        .unwrap_or_else(|e| panic!("second: {e}"));
    assert_eq!(
        second
            .snapshot()
            .unwrap_or_else(|e| panic!("snapshot: {e}"))
            .sequence,
        0
    );
    directory
        .publish(endpoint)
        .unwrap_or_else(|e| panic!("publish: {e}"));
    assert_eq!(
        second
            .snapshot()
            .unwrap_or_else(|e| panic!("snapshot: {e}"))
            .sequence,
        1
    );
}
