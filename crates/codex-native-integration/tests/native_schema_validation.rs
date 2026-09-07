use codex_native_integration::NativeSchemaBundle;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn native_payload_validation_follows_references_and_rejects_missing_definition() {
    let source = json!({"definitions":{"v2":{
        "Payload":{"type":"object","required":["input"],"additionalProperties":false,
            "properties":{"input":{"$ref":"#/definitions/v2/Input"}}},
        "Input":{"type":"string","minLength":1}
    }}});
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&source).unwrap_or_else(|error| panic!("source: {error}")),
    )]))
    .unwrap_or_else(|error| panic!("bundle: {error}"));
    let validator = bundle
        .validator_for_v2("Payload")
        .unwrap_or_else(|error| panic!("validator: {error}"));
    assert!(validator.is_valid(&json!({"input":"hello"})));
    assert!(!validator.is_valid(&json!({"input":""})));
    assert!(!validator.is_valid(&json!({"input":12})));
    assert!(!validator.is_valid(&json!({"input":"hello","extra":true})));
    assert!(!validator.is_valid(&json!({})));
    assert!(bundle.validator_for_v2("Missing").is_err());
    assert!(bundle.validator_for_v2("../Payload").is_err());
}

#[test]
fn native_envelope_validation_resolves_top_level_and_v2_references() {
    // Arrange: a root envelope references a v2 payload, as native exports do.
    let source = json!({"definitions":{
        "ServerNotification":{"type":"object","required":["method","params"],"additionalProperties":false,
            "properties":{"method":{"const":"fixture/update"},"params":{"$ref":"#/definitions/v2/Update"}}},
        "v2":{"Update":{"type":"object","required":["threadId"],"properties":{"threadId":{"type":"string"}}}}
    }});
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&source).unwrap(),
    )]))
    .unwrap();
    // Act.
    let validator = bundle.validator_for_root("ServerNotification").unwrap();
    // Assert: malformed methods, payloads and unsolicited IDs fail at the envelope boundary.
    assert!(validator.is_valid(&json!({"method":"fixture/update","params":{"threadId":"one"}})));
    assert!(!validator.is_valid(&json!({"method":"fixture/update","params":{"threadId":42}})));
    assert!(!validator.is_valid(&json!({"method":"other/update","params":{"threadId":"one"}})));
    assert!(
        !validator.is_valid(&json!({"id":1,"method":"fixture/update","params":{"threadId":"one"}}))
    );
    assert!(bundle.validator_for_root("../ServerNotification").is_err());
}
