//! Published CLI envelopes validate real protocol shapes, including upstream ACP content.
#[test]
fn cli_catalog_covers_closed_envelopes_and_pinned_acp_variants() {
    let schemas = communication_protocol::protocol_type_schemas().unwrap();
    let validator = |name| jsonschema::validator_for(schemas.get(name).unwrap()).unwrap();
    let finite = validator("FiniteCommandRecord");
    assert!(finite.is_valid(&serde_json::json!({"kind":"result","result":{"sessions":[]}})));
    assert!(!finite.is_valid(&serde_json::json!({"kind":"result","result":{},"unexpected":true})));
    let id = "00000000-0000-4000-8000-000000000001";
    let target = serde_json::json!({"endpoint":{"serviceId":id,"endpointId":"codex-local"},"sessionId":"thread"});
    let observation = validator("NativeObservationRecord");
    assert!(observation.is_valid(&serde_json::json!({"kind":"listenerReady","target":target,"generation":{"serviceEpoch":id,"generation":1}})));
    assert!(
        !observation.is_valid(&serde_json::json!({"kind":"connectionClosed","reason":"finished"}))
    );
    let conversation = validator("ConversationRecord");
    assert!(conversation.is_valid(&serde_json::json!({"kind":"sessionUpdate","target":target,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}})));
    assert!(conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"result":{"stopReason":"cancelled"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"result":{"stopReason":"interrupted"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"sessionUpdate","target":target,"update":{"sessionUpdate":"invented"}})));
    assert!(conversation.is_valid(&serde_json::json!({"kind":"conversationError","target":null,"stage":"connect","effect":"notDispatched","message":"Unavailable"})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"conversationError","stage":"connect","effect":"notDispatched","message":"Unavailable"})));
}
