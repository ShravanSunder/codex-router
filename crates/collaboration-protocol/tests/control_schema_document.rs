use collaboration_protocol::control_schema_document;
use serde_json::{Value, json};

#[test]
fn complete_schema_pairs_all_methods_and_preserves_protocol_boundaries() {
    let schema = control_schema_document(None).unwrap_or_else(|error| panic!("schema: {error}"));
    let methods = schema["x-methods"]
        .as_object()
        .unwrap_or_else(|| panic!("method map"));
    assert_eq!(methods.len(), 94);
    for method in [
        "conversation/create",
        "conversation/load",
        "conversation/prompt",
        "conversation/cancel",
        "conversation/operationShow",
        "conversation/operationWait",
        "conversation/operationReconcile",
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
        "board/threadListen",
        "board/threadWait",
        "board/threadListenShow",
        "board/threadListenCancel",
        "board/threadCreate",
        "board/threadJoin",
        "board/threadLeave",
        "board/threadParticipantList",
        "control/initialize",
        "endpoint/list",
        "codex/sessionList",
        "codex/sessionInspect",
        "codex/sessionRename",
        "message/send",
        "codex/turnInterrupt",
        "approval/list",
        "approval/decide",
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
    let record: collaboration_protocol::LifecycleRecord = serde_json::from_value(json!({
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
        "generation":{"serviceEpoch":id,"generation":1},"effectiveAccess":null,"settingsObservation":{"kind":"unavailable","reason":"threadReadOmitsSettings"},"thread":{"id":"thread"}
    }});
    assert!(validator.is_valid(&response));
    response["result"]["thread"]["id"] = json!(42);
    assert!(!validator.is_valid(&response));
    response["result"]["thread"]["id"] = json!("thread");
    response["result"]["extra"] = json!(true);
    assert!(!validator.is_valid(&response));
}

#[test]
fn run_show_schema_accepts_selected_provider_and_peer_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let mut schema = control_schema_document(None)?;
    let response_ref = schema["x-methods"]["run/show"]["response"]["$ref"]
        .as_str()
        .ok_or("run/show response schema missing")?
        .to_owned();
    schema["$ref"] = json!(response_ref);
    let validator = jsonschema::validator_for(&schema)?;
    let service = "00000000-0000-4000-8000-000000000001";
    let endpoint = json!({"serviceId":service,"endpointId":"claude-local"});
    let target = json!({"endpoint":endpoint,"sessionId":"provider-worker"});
    let operation = agent_automation::AttemptId::generate();
    let start = "2026-09-24T00:00:00Z";
    let deadline = "2026-09-24T00:02:00Z";
    let inputs = json!({
        "scheduleChangeId":agent_automation::ChangeId::generate(),
        "instructionRevisionId":agent_automation::RevisionId::generate(),
        "instructionText":"Check provider job",
        "continuity":{"kind":"none"},
        "executionConfiguration":{
            "destination":{"kind":"ownedThread","target":target,"cwd":"/isolated-fixture"},
            "executionTimeoutSeconds":120,
            "model":"fixture-model",
            "effort":"medium"
        }
    });
    let provider = json!({
        "runId":agent_automation::RunId::generate(),
        "scheduleId":agent_automation::ScheduleId::generate(),
        "dueAt":start,
        "state":{"kind":"executing","inputs":inputs,"execution":{
            "kind":"providerAcp","target":target,"operationId":operation,
            "startedAt":start,"deadlineAt":deadline,"effectiveTimeoutSeconds":120
        }},
        "executionEvidence":{
            "route":{"kind":"providerAcp","bindingId":"fixture-binding",
                "generation":{"serviceEpoch":service,"generation":1},
                "target":target,"operationId":operation,"submission":"accepted"},
            "timing":{"dispatchStartedAt":start,"effectiveTimeoutSeconds":120,"deadlineAt":deadline},
            "acceptance":{"outcome":{"kind":"started"},"reachability":"providerAcp",
                "client":{"kind":"providerAcp","operationId":operation}}
        },
        "summary":null
    });
    let _: collaboration_protocol::RunSnapshot = serde_json::from_value(provider.clone())?;
    let response = json!({"jsonrpc":"2.0","id":"provider-run","result":provider});
    if !validator.is_valid(&response) {
        return Err("run/show schema rejected provider route evidence".into());
    }
    let mut obsolete = response;
    obsolete["result"]["executionEvidence"]["native"] = json!({});
    if validator.is_valid(&obsolete) {
        return Err("run/show schema accepted the removed native-only field".into());
    }

    let peer_target = json!({"endpoint":endpoint,"sessionId":"peer-worker"});
    let peer_inputs = json!({
        "scheduleChangeId":agent_automation::ChangeId::generate(),
        "instructionRevisionId":agent_automation::RevisionId::generate(),
        "instructionText":"Notify peer",
        "continuity":{"kind":"none"},
        "executionConfiguration":{
            "destination":{"kind":"ownedThread","target":peer_target,"cwd":"/isolated-fixture"},
            "executionTimeoutSeconds":120,
            "model":"fixture-model",
            "effort":"medium"
        }
    });
    let peer = json!({
        "runId":agent_automation::RunId::generate(),
        "scheduleId":agent_automation::ScheduleId::generate(),
        "dueAt":start,
        "state":{"kind":"finished","inputs":peer_inputs,
            "execution":{"kind":"claudeCodePeer","target":peer_target,"writtenAt":start},
            "outcome":{"kind":"peerMessageWritten","explanation":"written"},
            "summaryRunId":null},
        "executionEvidence":{
            "route":{"kind":"claudeCodePeer","sessionId":"peer-worker","write":"written"},
            "timing":{"dispatchStartedAt":start,"effectiveTimeoutSeconds":120,"deadlineAt":deadline},
            "acceptance":null
        },
        "summary":null
    });
    let _: collaboration_protocol::RunSnapshot = serde_json::from_value(peer.clone())?;
    if !validator.is_valid(&json!({"jsonrpc":"2.0","id":"peer-run","result":peer})) {
        return Err("run/show schema rejected final peer write evidence".into());
    }
    Ok(())
}
