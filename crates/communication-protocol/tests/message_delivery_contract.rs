use communication_protocol::{NativeSendParams, control_schema_document};
use serde_json::{Value, json};

fn request() -> Value {
    let id = "00000000-0000-4000-8000-000000000001";
    let target =
        json!({"endpoint":{"serviceId":id,"endpointId":"codex-local"},"sessionId":"receiver"});
    json!({"target":target,"generation":{"serviceEpoch":id,"generation":1},
        "message":{"kind":"agent","sender":target,"text":"A finding"}})
}

#[test]
fn agent_message_has_auto_delivery_and_rejects_legacy_or_ambiguous_input() {
    // Arrange: an agent message with omitted delivery.
    let mut value = request();
    // Act: decode through the public request type and serialize its resolved default.
    let parsed = serde_json::from_value::<NativeSendParams>(value.clone())
        .expect("typed agent communication must be accepted");
    let encoded = serde_json::to_value(parsed).unwrap();
    // Assert: caller intent survives; explicit invalid variants and old input shape fail.
    assert_eq!(encoded["delivery"], "auto");
    assert_eq!(encoded["message"]["kind"], "agent");
    value["message"]["text"] = json!("");
    assert!(serde_json::from_value::<NativeSendParams>(value).is_err());
    let mut legacy = request();
    legacy.as_object_mut().unwrap().remove("message");
    legacy["input"] = json!([{"type":"text","text":"old input"}]);
    assert!(serde_json::from_value::<NativeSendParams>(legacy).is_err());
    let mut human = request();
    human["message"]["kind"] = json!("humanUser");
    assert!(serde_json::from_value::<NativeSendParams>(human.clone()).is_err());
    human["message"].as_object_mut().unwrap().remove("sender");
    assert!(serde_json::from_value::<NativeSendParams>(human).is_ok());
}

#[test]
fn message_error_schema_requires_partial_effects() {
    // Arrange: a known resume followed by rejected submission.
    let mut schema = control_schema_document(None).unwrap();
    schema["$ref"] = json!("#/$defs/codex-messageSend-error");
    let validator = jsonschema::validator_for(&schema).unwrap();
    let mut response = json!({"jsonrpc":"2.0","id":"send","error":{"code":-32050,"message":"Rejected","data":{
        "kind":"nativeRejected","stage":"start","message":"Rejected",
        "effects":{"resume":"accepted","submission":"rejected"},"clientUserMessageId":"correlation"
    }}});
    // Act / Assert: preserve both effects; a message-only error cannot hide resume.
    assert!(validator.is_valid(&response));
    response["error"]["data"]
        .as_object_mut()
        .unwrap()
        .remove("effects");
    assert!(!validator.is_valid(&response));
}
