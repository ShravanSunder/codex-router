use collaboration_protocol::{
    ConversationBindingIdentity, ConversationCreateOutcome, ConversationOperationFailure,
    control_error_is_valid, control_schema_document,
};
use serde_json::{Value, json};

fn method_validator(
    schema: &Value,
    method: &str,
    pairing: &str,
) -> Result<jsonschema::Validator, String> {
    let schema_reference = schema
        .get("x-methods")
        .and_then(|methods| methods.get(method))
        .and_then(|contract| contract.get(pairing))
        .and_then(|reference| reference.get("$ref"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {method} {pairing} reference"))?;
    let mut selected = schema.clone();
    selected
        .as_object_mut()
        .ok_or_else(|| "schema document must be an object".to_owned())?
        .insert("$ref".to_owned(), json!(schema_reference));
    jsonschema::validator_for(&selected).map_err(|error| error.to_string())
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
fn external_conversation_methods_have_closed_typed_pairings() {
    let schema = control_schema_document(None).unwrap_or_else(|error| panic!("schema: {error}"));
    let methods = schema["x-methods"]
        .as_object()
        .unwrap_or_else(|| panic!("method map"));

    for method in [
        "conversation/create",
        "conversation/load",
        "conversation/prompt",
        "conversation/cancel",
        "conversation/operationShow",
        "conversation/operationWait",
        "conversation/operationReconcile",
    ] {
        assert!(methods.contains_key(method), "missing {method}");
        for pairing in ["params", "result", "request", "response", "error"] {
            let pointer = methods[method][pairing]["$ref"]
                .as_str()
                .unwrap_or_else(|| panic!("missing {method} {pairing}"));
            assert!(
                schema.pointer(pointer.trim_start_matches('#')).is_some(),
                "unresolved {method} {pairing}"
            );
        }
    }
}

#[test]
fn mutations_require_caller_operation_identity_and_allow_unpinned_generation() {
    let schema = control_schema_document(None).unwrap_or_else(|error| panic!("schema: {error}"));
    let create_validator = method_validator(&schema, "conversation/create", "request")
        .unwrap_or_else(|error| panic!("create validator: {error}"));
    let actor = session("creator-session");
    let mut request = json!({
        "jsonrpc": "2.0",
        "id": "create-1",
        "method": "conversation/create",
        "params": {
            "operationId": "019f0000-0000-7000-8000-000000000010",
            "endpoint": actor["endpoint"],
            "generation": generation(),
            "workingDirectory": "/tmp/provider-work",
            "createdBy": actor,
            "approver": session("approver-session"),
            "requestedPolicy": {"access": "workspace-write"}
        }
    });
    assert!(create_validator.is_valid(&request));

    request["params"]
        .as_object_mut()
        .unwrap_or_else(|| panic!("params"))
        .remove("operationId");
    assert!(!create_validator.is_valid(&request));
    request["params"]["operationId"] = json!("server-generated-later");
    assert!(!create_validator.is_valid(&request));
    request["params"]["operationId"] = json!("019f0000-0000-7000-8000-000000000010");
    request["params"]
        .as_object_mut()
        .unwrap_or_else(|| panic!("params"))
        .remove("generation");
    assert!(create_validator.is_valid(&request));
}

#[test]
fn failures_preserve_known_target_and_operation_effect_evidence() {
    let schema = control_schema_document(None).unwrap_or_else(|error| panic!("schema: {error}"));
    let validator = method_validator(&schema, "conversation/prompt", "error")
        .unwrap_or_else(|error| panic!("prompt error validator: {error}"));
    let mut failure = json!({
        "jsonrpc": "2.0",
        "id": "prompt-1",
        "error": {
            "code": -32050,
            "message": "provider response was lost",
            "data": {
                "kind": "outcomeUnknown",
                "stage": "settlement",
                "effect": "unknown",
                "message": "provider response was lost",
                "operationId": "019f0000-0000-7000-8000-000000000011",
                "target": session("provider-conversation")
            }
        }
    });
    assert!(validator.is_valid(&failure));
    assert!(control_error_is_valid("conversation/prompt", &failure));

    failure["error"]["data"]
        .as_object_mut()
        .unwrap_or_else(|| panic!("failure data"))
        .remove("effect");
    assert!(!validator.is_valid(&failure));
}

#[test]
fn unavailable_conversation_failure_carries_catalog_recovery_and_legacy_errors_decode() {
    let schema = control_schema_document(None).expect("schema");
    let validator =
        method_validator(&schema, "conversation/create", "error").expect("create error validator");
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
    let response = json!({"jsonrpc":"2.0","id":"create-1","error":{
        "code":-32050,"message":data["message"],"data":data
    }});
    assert!(validator.is_valid(&response));

    let mut legacy = response["error"]["data"].clone();
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
    let schema = control_schema_document(None).unwrap_or_else(|error| panic!("schema: {error}"));
    let show_validator = method_validator(&schema, "conversation/operationShow", "response")
        .unwrap_or_else(|error| panic!("show validator: {error}"));
    let show_response = json!({
        "jsonrpc": "2.0",
        "id": "show-1",
        "result": operation_snapshot()
    });
    assert!(show_validator.is_valid(&show_response));

    let wait_validator = method_validator(&schema, "conversation/operationWait", "response")
        .unwrap_or_else(|error| panic!("wait validator: {error}"));
    let wait_response = json!({
        "jsonrpc": "2.0",
        "id": "wait-1",
        "result": {
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
        }
    });
    let wait_errors = wait_validator
        .iter_errors(&wait_response)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(wait_errors.is_empty(), "{}", wait_errors.join("; "));

    let mut invalid_show = show_response;
    invalid_show["result"]["prompt"] = json!("must never be durable metadata");
    assert!(!show_validator.is_valid(&invalid_show));
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
