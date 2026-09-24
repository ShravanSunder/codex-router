use agent_automation::{ContinuityInput, SummaryAttempt, SummarySource, SummarySourceReference};
use serde_json::{Value, json};

#[test]
fn old_stored_summary_shapes_decode_as_native_turn() -> Result<(), Box<dyn std::error::Error>> {
    let attempt_id = agent_automation::AttemptId::generate();
    let attempt: SummaryAttempt<String, String> = serde_json::from_value(json!({
        "attemptId":attempt_id,"sourceTarget":"worker","sourceTurnId":"turn-one",
        "target":null,"nativeTurnId":null,"effectiveTimeoutSeconds":60,
        "startedAtMs":1000,"deadlineAtMs":61000,"phase":"preparing",
        "effects":{"target":null,"generation":null,"clientUserMessageId":null,
            "nativeTurnId":null,"nativeSubmissionId":null,"allocation":"notRequested",
            "resume":"notRequested","submission":"notDispatched","cessation":"notApplicable"},
        "explanation":null
    }))?;
    if serde_json::to_value(&attempt)?["sourceReference"]
        != json!({"kind":"nativeTurn","turnId":"turn-one"})
    {
        return Err("legacy summary attempt lost its native source".into());
    }

    let source: SummarySource<String> = serde_json::from_value(json!({
        "kind":"completed","sourceTarget":"worker","sourceTurnId":"turn-one",
        "summaryAttemptId":attempt_id
    }))?;
    if serde_json::to_value(source)?["sourceReference"]
        != json!({"kind":"nativeTurn","turnId":"turn-one"})
    {
        return Err("legacy completed source lost its native turn".into());
    }

    let continuity: ContinuityInput<String> = serde_json::from_value(json!({
        "kind":"localSummary","text":"summary","sourceRunId":agent_automation::RunId::generate(),
        "sourceTarget":"worker","sourceTurnId":"turn-one"
    }))?;
    if serde_json::to_value(continuity)?["sourceReference"]
        != json!({"kind":"nativeTurn","turnId":"turn-one"})
    {
        return Err("legacy continuity lost its native source".into());
    }
    Ok(())
}

#[test]
fn provider_operation_reference_round_trips_without_turn_id()
-> Result<(), Box<dyn std::error::Error>> {
    let reference = SummarySourceReference::ProviderOperation {
        attempt_id: agent_automation::AttemptId::generate(),
    };
    let encoded = serde_json::to_value(&reference)?;
    if encoded["kind"] != "providerOperation" || encoded.get("turnId").is_some() {
        return Err("provider source invented a native turn".into());
    }
    let decoded: SummarySourceReference = serde_json::from_value(encoded.clone())?;
    if serde_json::to_value(decoded)? != encoded {
        return Err("provider source changed during round trip".into());
    }
    let _: Value = encoded;
    Ok(())
}
