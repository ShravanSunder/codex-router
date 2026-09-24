use collaboration_protocol::DeliveryEvidence;
use serde_json::{json, to_value};

#[test]
fn preselection_delivery_preserves_attempt_identity_without_native_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let attempt_id = agent_automation::AttemptId::generate();
    let pending = json!({"kind":"dispatching","attemptId":attempt_id,"effects":null});
    let decoded: DeliveryEvidence = serde_json::from_value(pending.clone())?;
    if to_value(decoded)? != pending {
        return Err("preselection attempt changed on round trip".into());
    }
    let known_none = json!({
        "kind":"knownNotSubmitted","attemptId":attempt_id,
        "reason":"Host stopped before a route was selected","effects":null
    });
    let decoded: DeliveryEvidence = serde_json::from_value(known_none.clone())?;
    if to_value(decoded)? != known_none {
        return Err("known non-submission lost its reason or attempt identity".into());
    }
    Ok(())
}
