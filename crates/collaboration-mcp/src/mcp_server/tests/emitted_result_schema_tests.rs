//! The advertised output schemas accept the results tools build from real domain values:
//! classified Codex session failures, lifecycle records decoded from the journal's wire form,
//! and provider and peer run evidence decoded through `RunSnapshot`'s validation. The
//! hand-written samples for every tool live in `schema_tests.rs`.
use super::*;
use collaboration_service::collaboration_application::{
    NativeSessionFailure, NativeSessionFailureKind, NativeSessionStage,
};

fn advertised_output_validator(tool_name: &str) -> jsonschema::Validator {
    let schema = CollaborationMcpServer::catalog_only()
        .resolved_tools()
        .into_iter()
        .find(|tool| tool.name == tool_name)
        .and_then(|tool| tool.output_schema)
        .unwrap_or_else(|| panic!("{tool_name} advertises an output schema"));
    jsonschema::validator_for(&Value::Object((*schema).clone()))
        .unwrap_or_else(|error| panic!("{tool_name} output validator: {error}"))
}

fn validated_structured_content(
    validator: &jsonschema::Validator,
    tool_name: &str,
    result: rmcp::model::CallToolResult,
) -> Value {
    let structured = result
        .structured_content
        .unwrap_or_else(|| panic!("{tool_name} result is structured"));
    if let Err(error) = validator.validate(&structured) {
        panic!("{tool_name} advertised schema rejects its own result: {error}; {structured}");
    }
    structured
}

#[test]
fn session_inspect_and_rename_schemas_accept_every_classified_failure_they_emit() {
    let native_rejection = |stage, message: &str, reason, next_action, native_code| {
        NativeSessionFailure::NativeRejected {
            stage,
            message: message.to_owned(),
            reason,
            next_action,
            native_code,
        }
    };
    for (tool_name, stage, possible_effect) in [
        (
            "session_inspect",
            NativeSessionStage::Inspect,
            OperationEffect::None,
        ),
        (
            "session_rename",
            NativeSessionStage::Rename,
            OperationEffect::Unknown,
        ),
    ] {
        // Arrange: one failure per class the classifier hands these tools.
        let validator = advertised_output_validator(tool_name);
        let mut failures = vec![
            native_rejection(
                stage,
                "thread has an active turn and is busy",
                "busy",
                "useDeliverySteer",
                None,
            ),
            native_rejection(
                stage,
                "Native control operation failed",
                "unknown",
                "retryLater",
                Some(-32099),
            ),
            native_rejection(
                stage,
                "thread 019f0000-0000-7000-8000-000000000001 already has an active writer",
                "heldByAnotherClient",
                "messageFromHoldingCodexClient",
                None,
            ),
            NativeSessionFailure::Refused {
                kind: NativeSessionFailureKind::Unavailable,
                stage,
                message: "Native control operation failed".to_owned(),
            },
        ];
        if stage == NativeSessionStage::Rename {
            failures.push(NativeSessionFailure::NameMismatch {
                requested: "Review".to_owned(),
                effective: "Old name".to_owned(),
            });
        }

        for failure in failures {
            let classification = serde_json::to_value(&failure).expect("failure encodes");

            // Act
            let result =
                super::super::application_result::<Value, _>(Err(failure), possible_effect);

            // Assert: the schema admits the error, and the error keeps the classification
            // (reason, corrective action, native code, requested and echoed names).
            assert_eq!(result.is_error, Some(true), "{tool_name} reports an error");
            let structured = validated_structured_content(&validator, tool_name, result);
            assert_eq!(
                structured["data"], classification,
                "{tool_name} error keeps the classified failure"
            );
        }
    }
}

#[test]
fn journal_read_schema_accepts_a_page_of_decoded_flattened_lifecycle_records() {
    // Arrange: a page decoded from the journal's wire form, so each record is the flattened
    // observation the journal actually stores.
    let validator = advertised_output_validator("journal_read");
    let id = FIXTURE_SERVICE_ID;
    let page: collaboration_protocol::JournalPage = serde_json::from_value(serde_json::json!({
        "bounds":{"journalId":id,"earliestSequence":1,"lastSequence":1},
        "records":[{
            "journalId":id,"sequence":1,"schemaVersion":1,"observedAt":"2026-09-05T12:00:00Z",
            "source":"observerLifecycle",
            "scope":{"endpoint":{"serviceId":id,"endpointId":"codex-local"},"generation":null,"observerId":id},
            "subject":{"kind":"backend"},"change":{"kind":"coverageLost"}
        }],
        "next":{"journalId":id,"sequence":1},"caughtUp":true
    }))
    .expect("journal page decodes");

    // Act
    let result = super::super::success_result(&page);

    // Assert
    let structured = validated_structured_content(&validator, "journal_read", result);
    assert_eq!(structured["records"][0]["source"], "observerLifecycle");
}

#[test]
fn run_show_schema_accepts_provider_and_peer_run_evidence() {
    // Arrange: one provider run still executing and one finished peer write, each decoded
    // through `RunSnapshot`'s own validation.
    let validator = advertised_output_validator("run_show");
    let endpoint = serde_json::json!({"serviceId":FIXTURE_SERVICE_ID,"endpointId":"claude-local"});
    let start = "2026-09-24T00:00:00Z";
    let deadline = "2026-09-24T00:02:00Z";
    let timing = serde_json::json!({
        "dispatchStartedAt":start,"effectiveTimeoutSeconds":120,"deadlineAt":deadline
    });
    let inputs = |target: &Value, instruction: &str, sequence: u8| {
        serde_json::json!({
            "scheduleChangeId":format!("019f0000-0000-7000-8000-0000000001{sequence:02x}"),
            "instructionRevisionId":format!("019f0000-0000-7000-8000-0000000002{sequence:02x}"),
            "instructionText":instruction,
            "continuity":{"kind":"none"},
            "executionConfiguration":{
                "destination":{"kind":"ownedThread","target":target,"cwd":"/isolated-fixture"},
                "executionTimeoutSeconds":120,"model":"fixture-model","effort":"medium"
            }
        })
    };
    let provider_target = serde_json::json!({"endpoint":endpoint,"sessionId":"provider-worker"});
    let operation = "019f0000-0000-7000-8000-000000000301";
    let provider: collaboration_protocol::RunSnapshot = serde_json::from_value(serde_json::json!({
        "runId":"019f0000-0000-7000-8000-000000000401",
        "scheduleId":"019f0000-0000-7000-8000-000000000501",
        "dueAt":start,
        "state":{"kind":"executing","inputs":inputs(&provider_target, "Check provider job", 1),
            "execution":{"kind":"providerAcp","target":provider_target,"operationId":operation,
                "startedAt":start,"deadlineAt":deadline,"effectiveTimeoutSeconds":120}},
        "executionEvidence":{
            "route":{"kind":"providerAcp","bindingId":"fixture-binding",
                "generation":{"serviceEpoch":FIXTURE_SERVICE_ID,"generation":1},
                "target":provider_target,"operationId":operation,"submission":"accepted"},
            "timing":timing,
            "acceptance":{"outcome":{"kind":"started"},"reachability":"providerAcp",
                "client":{"kind":"providerAcp","operationId":operation}}
        },
        "summary":null
    }))
    .expect("provider run decodes");
    let peer_target = serde_json::json!({"endpoint":endpoint,"sessionId":"peer-worker"});
    let peer: collaboration_protocol::RunSnapshot = serde_json::from_value(serde_json::json!({
        "runId":"019f0000-0000-7000-8000-000000000402",
        "scheduleId":"019f0000-0000-7000-8000-000000000502",
        "dueAt":start,
        "state":{"kind":"finished","inputs":inputs(&peer_target, "Notify peer", 2),
            "execution":{"kind":"claudeCodePeer","target":peer_target,"writtenAt":start},
            "outcome":{"kind":"peerMessageWritten","explanation":"written"},
            "summaryRunId":null},
        "executionEvidence":{
            "route":{"kind":"claudeCodePeer","sessionId":"peer-worker","write":"written"},
            "timing":timing,"acceptance":null
        },
        "summary":null
    }))
    .expect("peer run decodes");

    // Act
    let provider_result = super::super::success_result(&provider);
    let peer_result = super::super::success_result(&peer);

    // Assert
    let mut provider_structured =
        validated_structured_content(&validator, "run_show", provider_result);
    validated_structured_content(&validator, "run_show", peer_result);
    provider_structured["executionEvidence"]["native"] = serde_json::json!({});
    assert!(
        !validator.is_valid(&provider_structured),
        "run_show schema accepts the removed native-only evidence field"
    );
}
