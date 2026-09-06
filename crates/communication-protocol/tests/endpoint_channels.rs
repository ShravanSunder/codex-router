use communication_protocol::EndpointDescription;
use serde_json::json;

#[test]
fn endpoint_has_two_protocol_channels_without_two_identities() {
    let description = json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"label":"Local Codex","availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":null},{"kind":"acp","transport":"unixJsonLines","path":"codex-acp.sock","schemaDigest":format!("sha256:{}","a".repeat(64))}]});
    let value: EndpointDescription = serde_json::from_value(description.clone())
        .unwrap_or_else(|error| panic!("valid: {error}"));
    assert_eq!(
        serde_json::to_value(value).unwrap_or_else(|error| panic!("serialize: {error}")),
        description
    );
}

#[test]
fn invalid_channel_carrier_and_empty_channels_are_rejected() {
    let base = json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"label":"Local Codex","availability":{"state":"unprobed"},"channels":[]});
    assert!(serde_json::from_value::<EndpointDescription>(base.clone()).is_err());
    let mut wrong = base;
    wrong["channels"] = json!([{"kind":"nativeCodex","transport":"unixJsonLines","path":"x","schemaDigest":null,"generation":null}]);
    assert!(serde_json::from_value::<EndpointDescription>(wrong).is_err());
}
