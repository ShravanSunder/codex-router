use collaboration_protocol::{RunExecution, RunExecutionEvidence};
use serde_json::json;

#[test]
fn each_run_execution_identity_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    let target = json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"worker"});
    let start = "2026-09-24T00:00:00Z";
    let deadline = "2026-09-24T01:00:00Z";
    let cases = [
        json!({"kind":"codexAppServer","target":target,"nativeTurnId":"turn-one","startedAt":start,"deadlineAt":deadline,"effectiveTimeoutSeconds":3600}),
        json!({"kind":"providerAcp","target":target,"operationId":agent_automation::OperationId::generate(),"startedAt":start,"deadlineAt":deadline,"effectiveTimeoutSeconds":3600}),
        json!({"kind":"claudeCodePeer","target":target,"writtenAt":start}),
    ];
    for case in cases {
        let decoded: RunExecution = serde_json::from_value(case.clone())?;
        if serde_json::to_value(decoded)? != case {
            return Err("run execution changed during round trip".into());
        }
    }
    Ok(())
}

#[test]
fn selected_provider_evidence_retains_receipt_without_native_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let receipt = json!({"outcome":{"kind":"started"},"reachability":"providerAcp","client":{"kind":"providerAcp","operationId":agent_automation::OperationId::generate()}});
    let value = json!({"route":{"kind":"providerAcp","bindingId":"binding-one","generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1},"target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"worker"},"operationId":agent_automation::OperationId::generate(),"submission":"accepted"},"timing":{"dispatchStartedAt":"2026-09-24T00:00:00Z","deadlineAt":"2026-09-24T01:00:00Z","effectiveTimeoutSeconds":3600},"acceptance":receipt});
    let evidence: RunExecutionEvidence = serde_json::from_value(value.clone())?;
    if serde_json::to_value(evidence)? != value {
        return Err("provider run evidence changed during round trip".into());
    }
    Ok(())
}
