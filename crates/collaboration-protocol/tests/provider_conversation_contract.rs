use collaboration_protocol::{
    ConversationBindingIdentity, ConversationCreateOutcome, ConversationCreateRequest,
    ConversationOperationFailure, ConversationOperationSettlement, ConversationOperationSnapshot,
    ConversationOperationWaitResult, ConversationPromptRequest,
};
use serde_json::{Value, json};

/// The JSON Schema a conversation tool publishes for `TWire`, as a validator.
fn type_validator<TWire: schemars::JsonSchema>() -> Result<jsonschema::Validator, String> {
    let schema =
        serde_json::to_value(schemars::schema_for!(TWire)).map_err(|error| error.to_string())?;
    jsonschema::validator_for(&schema).map_err(|error| error.to_string())
}

fn endpoint() -> Value {
    json!({
        "serviceId": "019f0000-0000-7000-8000-000000000001",
        "endpointId": "claude-code"
    })
}

fn session(session_id: &str) -> Value {
    json!({
        "endpoint": endpoint(),
        "sessionId": session_id
    })
}

fn generation() -> Value {
    json!({
        "serviceEpoch": "019f0000-0000-7000-8000-000000000002",
        "generation": 3
    })
}

fn operation_snapshot() -> Value {
    json!({
        "operationId": "019f0000-0000-7000-8000-000000000011",
        "operation": "conversationPrompt",
        "binding": {"kind": "externalProvider", "binding": {
            "endpoint": endpoint(),
            "bindingId": "provider-binding-3",
            "runtime": {
                "provider": "claudeCode",
                "runtimeName": "claude-agent-acp",
                "runtimeVersion": "0.14.2"
            },
            "transport": "stdioAcp",
            "generation": generation(),
            "capabilities": [
                {"name": "prompt", "status": "supported", "evidence": "observed"},
                {"name": "callerDetach", "status": "supported", "evidence": "routerQualified"}
            ]
        }},
        "target": session("provider-conversation"),
        "stage": "terminal",
        "effect": "applied",
        "reconciliation": "confirmed",
        "admittedAt": "2026-09-20T12:00:00Z",
        "terminalAt": "2026-09-20T12:00:01Z"
    })
}

#[test]
fn conversation_binding_identity_round_trips_both_routes() -> Result<(), Box<dyn std::error::Error>>
{
    let external = operation_snapshot()["binding"].clone();
    let decoded: ConversationBindingIdentity = serde_json::from_value(external.clone())?;
    if serde_json::to_value(decoded)? != external {
        return Err("external binding changed in the round trip".into());
    }

    let codex = json!({
        "kind": "codexAcp",
        "endpoint": {"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},
        "listenerPath": "/tmp/codex-acp.sock",
        "generation": generation()
    });
    let decoded: ConversationBindingIdentity = serde_json::from_value(codex.clone())?;
    if serde_json::to_value(decoded)? != codex {
        return Err("Codex binding changed in the round trip".into());
    }
    if serde_json::from_value::<ConversationBindingIdentity>(json!({
        "kind":"codexAcp","endpoint":endpoint(),"listenerPath":"","generation":generation()
    }))
    .is_ok()
    {
        return Err("empty Codex listener path was accepted".into());
    }
    Ok(())
}

#[test]
fn conversation_create_outcome_keeps_caller_operation_identity()
-> Result<(), Box<dyn std::error::Error>> {
    for value in [
        json!({"kind":"created","operationId":"019f0000-0000-7000-8000-000000000011","target":session("created")}),
        json!({"kind":"created","operationId":"019f0000-0000-7000-8000-000000000011","target":session("created"),
            "effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},
                "mappingStatus":"verified","authentication":"authenticated","mode":"ask","model":"provider-model","effort":"high"}}),
        json!({"kind":"pending","operationId":"019f0000-0000-7000-8000-000000000011"}),
    ] {
        let outcome: ConversationCreateOutcome = serde_json::from_value(value.clone())?;
        if serde_json::to_value(outcome)? != value {
            return Err("conversation create outcome changed in round trip".into());
        }
    }
    Ok(())
}

#[test]
fn mutations_require_caller_operation_identity_and_allow_unpinned_generation() {
    let create_validator = type_validator::<ConversationCreateRequest>()
        .unwrap_or_else(|error| panic!("validator: {error}"));
    let actor = session("creator-session");
    let mut request = json!({
        "operationId": "019f0000-0000-7000-8000-000000000010",
        "endpoint": actor["endpoint"],
        "generation": generation(),
        "workingDirectory": "/tmp/provider-work",
        "createdBy": actor,
        "approver": session("approver-session"),
        "requestedPolicy": {"access": "workspace-write"}
    });
    assert!(create_validator.is_valid(&request));

    request
        .as_object_mut()
        .unwrap_or_else(|| panic!("params"))
        .remove("operationId");
    assert!(!create_validator.is_valid(&request));
    request["operationId"] = json!("server-generated-later");
    assert!(!create_validator.is_valid(&request));
    request["operationId"] = json!("019f0000-0000-7000-8000-000000000010");
    request
        .as_object_mut()
        .unwrap_or_else(|| panic!("params"))
        .remove("generation");
    assert!(create_validator.is_valid(&request));
}

#[test]
fn provider_create_settings_and_partial_settlement_have_additive_typed_shapes() {
    let request = json!({
        "operationId":"019f0000-0000-7000-8000-000000000010",
        "endpoint":endpoint(),
        "workingDirectory":"/tmp/provider-work",
        "createdBy":session("creator"),
        "approver":session("approver"),
        "requestedPolicy":{"access":"workspace-write"},
        "settings":{"mode":"plan","model":"provider-model","effort":"high"}
    });
    let decoded: ConversationCreateRequest =
        serde_json::from_value(request.clone()).expect("provider settings request");
    assert_eq!(serde_json::to_value(decoded).expect("round trip"), request);
    let settlement = json!({
        "kind":"createdWithoutSettings", "target":session("created"),
        "applied":[{"setting":"mode","value":"plan"}],
        "failed":[{"setting":"model","value":"provider-model","reason":"agent refused"}],
        "notApplied":[{"setting":"effort","value":"high"}]
    });
    let decoded: ConversationOperationSettlement =
        serde_json::from_value(settlement.clone()).expect("partial settlement");
    assert_eq!(
        serde_json::to_value(decoded).expect("round trip"),
        settlement
    );
    let outcome = json!({
        "kind":"createdWithoutSettings",
        "operationId":"019f0000-0000-7000-8000-000000000010",
        "target":session("created"),
        "applied":[{"setting":"mode","value":"plan"}],
        "failed":[{"setting":"model","value":"provider-model","reason":"agent refused"}],
        "notApplied":[{"setting":"effort","value":"high"}]
    });
    let decoded: ConversationCreateOutcome =
        serde_json::from_value(outcome.clone()).expect("partial outcome with untried setting");
    assert_eq!(serde_json::to_value(decoded).expect("round trip"), outcome);
}

#[test]
fn invalid_setting_failure_names_advertised_values_and_session_disposition() {
    let failure = json!({
        "kind":"invalidSetting", "stage":"settlement", "effect":"none",
        "message":"mode value invalid; advertised: plan, default; Session closed",
        "operationId":"019f0000-0000-7000-8000-000000000011",
        "invalidSetting":{
            "setting":"mode", "value":"wrong", "advertised":["plan","default"],
            "sessionDisposition":"closed"
        }
    });
    let decoded: ConversationOperationFailure =
        serde_json::from_value(failure.clone()).expect("typed invalid setting");
    assert_eq!(serde_json::to_value(decoded).expect("round trip"), failure);
}

#[test]
fn provider_prompt_input_id_is_additive_and_kept_verbatim() {
    let base = json!({
        "operationId":"019f0000-0000-7000-8000-000000000011",
        "target":session("provider-session"),
        "requestedBy":session("creator"),
        "approver":session("approver"),
        "prompt":{"kind":"router","text":"hello"}
    });
    let old: ConversationPromptRequest = serde_json::from_value(base.clone()).expect("old prompt");
    assert_eq!(serde_json::to_value(old).expect("old round trip"), base);
    let mut with_id = base;
    with_id["inputId"] = json!("input-from-front-door");
    let prompt: ConversationPromptRequest =
        serde_json::from_value(with_id.clone()).expect("typed Input ID");
    assert_eq!(
        prompt
            .input_id
            .as_ref()
            .map(session_event_model::InputId::as_str),
        Some("input-from-front-door")
    );
    assert_eq!(serde_json::to_value(prompt).expect("round trip"), with_id);
}

#[test]
fn failures_preserve_known_target_and_operation_effect_evidence() {
    let validator = type_validator::<ConversationOperationFailure>()
        .unwrap_or_else(|error| panic!("validator: {error}"));
    let mut failure = json!({
        "kind": "outcomeUnknown",
        "stage": "settlement",
        "effect": "unknown",
        "message": "provider response was lost",
        "operationId": "019f0000-0000-7000-8000-000000000011",
        "target": session("provider-conversation")
    });
    assert!(validator.is_valid(&failure));

    failure
        .as_object_mut()
        .unwrap_or_else(|| panic!("failure data"))
        .remove("effect");
    assert!(!validator.is_valid(&failure));
}

#[test]
fn unavailable_conversation_failure_carries_catalog_recovery_and_legacy_errors_decode() {
    let validator = type_validator::<ConversationOperationFailure>()
        .unwrap_or_else(|error| panic!("validator: {error}"));
    let availability = json!({
        "state":"unavailable","observedAt":"2026-09-24T00:00:00Z",
        "reason":"provider executable is missing","fix":"install the provider binary"
    });
    let data = json!({
        "kind":"unavailable","stage":"binding","effect":"none",
        "message":"provider conversation endpoint claude-code unavailable",
        "operationId":"019f0000-0000-7000-8000-000000000011",
        "endpoint":endpoint(),"availability":availability
    });
    let failure: ConversationOperationFailure =
        serde_json::from_value(data.clone()).expect("typed failure");
    assert_eq!(serde_json::to_value(failure).expect("round trip"), data);
    assert!(validator.is_valid(&data));

    let mut legacy = data;
    legacy
        .as_object_mut()
        .expect("legacy error")
        .remove("endpoint");
    legacy
        .as_object_mut()
        .expect("legacy error")
        .remove("availability");
    let decoded: ConversationOperationFailure =
        serde_json::from_value(legacy.clone()).expect("legacy failure remains readable");
    assert_eq!(
        serde_json::to_value(decoded).expect("legacy round trip"),
        legacy
    );
}

#[test]
fn inspection_and_wait_keep_durable_metadata_separate_from_ephemeral_output() {
    let show_validator = type_validator::<ConversationOperationSnapshot>()
        .unwrap_or_else(|error| panic!("validator: {error}"));
    let show_response = operation_snapshot();
    assert!(show_validator.is_valid(&show_response));

    let wait_validator = type_validator::<ConversationOperationWaitResult>()
        .unwrap_or_else(|error| panic!("validator: {error}"));
    let wait_response = json!({
        "operation": operation_snapshot(),
        "output": {
            "kind": "available",
            "settlement": {
                "kind": "promptCompleted",
                "target": session("provider-conversation"),
                "stopReason": "end_turn",
                "response": "ephemeral provider reply"
            }
        }
    });
    let wait_errors = wait_validator
        .iter_errors(&wait_response)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(wait_errors.is_empty(), "{}", wait_errors.join("; "));

    let mut invalid_show = show_response;
    invalid_show["prompt"] = json!("must never be durable metadata");
    assert!(!show_validator.is_valid(&invalid_show));
}

#[test]
fn output_limit_reasons_decode_strictly_and_validate_in_operation_wait() {
    let wait_validator = type_validator::<ConversationOperationWaitResult>()
        .unwrap_or_else(|error| panic!("validator: {error}"));

    for reason in ["outputLimitExceeded", "outputInvalid"] {
        let typed_reason: collaboration_protocol::ConversationOutputUnavailableReason =
            serde_json::from_value(json!(reason)).expect("new output reason decodes");
        assert_eq!(serde_json::to_value(typed_reason).unwrap(), json!(reason));

        let response = json!({
            "operation":operation_snapshot(),
            "output":{"kind":"outputUnavailable","reason":reason}
        });
        assert!(
            wait_validator.is_valid(&response),
            "conversation/operationWait rejected {reason}"
        );
    }

    assert!(
        serde_json::from_value::<collaboration_protocol::ConversationOutputUnavailableReason>(
            json!("futureReason")
        )
        .is_err()
    );
}

#[test]
fn validated_provider_collections_and_paths_reject_ambiguous_values() {
    let duplicate_capabilities = json!([
        {"name": "prompt", "status": "supported", "evidence": "advertised"},
        {"name": "prompt", "status": "supported", "evidence": "observed"}
    ]);
    assert!(
        serde_json::from_value::<collaboration_protocol::ProviderCapabilities>(
            duplicate_capabilities
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<collaboration_protocol::ProviderWorkingDirectory>(json!(
            "relative/path"
        ))
        .is_err()
    );
}

#[test]
fn unknown_stop_reason_round_trips_with_completed_prompt_output() {
    let settlement = json!({
        "kind":"promptCompleted",
        "target":session("provider-conversation"),
        "stopReason":{"unknown":"future_reason"},
        "response":"first second"
    });
    let decoded: collaboration_protocol::ConversationOperationSettlement =
        serde_json::from_value(settlement.clone()).expect("typed unknown settlement");
    assert_eq!(
        serde_json::to_value(decoded).expect("encode settlement"),
        settlement
    );
}
