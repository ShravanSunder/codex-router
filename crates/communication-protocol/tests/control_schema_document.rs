use communication_protocol::control_schema_document;
use serde_json::{Value, json};

#[test]
fn complete_schema_pairs_all_methods_and_preserves_protocol_boundaries() {
    let schema = control_schema_document(None).unwrap_or_else(|error| panic!("schema: {error}"));
    let methods = schema["x-methods"]
        .as_object()
        .unwrap_or_else(|| panic!("method map"));
    assert_eq!(methods.len(), 45);
    for method in [
        "automation/configure",
        "automation/status",
        "automation/events",
        "operation/show",
        "operation/reconcile",
        "delivery/reconcile",
        "run/reconcile",
        "delivery/attempts",
        "run/summaries",
        "instruction/list",
        "schedule/list",
        "run/list",
        "revision/list",
        "delivery/list",
        "run/show",
        "run/summaryRetry",
        "run/summarySkip",
        "schedule/prepare",
        "schedule/create",
        "schedule/show",
        "schedule/export",
        "schedule/import",
        "schedule/update",
        "schedule/enable",
        "schedule/disable",
        "wake/list",
        "wake/subscribe",
        "wake/pause",
        "wake/resume",
        "wake/cancel",
        "delivery/show",
        "wake/send",
        "wake/show",
        "instruction/create",
        "instruction/update",
        "instruction/show",
        "control/initialize",
        "endpoint/list",
        "codex/sessionList",
        "codex/sessionInspect",
        "codex/messageSend",
        "codex/turnInterrupt",
        "addressBook/list",
        "lifecycleJournal/read",
        "lifecycleJournal/status",
    ] {
        assert!(methods.contains_key(method), "missing {method}");
        for pairing in ["params", "result", "request", "response", "error"] {
            let pointer = methods[method][pairing]["$ref"]
                .as_str()
                .unwrap_or_else(|| panic!("pairing"));
            assert!(
                schema.pointer(pointer.trim_start_matches('#')).is_some(),
                "unresolved {method} {pairing}"
            );
        }
    }
    let validator =
        jsonschema::validator_for(&schema).unwrap_or_else(|error| panic!("compile: {error}"));
    assert!(
        validator.is_valid(&json!({"jsonrpc":"2.0","id":"1","method":"endpoint/list","params":{}}))
    );
    assert!(
        !validator
            .is_valid(&json!([{ "jsonrpc":"2.0","id":"1","method":"endpoint/list","params":{} }]))
    );
    assert!(!validator.is_valid(
        &json!({"jsonrpc":"2.0","id":"1","method":"endpoint/list","params":{"extra":true}})
    ));
    assert!(!validator.is_valid(
        &json!({"jsonrpc":"2.0","id":"1","method":"endpoint/list","params":{},"extra":true})
    ));
    let mut initialize_error = schema.clone();
    initialize_error["$ref"] = json!("#/$defs/control-initialize-error");
    let validator = jsonschema::validator_for(&initialize_error)
        .unwrap_or_else(|error| panic!("error schema: {error}"));
    let mut error = json!({"jsonrpc":"2.0","id":"1","error":{"code":-32050,"message":"Unavailable","data":{"kind":"unavailable","stage":"initialize","message":"Unavailable"}}});
    assert!(validator.is_valid(&error));
    error["error"]["data"]["kind"] = json!("nativeRejected");
    assert!(!validator.is_valid(&error));
}

struct FixtureNativeSchemas {
    uri: String,
}

#[test]
fn schema_accepts_actual_flattened_lifecycle_records() {
    let mut schema =
        control_schema_document(None).unwrap_or_else(|error| panic!("schema: {error}"));
    schema["$ref"] = json!("#/$defs/lifecycleJournal-read-response");
    let validator =
        jsonschema::validator_for(&schema).unwrap_or_else(|error| panic!("validator: {error}"));
    let id = "00000000-0000-4000-8000-000000000001";
    let record: communication_protocol::LifecycleRecord = serde_json::from_value(json!({
        "journalId":id,"sequence":1,"schemaVersion":1,"observedAt":"2026-09-05T12:00:00Z",
        "source":"observerLifecycle","scope":{"endpoint":{"serviceId":id,"endpointId":"codex-local"},"generation":null,"observerId":id},
        "subject":{"kind":"backend"},"change":{"kind":"coverageLost"}
    })).unwrap_or_else(|error| panic!("record: {error}"));
    let response = json!({"jsonrpc":"2.0","id":"read-1","result":{
        "bounds":{"journalId":id,"earliestSequence":1,"lastSequence":1},
        "records":[record],"next":{"journalId":id,"sequence":1},"caughtUp":true
    }});
    let errors: Vec<_> = validator
        .iter_errors(&response)
        .map(|error| error.to_string())
        .collect();
    assert!(errors.is_empty(), "{}", errors.join("; "));
}
impl jsonschema::Retrieve for FixtureNativeSchemas {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        if uri.as_str() != self.uri {
            return Err("unexpected schema retrieval".into());
        }
        Ok(
            json!({"$schema":"http://json-schema.org/draft-07/schema#","definitions":{"v2":{
                "Thread":{"type":"object","required":["id"],"properties":{"id":{"type":"string"}}},
                "ThreadStatus":{"type":"object"},
                "UserInput":{"type":"object","required":["type","text"],"properties":{"type":{"const":"text"},"text":{"type":"string"}}}
            }}}),
        )
    }
}
#[test]
fn bound_native_thread_uses_exact_offline_schema_and_closed_control_result() {
    let digest = format!("sha256:{}", "a".repeat(64))
        .try_into()
        .unwrap_or_else(|error| panic!("digest: {error}"));
    let mut schema =
        control_schema_document(Some(&digest)).unwrap_or_else(|error| panic!("schema: {error}"));
    schema["$ref"] = json!("#/$defs/codex-sessionInspect-response");
    let validator = jsonschema::options()
        .with_retriever(FixtureNativeSchemas {
            uri: format!(
                "codex-schema://{}/codex_app_server_protocol.schemas.json",
                "a".repeat(64)
            ),
        })
        .build(&schema)
        .unwrap_or_else(|error| panic!("compile bound schema: {error}"));
    let id = "00000000-0000-4000-8000-000000000001";
    let mut response = json!({"jsonrpc":"2.0","id":"inspect-1","result":{
        "target":{"endpoint":{"serviceId":id,"endpointId":"codex-local"},"sessionId":"thread"},
        "generation":{"serviceEpoch":id,"generation":1},"thread":{"id":"thread"}
    }});
    assert!(validator.is_valid(&response));
    response["result"]["thread"]["id"] = json!(42);
    assert!(!validator.is_valid(&response));
    response["result"]["thread"]["id"] = json!("thread");
    response["result"]["extra"] = json!(true);
    assert!(!validator.is_valid(&response));
}
