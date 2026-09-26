use collaboration_protocol::{SessionMessageSendParams, control_schema_document};
use serde_json::{Value, json};

fn request() -> Value {
    let id = "00000000-0000-4000-8000-000000000001";
    let target =
        json!({"endpoint":{"serviceId":id,"endpointId":"codex-local"},"sessionId":"receiver"});
    json!({"target":target,"generationGuard":{"serviceEpoch":id,"generation":1},
        "message":{"kind":"agent","sender":target,"text":"A finding"}})
}

#[test]
fn agent_message_has_auto_delivery_and_rejects_legacy_or_ambiguous_input() {
    // Arrange: an agent message with omitted delivery.
    let mut value = request();
    // Act: decode through the public request type and serialize its resolved default.
    let parsed = serde_json::from_value::<SessionMessageSendParams>(value.clone())
        .expect("typed agent communication must be accepted");
    let encoded = serde_json::to_value(parsed).unwrap();
    // Assert: caller intent survives; explicit invalid variants and old input shape fail.
    assert_eq!(encoded["mode"], "auto");
    assert_eq!(encoded["message"]["kind"], "agent");
    value["message"]["text"] = json!("");
    assert!(serde_json::from_value::<SessionMessageSendParams>(value).is_err());
    let mut legacy = request();
    legacy.as_object_mut().unwrap().remove("message");
    legacy["input"] = json!([{"type":"text","text":"old input"}]);
    assert!(serde_json::from_value::<SessionMessageSendParams>(legacy).is_err());
    let mut human = request();
    human["message"]["kind"] = json!("humanUser");
    assert!(serde_json::from_value::<SessionMessageSendParams>(human.clone()).is_err());
    human["message"].as_object_mut().unwrap().remove("sender");
    assert!(serde_json::from_value::<SessionMessageSendParams>(human).is_ok());
}

#[test]
fn message_result_schema_preserves_closed_rejection_diagnostics() {
    let mut schema = control_schema_document(None).unwrap();
    schema["$ref"] = json!("#/$defs/message-send-response");
    let validator = jsonschema::validator_for(&schema).unwrap();
    let mut response = json!({"jsonrpc":"2.0","id":"send","result":{
        "outcome":{"kind":"rejected","reason":"childThread","nextAction":"inspectTarget","clientCode":-32000,"detail":null},
        "reachability":"codexAppServer","client":null
    }});
    assert!(validator.is_valid(&response));
    response["result"]["outcome"]
        .as_object_mut()
        .unwrap()
        .remove("nextAction");
    assert!(!validator.is_valid(&response));
    response["result"]["outcome"]["nextAction"] = json!("inspectTarget");
    response["result"]["outcome"]["reason"] = json!("inventedReason");
    assert!(!validator.is_valid(&response));
}
