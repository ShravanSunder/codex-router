//! Published CLI envelopes validate real protocol shapes, including upstream ACP content.
#[test]
fn cli_catalog_covers_closed_envelopes_and_pinned_acp_variants() {
    let schemas = collaboration_protocol::protocol_type_schemas().unwrap();
    let validator = |name| jsonschema::validator_for(schemas.get(name).unwrap()).unwrap();
    let finite = validator("FiniteCommandRecord");
    assert!(finite.is_valid(&serde_json::json!({"kind":"result","cliVersion":"0.1.27","serviceVersion":"0.1.27","result":{"page":{"records":[],"nextCursor":null}}})));
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
    assert!(conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"medium","effectiveAccess":"workspace-write","settingsObservation":{"kind":"observed","source":"threadStart","observedAt":"2026-09-16T00:00:00Z","routerAccess":"workspace-write","nativeSandbox":null,"permissionProfile":null,"approvalPolicy":null,"approvalsReviewer":null},"idleSeconds":0,"result":{"stopReason":"cancelled"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"medium","effectiveAccess":"workspace-write","settingsObservation":{"kind":"unavailable","reason":"threadReadOmitsSettings"},"idleSeconds":0,"result":{"stopReason":"interrupted"}})));
    // A thread with no recorded route still resumes; its receipt names that reason.
    assert!(conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"medium","effectiveAccess":null,"settingsObservation":{"kind":"unavailable","reason":"noRecordedAccessRoute"},"idleSeconds":0,"result":{"stopReason":"end_turn"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"medium","effectiveAccess":null,"settingsObservation":{"kind":"unavailable","reason":"inventedReason"},"idleSeconds":0,"result":{"stopReason":"end_turn"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"sessionUpdate","target":target,"update":{"sessionUpdate":"invented"}})));
    assert!(conversation.is_valid(&serde_json::json!({"kind":"conversationError","target":null,"stage":"connect","effect":"notDispatched","message":"Unavailable"})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"conversationError","stage":"connect","effect":"notDispatched","message":"Unavailable"})));
}
