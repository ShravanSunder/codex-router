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
    assert!(finite.is_valid(&serde_json::json!({
        "kind":"error",
        "target":target,
        "error":{"kind":"unavailable","effect":"unknown"}
    })));
    let observation = validator("NativeObservationRecord");
    assert!(observation.is_valid(&serde_json::json!({"kind":"listenerReady","target":target,"generation":{"serviceEpoch":id,"generation":1}})));
    assert!(
        !observation.is_valid(&serde_json::json!({"kind":"connectionClosed","reason":"finished"}))
    );
    let conversation = validator("ConversationRecord");
    assert!(conversation.is_valid(&serde_json::json!({"kind":"sessionUpdate","target":target,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}})));
    assert!(conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"medium","effectiveAccess":"workspace-write","settingsObservation":{"kind":"observed","source":"threadStart","observedAt":"2026-09-16T00:00:00Z","routerAccess":"workspace-write","nativeSandbox":null,"permissionProfile":null,"approvalPolicy":null,"approvalsReviewer":null},"idleSeconds":0,"result":{"stopReason":"cancelled"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"medium","effectiveAccess":"workspace-write","settingsObservation":{"kind":"unavailable","reason":"threadReadOmitsSettings"},"idleSeconds":0,"result":{"stopReason":"interrupted"}})));
    // A resume that changes the effort reports it without refusing the turn.
    assert!(conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"high","effectiveAccess":"workspace-write","settingsObservation":{"kind":"unavailable","reason":"threadReadOmitsSettings"},"effortChange":{"previous":"medium","requested":"high"},"idleSeconds":0,"result":{"stopReason":"end_turn"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"high","effectiveAccess":"workspace-write","settingsObservation":{"kind":"unavailable","reason":"threadReadOmitsSettings"},"effortChange":{"previous":"medium"},"idleSeconds":0,"result":{"stopReason":"end_turn"}})));
    // A thread with no recorded route still resumes; its receipt names that reason.
    assert!(conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"medium","effectiveAccess":null,"settingsObservation":{"kind":"unavailable","reason":"noRecordedAccessRoute"},"idleSeconds":0,"result":{"stopReason":"end_turn"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"promptResult","target":target,"effectiveModel":"gpt-5.6-sol","effectiveEffort":"medium","effectiveAccess":null,"settingsObservation":{"kind":"unavailable","reason":"inventedReason"},"idleSeconds":0,"result":{"stopReason":"end_turn"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"sessionUpdate","target":target,"update":{"sessionUpdate":"invented"}})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"conversationError","target":null,"stage":"connect","effect":"notDispatched","message":"Unavailable"})));
    assert!(!conversation.is_valid(&serde_json::json!({"kind":"conversationError","target":null,"error":{"kind":"unavailable"}})));

    // These are the machine records emitted after the normal ACP event stream.
    // Keep the export bound to that real CLI contract rather than a stale
    // fixture-only subset of its variants.
    assert!(conversation.is_valid(&serde_json::json!({
        "kind":"conversationCreated",
        "target":target,
    })));
    assert!(conversation.is_valid(&serde_json::json!({
        "kind":"conversationSettlement",
        "target":target,
        "terminalReason":"cancelled",
        "result":{"stopReason":"cancelled"},
    })));
    assert!(conversation.is_valid(&serde_json::json!({
        "kind":"conversationSettlement",
        "target":target,
        "terminalReason":"timedOut",
        "result":{"stopReason":"cancelled"},
    })));
    assert!(conversation.is_valid(&serde_json::json!({
        "kind":"conversationError",
        "target":null,
        "error":{
            "kind":"unavailable",
            "serviceKind":null,
            "stage":"transport",
            "effect":"unknown",
            "message":"connection lost",
            "code":null,
            "data":null,
        },
    })));
}

/// One real sample per command kind, and no wrapper key left under `result`.
#[test]
fn published_envelopes_carry_pages_records_and_effects_without_wrapper_keys() {
    // Arrange: the published envelope validator and one sample per kind.
    let schemas = collaboration_protocol::protocol_type_schemas().unwrap();
    let envelope = jsonschema::validator_for(schemas.get("FiniteCommandRecord").unwrap()).unwrap();
    let message = serde_json::json!({
        "messageId":"01a0a9aa-0393-7a30-aeca-c7c77d679774",
        "boardId":"01a0a9aa-0393-7a30-aeca-c7c77d679775",
        "topicId":"01a0a9aa-0393-7a30-aeca-c7c77d679776",
        "text":"a finding"
    });
    let thread = serde_json::json!({
        "rootMessageId":"01a0a9aa-0393-7a30-aeca-c7c77d679774",
        "state":"open",
        "watchStatus":{"watching":true}
    });
    let listen = serde_json::json!({"listenId":"01a0a9aa-0393-7a30-aeca-c7c77d679777"});
    let samples = [
        (
            "list",
            serde_json::json!({"page":{"records":[&message],"nextCursor":null}}),
        ),
        ("messageShow", serde_json::json!({"record":message})),
        ("threadShow", serde_json::json!({"record":thread})),
        ("listenShow", serde_json::json!({"record":listen})),
        (
            "mutation",
            serde_json::json!({"record":{"rootMessageId":"01a0a9aa-0393-7a30-aeca-c7c77d679774"},
                "effects":{"outcome":"joined"}}),
        ),
        // Mutations whose wire type carries no effect field still publish effects.
        (
            "sessionRename",
            serde_json::json!({"record":{"name":"🔎 Review","previousName":"Old name"},
                "effects":{"previousName":"Old name"}}),
        ),
        (
            "turnInterrupt",
            serde_json::json!({"record":{"turnId":"turn-a","kind":"interruptCompleted"},
                "effects":{"kind":"interruptCompleted"}}),
        ),
        (
            "listenCancel",
            serde_json::json!({"record":&listen,"effects":{"reason":"cancelled"}}),
        ),
    ];

    for (kind, result) in samples {
        // Act.
        let published = serde_json::json!({"kind":"result","cliVersion":"0.1.28","serviceVersion":"0.1.28","result":result});

        // Assert: the envelope validates and names no entity wrapper.
        assert!(envelope.is_valid(&published), "{kind} envelope");
        for wrapper in ["message", "thread", "listen", "sessions"] {
            assert!(
                published.pointer(&format!("/result/{wrapper}")).is_none(),
                "{kind}: result must not wrap its entity in {wrapper}"
            );
            assert!(
                published
                    .pointer(&format!("/result/record/{wrapper}"))
                    .is_none(),
                "{kind}: the record must be the entity, not a {wrapper} wrapper"
            );
        }
    }

    // Assert: a single read's result schema is the entity's own, with no
    // wrapper property standing between the record and its fields.
    let document = collaboration_protocol::control_schema_document(None).unwrap();
    let definitions = serde_json::to_string(&document).unwrap();
    for wrapper in [
        r#""MessageShowResult":{"properties":{"message""#,
        r#""ThreadShowResult":{"properties":{"thread""#,
        r#""ThreadListenShowResult":{"properties":{"listen""#,
        r#""ThreadListenCancelResult":{"properties":{"listen""#,
        r#""ThreadListenResult":{"properties":{"listen""#,
    ] {
        assert!(
            !definitions.contains(wrapper),
            "a single read must publish the entity, not {wrapper}"
        );
    }
}
