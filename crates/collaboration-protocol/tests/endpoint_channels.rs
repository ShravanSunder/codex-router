use collaboration_protocol::{EndpointDescription, control_schema_document};
use serde_json::{Value, json};

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

#[test]
fn unavailable_endpoint_has_no_transport_and_names_a_fix() {
    let endpoint = json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"label":"Claude Code","availability":{"state":"unavailable","observedAt":"2026-09-24T12:00:00Z","reason":"executable not found","fix":"install claude-agent-acp"},"channels":[]});
    let decoded: EndpointDescription =
        serde_json::from_value(endpoint.clone()).expect("unavailable provider with no transport");
    assert_eq!(serde_json::to_value(decoded).expect("serialize"), endpoint);
}

fn external_provider_endpoint() -> Value {
    json!({
        "endpoint": {
            "serviceId": "00000000-0000-4000-8000-000000000001",
            "endpointId": "claude-code"
        },
        "label": "Claude Code",
        "availability": {
            "state": "available",
            "observedAt": "2026-09-20T12:00:00Z"
        },
        "channels": [{
            "kind": "externalProvider",
            "transport": "stdioAcp",
            "runtime": {
                "provider": "claudeCode",
                "runtimeName": "claude-agent-acp",
                "runtimeVersion": "0.14.2"
            },
            "bindingId": "provider-binding-3",
            "bindingGeneration": 3,
            "capabilities": [
                {"name": "create", "status": "supported", "evidence": "observed"},
                {"name": "callerDetach", "status": "supported", "evidence": "routerQualified"}
            ]
        }]
    })
}

#[test]
fn external_provider_advertisement_is_typed_without_private_launch_details() {
    let description = external_provider_endpoint();
    let decoded: EndpointDescription = serde_json::from_value(description.clone())
        .unwrap_or_else(|error| panic!("external endpoint: {error}"));
    assert_eq!(
        serde_json::to_value(decoded).unwrap_or_else(|error| panic!("serialize: {error}")),
        description
    );

    for private_field in ["executable", "arguments", "environment", "path"] {
        let mut exposed = description.clone();
        exposed["channels"][0][private_field] = json!("private");
        assert!(
            serde_json::from_value::<EndpointDescription>(exposed).is_err(),
            "private field {private_field} must remain unrepresentable"
        );
    }
}

#[test]
fn endpoint_inventory_schema_publishes_external_provider_binding() {
    let schema = control_schema_document(None).unwrap_or_else(|error| panic!("schema: {error}"));
    let schema_reference = schema["x-methods"]["endpoint/list"]["response"]["$ref"]
        .as_str()
        .unwrap_or_else(|| panic!("endpoint response schema"));
    let mut selected = schema.clone();
    selected["$ref"] = json!(schema_reference);
    let validator =
        jsonschema::validator_for(&selected).unwrap_or_else(|error| panic!("validator: {error}"));
    let response = json!({
        "jsonrpc": "2.0",
        "id": "endpoint-list-1",
        "result": {
            "serviceEpoch": "00000000-0000-4000-8000-000000000002",
            "sequence": 4,
            "endpoints": [external_provider_endpoint()]
        }
    });
    assert!(validator.is_valid(&response));
}
