use collaboration_protocol::{EndpointDescription, UuidIdentity};
use collaboration_service::EndpointDirectory;
use serde_json::json;

fn codex_endpoint(service_id: &str) -> Result<EndpointDescription, serde_json::Error> {
    serde_json::from_value(
        json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"label":"Codex","availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":null}]}),
    )
}

#[test]
fn the_inventory_counts_every_publication_and_holds_the_latest_description() {
    // Arrange
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())
        .unwrap_or_else(|e| panic!("id: {e}"));
    let directory = EndpointDirectory::new(service_id);
    let endpoint = codex_endpoint("00000000-0000-4000-8000-000000000001")
        .unwrap_or_else(|e| panic!("endpoint: {e}"));
    let reader = directory
        .subscribe()
        .unwrap_or_else(|e| panic!("reader: {e}"));

    // Act
    directory
        .publish(endpoint.clone())
        .unwrap_or_else(|e| panic!("publish: {e}"));
    let first = directory
        .inventory()
        .unwrap_or_else(|e| panic!("inventory: {e}"));
    directory
        .publish(endpoint.clone())
        .unwrap_or_else(|e| panic!("republish: {e}"));
    let foreign = directory.publish(
        codex_endpoint("00000000-0000-4000-8000-000000000009")
            .unwrap_or_else(|e| panic!("endpoint: {e}")),
    );
    let second = reader
        .snapshot()
        .unwrap_or_else(|e| panic!("snapshot: {e}"));

    // Assert
    assert_eq!(first.sequence, 1);
    assert_eq!(first.endpoints, vec![endpoint.clone()]);
    assert!(foreign.is_err(), "another service's endpoint was published");
    assert_eq!(second.sequence, 2);
    assert_eq!(second.endpoints, vec![endpoint]);
}
