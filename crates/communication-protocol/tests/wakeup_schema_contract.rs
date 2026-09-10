use communication_protocol::control_schema_document;
use serde_json::json;

#[test]
fn wake_creation_preserves_message_modes_and_rejects_invalid_timing()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = control_schema_document(None)?;
    let validator = jsonschema::validator_for(&schema)?;
    let target = json!({"endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},"sessionId":"thread-b"});
    let request = json!({"jsonrpc":"2.0","id":"wake-1","method":"wake/send","params":{
        "operationId":"019f0000-0000-7000-8000-000000000002",
        "message":{"target":target,"content":{"kind":"humanUser","text":"Check progress"},"delivery":"steer","generationGuard":null},
        "timing":{"kind":"after","seconds":60},"expiry":{"kind":"none"}
    }});
    if !validator.is_valid(&request) {
        return Err(format!(
            "wake creation missing from public schema: {}",
            validator
                .iter_errors(&request)
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        )
        .into());
    }
    let mut absent = request.clone();
    absent["params"]["message"]
        .as_object_mut()
        .ok_or("missing fixture object")?
        .remove("generationGuard");
    if validator.is_valid(&absent) {
        return Err("nullable generation guard was not required".into());
    }
    if serde_json::from_value::<communication_protocol::WakeSendRequest>(absent["params"].clone())
        .is_ok()
    {
        return Err("Rust request allowed an omitted nullable field".into());
    }
    for seconds in [0, 31536001] {
        let mut invalid = request.clone();
        invalid["params"]["timing"]["seconds"] = json!(seconds);
        if validator.is_valid(&invalid) {
            return Err("wake timing bounds not enforced".into());
        }
    }
    let mut invalid = request.clone();
    invalid["params"]["message"]["proofOfSender"] = json!(true);
    if validator.is_valid(&invalid) {
        return Err("saved message allows undeclared fields".into());
    }
    let mut invalid = request;
    invalid["params"]["timing"] = json!({"kind":"cron","expression":"*/10 * * * *"});
    if validator.is_valid(&invalid) {
        return Err("cron admitted without explicit timezone".into());
    }
    Ok(())
}

#[test]
fn wake_failure_round_trip_retains_field_guidance() -> Result<(), Box<dyn std::error::Error>> {
    let value = json!({"kind":"invalidField","field":"timing","constraint":"explicit IANA timezone required","stage":"validation","message":"Provide a valid timezone","operationId":null,"wakeupId":null,"effects":{"kind":"local","mutation":"none"},"nextAction":"correctRequest"});
    let schema = control_schema_document(None)?;
    let validator = jsonschema::validator_for(&schema)?;
    let response = json!({"jsonrpc":"2.0","id":"wake-1","error":{"code":-32050,"message":"Wake failed","data":&value}});
    if !validator.is_valid(&response) {
        return Err("structured wake failure omitted from schema".into());
    }
    let parsed: communication_protocol::WakeFailure = serde_json::from_value(value.clone())?;
    if serde_json::to_value(parsed)? != value {
        return Err("error guidance changed during decoding".into());
    }
    Ok(())
}

#[test]
fn lifecycle_response_carries_dispatched_effects_and_discarded_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = control_schema_document(None)?;
    for method in ["wake/pause", "wake/resume", "wake/cancel", "delivery/show"] {
        if schema
            .pointer(&format!("/x-methods/{}", method.replace('/', "~1")))
            .is_none()
        {
            return Err(format!("missing lifecycle method {method}").into());
        }
    }
    let value = json!({"deliveryId":"019f0000-0000-7000-8000-000000000001","target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"B"},"mode":"queue","source":{"kind":"wake","wakeupId":"019f0000-0000-7000-8000-000000000002","occurrenceId":"019f0000-0000-7000-8000-000000000003"},"eligibleAt":"2026-09-08T00:00:00.000Z","expiresAt":null,"disposition":"pending","evidence":{"kind":"notDispatched"}});
    let parsed: communication_protocol::DeliveryInspection = serde_json::from_value(value.clone())?;
    if serde_json::to_value(parsed)? != value {
        return Err("delivery inspection lost fields".into());
    }
    let validator = jsonschema::validator_for(&schema)?;
    if !validator.is_valid(&json!({"jsonrpc":"2.0","id":"d1","result":value})) {
        return Err("delivery result absent from schema".into());
    }
    Ok(())
}
